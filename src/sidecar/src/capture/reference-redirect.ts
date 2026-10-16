/**
 * How a program built from one owner config reads the projects that config
 * references (carrick#2418): as an editor reads them.
 *
 * A workspace package a service references is usually not built on a scanned
 * checkout: its `package.json` points `types` at `dist/index.d.ts`, which
 * does not exist, so the import read as the error type. The compiler's editor
 * mode (`projectReferences` with `useSourceOfProjectReferenceRedirect`) reads
 * the package's source instead, under the package's own options. The
 * sidecar's programs cannot be handed references (ts-morph never passes them,
 * and the capture's emit would then skip the sources), so this module does
 * what the compiler's `updateHostForUseSourceOfProjectReferenceRedirect` does,
 * in the resolver:
 *
 *  1. Every project in the owner's reference closure is parsed through the
 *     read bound (`ProjectGraph`), its options through
 *     `sidecarCompilerOptions`.
 *  2. Each of its inputs maps to its declaration output, by the compiler's
 *     own `getOutputFileNames` with declarations on (a project with `outFile`
 *     has no per-file map and is skipped). A project that leaves `rootDir`
 *     unset is laid out as TypeScript 5 lays it out: under the config's
 *     directory when `composite`, else under the common directory of its
 *     inputs. TypeScript 6 puts every such project under the config's
 *     directory (`src/index.ts` to `dist/src/index.d.ts`), where a package
 *     written against 5 points its `types` at `dist/index.d.ts`.
 *  3. The resolver's host answers that an output exists while its source
 *     does, and that the directories outputs sit in exist. An output is
 *     looked up by the real path of its deepest existing ancestor: a package
 *     reached through a `node_modules` link has no `dist/`, so the real path
 *     of the output itself is the link path and would miss the map.
 *  4. An import made from a referenced project's own file resolves under that
 *     project's options (`redirectedReference`), and an answer that lands on
 *     an output becomes its source. A source inside the checkout and under no
 *     `node_modules` is not an external-library import, so the origin gates
 *     read it as the user's code.
 *
 * A package the owner does not reference stays unresolved, as in the editor.
 * Nothing here writes, and every read goes through the host it wraps.
 */

import * as path from 'node:path';
import ts from 'typescript';
import { boundedSys, isInsideCheckout, realOf, untrustedReadBound } from './checkout.js';
import { sidecarCompilerOptions } from './compiler-options.js';
import { ProjectGraph, type ServiceProject } from './project-references.js';

/** The part of a resolved module this module reads and rewrites. */
interface ResolvedModuleLike {
  resolvedFileName: string;
  extension?: string;
  isExternalLibraryImport?: boolean;
  resolvedUsingTsExtension?: boolean;
}

/** The part of a module resolution host this module wraps. */
interface ResolutionHostLike {
  fileExists(fileName: string): boolean;
  directoryExists?(directoryName: string): boolean;
}

export interface ReferenceRedirect {
  /**
   * The referenced project that lists `containingFile`, for
   * `ts.resolveModuleName`'s `redirectedReference`; `undefined` for a file
   * of the owner itself.
   */
  referenceFor(containingFile: string): ts.ResolvedProjectReference | undefined;
  /** `host` answering for the outputs of referenced projects (step 3). */
  resolutionHost<H extends ResolutionHostLike>(host: H): H;
  /** `answer`, or the source behind it when it is a referenced output (step 4). */
  sourceAnswer<R extends ResolvedModuleLike | undefined>(answer: R): R;
}

const DECLARATION = /\.d\.[cm]?ts$/;

/** `realOf`, cached per directory: the resolver asks about many files in few directories. */
function realPaths(): (p: string) => string {
  const dirs = new Map<string, string>();
  return (p) => {
    const abs = path.resolve(p);
    const dir = path.dirname(abs);
    let real = dirs.get(dir);
    if (real === undefined) {
      real = realOf(dir);
      dirs.set(dir, real);
    }
    return path.join(real, path.basename(abs));
  };
}

/** The directory every input sits under, as TypeScript computes it. */
function commonDirectory(files: readonly string[], ignoreCase: boolean): string | undefined {
  let common: string[] | undefined;
  const key = (part: string): string => (ignoreCase ? part.toLowerCase() : part);
  for (const file of files) {
    const parts = path.dirname(path.resolve(file)).split(path.sep);
    if (!common) {
      common = parts;
      continue;
    }
    let n = 0;
    while (n < common.length && n < parts.length && key(common[n]) === key(parts[n])) n += 1;
    common = common.slice(0, n);
  }
  return common && common.length > 0 ? common.join(path.sep) || path.sep : undefined;
}

/**
 * `parsed` with declarations on and, when it leaves `rootDir` unset,
 * TypeScript 5's root (see the header, step 2).
 */
function declarationLayout(parsed: ts.ParsedCommandLine, ignoreCase: boolean): ts.ParsedCommandLine {
  const options: ts.CompilerOptions = { ...parsed.options, declaration: true, emitDeclarationOnly: true };
  if (!options.rootDir) {
    const rootDir = options.composite
      ? path.dirname(path.resolve(String(options.configFilePath)))
      : commonDirectory(
          parsed.fileNames.filter((file) => !DECLARATION.test(file) && !file.endsWith('.json')),
          ignoreCase
        );
    if (rootDir) options.rootDir = rootDir;
  }
  return { ...parsed, options };
}

/**
 * The redirect for a program whose owner reaches `references` (its reference
 * closure, from `ProjectGraph.closureOf`), or `undefined` when there are none.
 */
export function referenceRedirect(
  references: readonly ServiceProject[],
  repoRoot: string
): ReferenceRedirect | undefined {
  if (references.length === 0) return undefined;
  const realOf = realPaths();
  const ignoreCase = !ts.sys.useCaseSensitiveFileNames;
  /** Real path of an output -> its source. */
  const outputs = new Map<string, string>();
  /** Real paths of the directories outputs sit in, up to their project's. */
  const outputDirs = new Set<string>();
  /** Base names of outputs and of their directories: the cheap test first. */
  const names = new Set<string>();
  const byFile = new Map<string, ts.ResolvedProjectReference>();
  for (const project of references) {
    const configFile = project.parsed.options.configFile as ts.TsConfigSourceFile | undefined;
    if (!configFile) continue;
    // Its own imports resolve under its options with TypeScript 5's defaults.
    const commandLine: ts.ParsedCommandLine = { ...project.parsed, options: sidecarCompilerOptions(project.parsed.options) };
    const reference: ts.ResolvedProjectReference = { commandLine, sourceFile: configFile };
    const projectDir = realOf(path.dirname(project.configPath));
    const layout = project.parsed.options.outFile ? undefined : declarationLayout(project.parsed, ignoreCase);
    for (const file of project.parsed.fileNames) {
      const key = path.resolve(file);
      if (!byFile.has(key)) byFile.set(key, reference);
      if (!layout || file.endsWith('.json') || DECLARATION.test(file)) continue;
      const declaration = ts.getOutputFileNames(layout, file, ignoreCase).find((out) => DECLARATION.test(out));
      if (!declaration) continue;
      const output = realOf(declaration);
      if (outputs.has(output)) continue;
      outputs.set(output, key);
      names.add(path.basename(output));
      for (let dir = path.dirname(output); !outputDirs.has(dir); dir = path.dirname(dir)) {
        outputDirs.add(dir);
        names.add(path.basename(dir));
        if (dir === projectDir || path.dirname(dir) === dir) break;
      }
    }
  }
  const sourceOf = (file: string): string | undefined =>
    DECLARATION.test(file) && names.has(path.basename(file)) ? outputs.get(realOf(file)) : undefined;
  const isOutputDir = (dir: string): boolean => names.has(path.basename(dir)) && outputDirs.has(realOf(dir));
  return {
    referenceFor: (containingFile) => byFile.get(path.resolve(containingFile)),
    resolutionHost: <H extends ResolutionHostLike>(host: H): H => {
      const wrapped = Object.create(host) as H;
      wrapped.fileExists = (file: string): boolean => {
        if (host.fileExists(file)) return true;
        const source = sourceOf(file);
        return source !== undefined && host.fileExists(source);
      };
      if (host.directoryExists) {
        const directoryExists = host.directoryExists.bind(host);
        wrapped.directoryExists = (dir: string): boolean => directoryExists(dir) || isOutputDir(dir);
      }
      return wrapped;
    },
    sourceAnswer: <R extends ResolvedModuleLike | undefined>(answer: R): R => {
      if (!answer) return answer;
      const source = sourceOf(answer.resolvedFileName);
      if (!source) return answer;
      const own =
        isInsideCheckout(source.replace(/\\/g, '/'), repoRoot) &&
        !source.includes(`${path.sep}node_modules${path.sep}`);
      return {
        ...answer,
        resolvedFileName: source,
        extension: path.extname(source),
        resolvedUsingTsExtension: false,
        isExternalLibraryImport: own ? false : answer.isExternalLibraryImport,
      };
    },
  };
}

/**
 * For the loader, which builds a program from one config path: the redirect
 * for that config's reference closure. A config that references nothing is
 * not parsed (`references` is never inherited through `extends`).
 */
export function referenceRedirectOf(configPath: string, repoRoot: string): ReferenceRedirect | undefined {
  const sys = boundedSys(untrustedReadBound(path.dirname(configPath)));
  const raw = ts.readConfigFile(configPath, (file) => sys.readFile(file)).config as
    | { references?: unknown }
    | undefined;
  if (!Array.isArray(raw?.references) || raw.references.length === 0) return undefined;
  let graph: ProjectGraph;
  try {
    graph = new ProjectGraph(configPath);
  } catch {
    return undefined;
  }
  return referenceRedirect(graph.closureOf(graph.named), repoRoot);
}

/**
 * For the capture's anchor program: `host` resolving every import through
 * `redirect`, with the module resolution cache it uses handed to the program.
 * A host with its own resolver and no cache makes declaration emit name a
 * transitive package through another (TS2742).
 */
export function redirectingCompilerHost(
  host: ts.CompilerHost,
  options: ts.CompilerOptions,
  redirect: ReferenceRedirect
): ts.CompilerHost {
  const resolutionHost = redirect.resolutionHost(host);
  const cache = ts.createModuleResolutionCache(
    host.getCurrentDirectory(),
    (fileName) => host.getCanonicalFileName(fileName),
    options
  );
  host.getModuleResolutionCache = () => cache;
  host.resolveModuleNameLiterals = (literals, containingFile, redirectedReference, compilerOptions, containingSourceFile) =>
    literals.map((literal) => {
      const answer = ts.resolveModuleName(
        literal.text,
        containingFile,
        compilerOptions,
        resolutionHost,
        cache,
        redirectedReference ?? redirect.referenceFor(containingFile),
        ts.getModeForUsageLocation(containingSourceFile, literal, compilerOptions)
      );
      return { ...answer, resolvedModule: redirect.sourceAnswer(answer.resolvedModule) };
    });
  return host;
}
