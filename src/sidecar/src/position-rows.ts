/**
 * Per-position compiler rows for a service (carrick#2405, move 2 of the fact
 * layer). Record-only: nothing in a scan reads the file yet.
 *
 * For every source file the scanner sends, one row for each position of four
 * kinds, each holding what the compiler knows there: the symbol, the
 * declaration it is attributed to (file and line), the package that declares
 * it and the type text.
 *
 * | kind      | node                                              | symbol of                 | type of        |
 * |-----------|---------------------------------------------------|---------------------------|----------------|
 * | `call`    | call, `new`, tagged template                      | the callee's name         | the call       |
 * | `member`  | property or element access                        | the name or the argument  | the access     |
 * | `compare` | `=== !== == != < > <= >= instanceof in`           | the left operand          | the left operand |
 * | `ident`   | an identifier in expression position              | itself                    | itself         |
 *
 * A row's key is `(start, end, kind)`: TypeScript `getStart()`/`getEnd()` of
 * the node, in UTF-16 code units from the start of the file, which is what
 * the scanner's `sidecar_position` gives for an SWC span. Every path written
 * is relative to the scanned repo's root, with `/` separators.
 *
 * Each file is read in the program of the project that owns it
 * (`ProjectLoader.projectKeyFor`, carrick#1604), one owner's program at a
 * time: a group's program is released before the next one is built. A file
 * the owner's program does not take (JavaScript under a config without
 * `allowJs`) is read after every owner, in that owner's `scripts` program
 * (`ProjectLoader.scriptProjectFor`, carrick#2413), with the owner's root
 * declaration files beside it so that the globals they declare are declared.
 *
 * The file format is pinned on the scanner side by `src/position_rows.rs`.
 */

import * as crypto from 'node:crypto';
import * as fs from 'node:fs';
import * as path from 'node:path';
import * as zlib from 'node:zlib';
import { ts, type Project } from 'ts-morph';
import { WriteGuard, installedPackageDirectory, isDeclarationFileName } from './capture/index.js';
import { attributedDeclaration, installedPackageOf } from './declaration-attribution.js';
import { typeToStringAsDeclared } from './print-type.js';
import type { ProjectLoader } from './project-loader.js';
import type { PositionRowsCounts } from './types.js';

/** The format's version, written in the header. */
export const POSITION_ROWS_VERSION = 1;

/** The type is `any`, declared or inferred. Never set with the error type. */
export const FLAG_ANY = 1;
/** The type is the compiler's error type: something did not resolve. */
export const FLAG_ERROR_TYPE = 2;
/** The checker threw at this node; the row holds nothing else. */
export const FLAG_CHECKER_THREW = 4;
/** The declaration is outside the checkout, not the default library and in no installed package. */
export const FLAG_DECLARATION_UNNAMED = 8;

export type PositionKind = 'call' | 'member' | 'compare' | 'ident';

/** `[kind, start, end, symbol, decl_file, decl_line, decl_package, type_text, flags]` */
export type PositionRow = [
  PositionKind,
  number,
  number,
  string | null,
  string | null,
  number | null,
  string | null,
  string | null,
  number,
];

export interface PositionRowsOptions {
  loader: ProjectLoader;
  /** Absolute paths, written in this order. */
  files: string[];
  scanRoot: string;
  outPath: string;
  service: string;
  /** Called after each file is walked or skipped. */
  onFile?: () => void;
}

const COMPARE_OPERATORS = new Set<ts.SyntaxKind>([
  ts.SyntaxKind.EqualsEqualsEqualsToken,
  ts.SyntaxKind.ExclamationEqualsEqualsToken,
  ts.SyntaxKind.EqualsEqualsToken,
  ts.SyntaxKind.ExclamationEqualsToken,
  ts.SyntaxKind.LessThanToken,
  ts.SyntaxKind.GreaterThanToken,
  ts.SyntaxKind.LessThanEqualsToken,
  ts.SyntaxKind.GreaterThanEqualsToken,
  ts.SyntaxKind.InstanceOfKeyword,
  ts.SyntaxKind.InKeyword,
]);

/** One line of the file per source file. */
interface FileLine {
  f: string;
  h?: string;
  skip?: 'unreadable' | 'not_in_program';
  r: PositionRow[];
}

/**
 * Walk `files`, write their rows to `outPath` and say what was written.
 *
 * The file is written beside `outPath` as `<outPath>.partial` and renamed
 * over it, so a walk that dies leaves no half-written rows file. Every write
 * goes through a {@link WriteGuard} over the file's directory, which refuses
 * a directory inside the scanned trees anywhere but beneath `.carrick`.
 */
export function writePositionRows(options: PositionRowsOptions): PositionRowsCounts {
  const { loader, files, scanRoot, outPath, service } = options;
  const outDir = path.dirname(outPath);
  const guard = WriteGuard.of({ dirs: [outDir], protect: [loader.getRepoRoot(), scanRoot] });
  const paths = new PathNames(scanRoot);
  const lines = new Array<string>(files.length);
  const counts: PositionRowsCounts = {
    files: files.length,
    rows: 0,
    resolved: 0,
    any: 0,
    error_type: 0,
    bytes: 0,
    load_ms: 0,
    walk_ms: 0,
  };

  // Files in request order within each owner's group; groups in the order
  // their first file appears.
  const groups = new Map<string, number[]>();
  files.forEach((file, index) => {
    const key = loader.projectKeyFor(file);
    groups.set(key, [...(groups.get(key) ?? []), index]);
  });

  // Files an owner's program did not take, by owner, with that program's
  // root declaration files.
  const untaken = new Map<string, { indexes: number[]; declarations: string[] }>();

  /**
   * Add each file of `indexes` to `project`, build its program once, and walk
   * every file the program takes. Returns the files it did not take; every
   * other file is settled, and reported to `onFile`.
   */
  const walkGroup = (project: Project, indexes: number[], hashes: Map<number, string>): number[] => {
    const loadStarted = performance.now();
    const read = new Map<number, string | undefined>();
    for (const index of indexes) {
      const file = files[index];
      if (!hashes.has(index)) {
        let bytes: Buffer;
        try {
          bytes = fs.readFileSync(file);
        } catch {
          lines[index] = fileLine({ f: paths.relative(file), skip: 'unreadable', r: [] });
          options.onFile?.();
          continue;
        }
        hashes.set(index, crypto.createHash('sha256').update(bytes).digest('hex'));
      }
      // A file no program lists is added, as `infer` adds one.
      let sourceFile = project.getSourceFile(file);
      if (!sourceFile) {
        try {
          sourceFile = project.addSourceFileAtPath(file);
        } catch {
          sourceFile = undefined;
        }
      }
      read.set(index, sourceFile?.getFilePath());
    }
    // Built once, after every file the group adds.
    const program = project.getProgram().compilerObject;
    counts.load_ms += performance.now() - loadStarted;

    const walkStarted = performance.now();
    const walker = new PositionWalker(program, paths, loader);
    const missed: number[] = [];
    for (const index of indexes) {
      if (!read.has(index)) continue;
      const fileName = read.get(index);
      const sourceFile = fileName ? program.getSourceFile(fileName) : undefined;
      if (!sourceFile) {
        missed.push(index);
        continue;
      }
      const rows = walker.rowsOf(sourceFile);
      for (const row of rows) {
        counts.rows += 1;
        if (row[3] !== null) counts.resolved += 1;
        if (row[8] & FLAG_ANY) counts.any += 1;
        if (row[8] & FLAG_ERROR_TYPE) counts.error_type += 1;
      }
      lines[index] = fileLine({ f: paths.relative(files[index]), h: hashes.get(index), r: rows });
      options.onFile?.();
    }
    counts.walk_ms += performance.now() - walkStarted;
    return missed;
  };

  const hashes = new Map<number, string>();
  for (const [key, indexes] of groups) {
    const project = loader.getProjectFor(key);
    const missed = walkGroup(project, indexes, hashes);
    if (missed.length > 0) {
      const declarations = project
        .getProgram()
        .compilerObject.getRootFileNames()
        .filter(isDeclarationFileName);
      untaken.set(key, { indexes: missed, declarations });
    }
    // One owner's program at a time.
    loader.releaseProjectFor(key);
  }

  for (const [key, { indexes, declarations }] of untaken) {
    const loadStarted = performance.now();
    const project = loader.scriptProjectFor(key);
    for (const declaration of declarations) {
      try {
        project.addSourceFileAtPath(declaration);
      } catch {
        // A declaration file that went away is not the walk's to report.
      }
    }
    counts.load_ms += performance.now() - loadStarted;
    for (const index of walkGroup(project, indexes, hashes)) {
      lines[index] = fileLine({ f: paths.relative(files[index]), h: hashes.get(index), skip: 'not_in_program', r: [] });
      options.onFile?.();
    }
  }

  const writeStarted = performance.now();
  const header = JSON.stringify({
    v: POSITION_ROWS_VERSION,
    typescript: ts.version,
    service,
    files: files.length,
  });
  const gzipped = zlib.gzipSync(Buffer.from([header, ...lines].join('\n') + '\n', 'utf8'));
  const partial = `${outPath}.partial`;
  guard.mkdir(outDir);
  guard.writeFile(partial, gzipped);
  guard.rename(partial, outPath);
  counts.bytes = gzipped.length;
  counts.walk_ms = Math.round(counts.walk_ms + performance.now() - writeStarted);
  counts.load_ms = Math.round(counts.load_ms);
  return counts;
}

/** A file's line, with its keys in the pinned order. */
function fileLine(line: FileLine): string {
  return JSON.stringify(line);
}

/**
 * Names for the files rows point at, never an absolute path: relative to the
 * scanned repo's root when inside it (its real path too, so a root reached
 * through a link still contains its files), `lib:<name>` for the compiler's
 * default library, `pkg:<name>/<path>` for an installed package outside the
 * root, and nothing otherwise.
 */
class PathNames {
  private readonly roots: string[];
  private readonly packageNames = new Map<string, string | null>();
  /**
   * Answers by declaring file: a service's rows name a few thousand files
   * hundreds of thousands of times, and the module graph's answer is a scan
   * of its packages (on a Deno service it was 40% of the walk).
   */
  private readonly declaringFiles = new Map<string, string | null>();
  private readonly declaringPackages = new Map<string, string | null>();

  constructor(scanRoot: string) {
    const resolved = path.resolve(scanRoot);
    let real = resolved;
    try {
      real = fs.realpathSync.native(resolved);
    } catch {
      // A root that cannot be resolved is named as given.
    }
    this.roots = real === resolved ? [resolved] : [resolved, real];
  }

  /** The root `file` lies under, and its path from there; `undefined` outside every root. */
  private inside(file: string): { root: string; rel: string } | undefined {
    for (const root of this.roots) {
      const rel = path.relative(root, file);
      if (rel === '' || rel === '..' || rel.startsWith(`..${path.sep}`) || path.isAbsolute(rel)) continue;
      return { root, rel };
    }
    return undefined;
  }

  /** A request file's name: relative to the root, which every file the scanner sends is inside. */
  relative(file: string): string {
    const inside = this.inside(path.resolve(file));
    return toSlashes(inside ? inside.rel : path.basename(file));
  }

  /** What a declaring file is written as, or `null` when nothing names it. */
  declaringFile(fileName: string): string | null {
    let named = this.declaringFiles.get(fileName);
    if (named === undefined) {
      named = this.nameDeclaringFile(fileName);
      this.declaringFiles.set(fileName, named);
    }
    return named;
  }

  private nameDeclaringFile(fileName: string): string | null {
    const inside = this.inside(fileName);
    if (inside) return toSlashes(inside.rel);
    const installed = installedPackageDirectory(this.roots[0], fileName);
    if (installed && fs.existsSync(path.join(installed.directory, 'package.json'))) {
      return `pkg:${installed.name}/${toSlashes(path.relative(installed.directory, fileName))}`;
    }
    return null;
  }

  /**
   * The package a declaring file belongs to: the module graph's or the path's
   * answer (`installedPackageOf`), else, for a file inside the root, the
   * `name` of the nearest package.json at or above it, up to the root.
   */
  declaringPackage(
    fileName: string,
    packageOf: (filePath: string) => string | undefined
  ): string | null {
    let named = this.declaringPackages.get(fileName);
    if (named === undefined) {
      named = this.nameDeclaringPackage(fileName, packageOf);
      this.declaringPackages.set(fileName, named);
    }
    return named;
  }

  private nameDeclaringPackage(
    fileName: string,
    packageOf: (filePath: string) => string | undefined
  ): string | null {
    const installed = installedPackageOf(fileName, packageOf);
    if (installed) return installed;
    const inside = this.inside(fileName);
    if (!inside) return null;
    let dir = path.dirname(fileName);
    for (;;) {
      const name = this.packageNameIn(dir);
      if (name !== null) return name;
      if (dir === inside.root) return null;
      const parent = path.dirname(dir);
      if (parent === dir) return null;
      dir = parent;
    }
  }

  private packageNameIn(dir: string): string | null {
    let name = this.packageNames.get(dir);
    if (name === undefined) {
      name = null;
      try {
        const manifest = JSON.parse(fs.readFileSync(path.join(dir, 'package.json'), 'utf8')) as {
          name?: unknown;
        };
        if (typeof manifest.name === 'string' && manifest.name.length > 0) name = manifest.name;
      } catch {
        // No package.json here, or one with no name.
      }
      this.packageNames.set(dir, name);
    }
    return name;
  }

  /**
   * `import("/abs/path")` in a printed type, written with the declaring
   * file's name instead. The print has no enclosing node, so it names every
   * module by the file that declares it and one type reads the same from
   * every file; it also skips the accessibility search an enclosing node
   * costs (40% of the walk on a 600-file service).
   */
  scrubType(text: string): string {
    if (!text.includes('import("')) return text;
    return text.replace(/import\("([^"]+)"\)/g, (whole, specifier: string) => {
      if (!path.isAbsolute(specifier)) return whole;
      return `import("${this.declaringFile(specifier) ?? '<outside the checkout>'}")`;
    });
  }
}

function toSlashes(p: string): string {
  return p.split(path.sep).join('/');
}

/** The rows of one program's files. */
class PositionWalker {
  private readonly checker: ts.TypeChecker;

  constructor(
    private readonly program: ts.Program,
    private readonly paths: PathNames,
    private readonly loader: ProjectLoader
  ) {
    this.checker = program.getTypeChecker();
  }

  rowsOf(sourceFile: ts.SourceFile): PositionRow[] {
    const rows: PositionRow[] = [];
    const visit = (node: ts.Node): void => {
      const kind = kindOf(node);
      if (kind) rows.push(this.row(kind, node, sourceFile));
      ts.forEachChild(node, visit);
    };
    ts.forEachChild(sourceFile, visit);
    return rows;
  }

  private row(kind: PositionKind, node: ts.Node, sourceFile: ts.SourceFile): PositionRow {
    const start = node.getStart(sourceFile);
    const end = node.getEnd();
    try {
      const symbol = this.symbolOf(kind, node);
      const typed = kind === 'compare' ? (node as ts.BinaryExpression).left : node;
      const type = this.checker.getTypeAtLocation(typed);
      let flags = 0;
      if (type.flags & ts.TypeFlags.Any) {
        flags |= (type as { intrinsicName?: string }).intrinsicName === 'error' ? FLAG_ERROR_TYPE : FLAG_ANY;
      }
      const typeText = this.paths.scrubType(typeToStringAsDeclared(this.checker, type, undefined));
      if (!symbol) return [kind, start, end, null, null, null, null, typeText, flags];
      const declaration = attributedDeclaration(() => this.program, symbol.declarations ?? []);
      if (!declaration) return [kind, start, end, ts.symbolName(symbol), null, null, null, typeText, flags];
      // A file's own module symbol is named by the compiler as the quoted
      // absolute file name (`"/<checkout>/pkg/index"`). Name it as `decl_file`
      // names the file. A symbol that merely lives in a file (CommonJS
      // `exports`) keeps its name.
      const symbolName = ts.symbolName(symbol);
      const isModule = declaration.kind === ts.SyntaxKind.SourceFile && symbolName.startsWith('"');
      const declaringFile = declaration.getSourceFile();
      const declFile = this.program.isSourceFileDefaultLibrary(declaringFile)
        ? `lib:${path.basename(declaringFile.fileName)}`
        : this.paths.declaringFile(declaringFile.fileName);
      if (declFile === null) flags |= FLAG_DECLARATION_UNNAMED;
      const declLine =
        declaringFile.getLineAndCharacterOfPosition(declaration.getStart(declaringFile)).line + 1;
      const declPackage = this.paths.declaringPackage(declaringFile.fileName, (file) =>
        this.loader.packageOf(file)
      );
      const name = isModule ? declFile : symbolName;
      return [kind, start, end, name, declFile, declLine, declPackage, typeText, flags];
    } catch {
      return [kind, start, end, null, null, null, null, null, FLAG_CHECKER_THREW];
    }
  }

  /** The row's symbol, following aliases to a target that has a declaration. */
  private symbolOf(kind: PositionKind, node: ts.Node): ts.Symbol | undefined {
    let symbol: ts.Symbol | undefined;
    if (kind === 'ident' && ts.isShorthandPropertyAssignment(node.parent) && node.parent.name === node) {
      symbol = this.checker.getShorthandAssignmentValueSymbol(node.parent);
    } else {
      const named = nameNodeOf(kind, node);
      symbol = named ? this.checker.getSymbolAtLocation(named) : undefined;
    }
    if (symbol && symbol.flags & ts.SymbolFlags.Alias) {
      const target = this.checker.getAliasedSymbol(symbol);
      // An import of a module that does not resolve aliases nothing: the
      // import itself is all there is.
      if (target.declarations && target.declarations.length > 0) symbol = target;
    }
    return symbol;
  }
}

/** The kind of row a node gives, if any (exported for the editor-parity test, carrick#2418). */
export function kindOf(node: ts.Node): PositionKind | undefined {
  if (ts.isCallExpression(node) || ts.isNewExpression(node) || ts.isTaggedTemplateExpression(node)) {
    return 'call';
  }
  if (ts.isPropertyAccessExpression(node) || ts.isElementAccessExpression(node)) return 'member';
  if (ts.isBinaryExpression(node) && COMPARE_OPERATORS.has(node.operatorToken.kind)) return 'compare';
  if (ts.isIdentifier(node) && isExpressionIdentifier(node)) return 'ident';
  return undefined;
}

/** The node whose symbol a row of `kind` names. */
function nameNodeOf(kind: PositionKind, node: ts.Node): ts.Node | undefined {
  switch (kind) {
    case 'call':
      return referenceName(
        ts.isTaggedTemplateExpression(node) ? node.tag : (node as ts.CallExpression | ts.NewExpression).expression
      );
    case 'member':
      return ts.isPropertyAccessExpression(node)
        ? node.name
        : (node as ts.ElementAccessExpression).argumentExpression;
    case 'compare':
      return referenceName((node as ts.BinaryExpression).left);
    case 'ident':
      return node;
  }
}

/**
 * The name an expression refers by, through the wrappers that do not change
 * what it refers to (`(x)`, `x as T`, `<T>x`, `x!`, `x satisfies T`): an
 * identifier, a member's name or argument, `super`. `undefined` for anything
 * else (a call's result, a literal).
 */
function referenceName(expression: ts.Expression): ts.Node | undefined {
  let inner: ts.Expression = expression;
  while (
    ts.isParenthesizedExpression(inner) ||
    ts.isAsExpression(inner) ||
    ts.isTypeAssertionExpression(inner) ||
    ts.isNonNullExpression(inner) ||
    ts.isSatisfiesExpression(inner)
  ) {
    inner = inner.expression;
  }
  if (ts.isIdentifier(inner) || ts.isPrivateIdentifier(inner)) return inner;
  if (ts.isPropertyAccessExpression(inner)) return inner.name;
  if (ts.isElementAccessExpression(inner)) return inner.argumentExpression;
  if (inner.kind === ts.SyntaxKind.SuperKeyword) return inner;
  return undefined;
}

/**
 * Whether an identifier is in expression position: a reference to a value,
 * including a callee, a member's object and a shorthand property `{ a }`.
 * Not a declaration's or binding's name, a type-position name, an import or
 * export specifier, a property key, a member's name, a label, a meta-property
 * (`import.meta`), or a JSX tag or attribute name.
 */
function isExpressionIdentifier(node: ts.Identifier): boolean {
  const parent = node.parent;
  if (ts.isShorthandPropertyAssignment(parent) && parent.name === node) return true;
  if (ts.isPartOfTypeNode(node)) return false;
  const named = parent as {
    name?: ts.Node;
    propertyName?: ts.Node;
    label?: ts.Node;
    tagName?: ts.Node;
  };
  if (named.name === node || named.propertyName === node || named.label === node) return false;
  if (named.tagName === node) return false;
  if (ts.isQualifiedName(parent) || ts.isMetaProperty(parent)) return false;
  // The tag of a JSX element written as a member (`<ui.Button>`).
  if (ts.isPropertyAccessExpression(parent) && isJsxTagName(parent)) return false;
  return true;
}

function isJsxTagName(node: ts.Node): boolean {
  let tag: ts.Node = node;
  while (ts.isPropertyAccessExpression(tag.parent) && tag.parent.expression === tag) tag = tag.parent;
  const holder = tag.parent as { tagName?: ts.Node };
  return holder.tagName === tag;
}
