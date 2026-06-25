/**
 * Type Compatibility Checker
 *
 * Provides manifest-based type checking between producer and consumer APIs.
 * Uses ts-morph for TypeScript type analysis and assignability checking.
 */

import {
  Project,
  SourceFile,
  InterfaceDeclaration,
  TypeAliasDeclaration,
  Type,
  SyntaxKind,
  ts,
} from "ts-morph";
import {
  ManifestMatcher,
  TypeManifest,
  ManifestEntry,
  MatchResult,
  TypeEvidence,
} from "./manifest-matcher";

// ============================================================================
// Type Definitions
// ============================================================================

/**
 * Information about a type mismatch between producer and consumer
 */
export interface TypeMismatch {
  endpoint: string;
  producerType: string;
  consumerCall: string;
  consumerType: string;
  isAssignable: boolean;
  errorDetails: string;
  producerLocation?: string;
  consumerLocation?: string;
  producerEvidence?: TypeEvidence;
  consumerEvidence?: TypeEvidence;
}

export interface UnknownTypePair {
  endpoint: string;
  reason: string;
  producerTypeAlias: string;
  consumerTypeAlias: string;
  producerLocation?: string;
  consumerLocation?: string;
  producerEvidence?: TypeEvidence;
  consumerEvidence?: TypeEvidence;
}

/**
 * Result of type compatibility checking
 */
export interface TypeCheckResult {
  totalProducers: number;
  totalConsumers: number;
  compatiblePairs: number;
  incompatiblePairs: number;
  mismatches: TypeMismatch[];
  orphanedProducers: string[];
  orphanedConsumers: string[];
  unknownPairs: UnknownTypePair[];
  /** Match details from manifest matching */
  matchDetails?: MatchResult[];
}

// ============================================================================
// TypeCompatibilityChecker Class
// ============================================================================

/**
 * TypeCompatibilityChecker - Checks type compatibility between producer and consumer APIs
 *
 * Uses manifest-based matching to find corresponding endpoints and ts-morph
 * for TypeScript type assignability checking.
 *
 * @example
 * ```typescript
 * const project = new Project({ tsConfigFilePath: './tsconfig.json' });
 * const checker = new TypeCompatibilityChecker(project);
 *
 * const producerManifest = checker.loadManifest('./producer-manifest.json');
 * const consumerManifest = checker.loadManifest('./consumer-manifest.json');
 *
 * const result = await checker.checkCompatibility(producerManifest, consumerManifest);
 * console.log(`Compatible: ${result.compatiblePairs}, Incompatible: ${result.incompatiblePairs}`);
 * ```
 */
export class TypeCompatibilityChecker {
  private manifestMatcher: ManifestMatcher;

  constructor(private project: Project) {
    this.manifestMatcher = new ManifestMatcher();
  }

  /**
   * Check type compatibility using manifest files
   *
   * This method:
   * 1. Uses ManifestMatcher to find endpoint matches
   * 2. Loads corresponding type aliases from the project
   * 3. Uses ts-morph's type assignability checking
   *
   * @param producerManifest - Manifest containing producer types
   * @param consumerManifest - Manifest containing consumer types
   * @param typesProject - Optional ts-morph Project for the bundled types
   * @returns TypeCheckResult with compatibility information
   */
  async checkCompatibility(
    producerManifest: TypeManifest,
    consumerManifest: TypeManifest,
    typesProject?: Project
  ): Promise<TypeCheckResult> {
    console.log(`[type-checker] Starting manifest-based type checking`);
    console.log(
      `[type-checker] Producer repo: ${producerManifest.repo_name} (${producerManifest.entries.length} entries)`
    );
    console.log(
      `[type-checker] Consumer repo: ${consumerManifest.repo_name} (${consumerManifest.entries.length} entries)`
    );

    // Use provided project or fall back to instance project
    const project = typesProject || this.project;

    // Match endpoints using the manifest matcher
    const { matches, orphanedProducers, orphanedConsumers } =
      this.manifestMatcher.matchEndpoints(producerManifest, consumerManifest);

    console.log(`[type-checker] Found ${matches.length} endpoint matches`);
    console.log(
      `[type-checker] Orphaned producers: ${orphanedProducers.length}`
    );
    console.log(
      `[type-checker] Orphaned consumers: ${orphanedConsumers.length}`
    );

    const result: TypeCheckResult = {
      totalProducers: producerManifest.entries.filter(
        (e) => e.role === "producer"
      ).length,
      totalConsumers: consumerManifest.entries.filter(
        (e) => e.role === "consumer"
      ).length,
      compatiblePairs: 0,
      incompatiblePairs: 0,
      mismatches: [],
      orphanedProducers: orphanedProducers.map(
        (o) =>
          `${o.entry.method} ${o.entry.path} (${o.entry.type_kind}, ${o.entry.type_alias})`
      ),
      orphanedConsumers: orphanedConsumers.map(
        (o) =>
          `${o.entry.method} ${o.entry.path} (${o.entry.type_kind}, ${o.entry.type_alias})`
      ),
      unknownPairs: [],
      matchDetails: matches,
    };

    // For each match, compare the types
    for (const match of matches) {
      const endpoint = `${match.method} ${match.path} (${match.type_kind})`;
      const producerUnknown =
        match.producer.type_state === "unknown" ||
        match.producer.evidence?.type_state === "unknown";
      const consumerUnknown =
        match.consumer.type_state === "unknown" ||
        match.consumer.evidence?.type_state === "unknown";
      const producerTypeInfo = producerUnknown
        ? this.resolveTypeInfo(project, match.producer.type_alias)
        : undefined;
      const consumerTypeInfo = consumerUnknown
        ? this.resolveTypeInfo(project, match.consumer.type_alias)
        : undefined;
      const producerIsUnknown =
        producerUnknown &&
        (!producerTypeInfo?.type || producerTypeInfo.isPlaceholderUnknown);
      const consumerIsUnknown =
        consumerUnknown &&
        (!consumerTypeInfo?.type || consumerTypeInfo.isPlaceholderUnknown);

      if (producerIsUnknown || consumerIsUnknown) {
        const reasonParts = [];
        if (producerIsUnknown) {
          reasonParts.push("producer type_state=unknown");
        }
        if (consumerIsUnknown) {
          reasonParts.push("consumer type_state=unknown");
        }
        result.unknownPairs.push({
          endpoint,
          reason: reasonParts.join(", "),
          producerTypeAlias: match.producer.type_alias,
          consumerTypeAlias: match.consumer.type_alias,
          producerLocation: this.formatEntryLocation(match.producer),
          consumerLocation: this.formatEntryLocation(match.consumer),
          producerEvidence: match.producer.evidence,
          consumerEvidence: match.consumer.evidence,
        });
        continue;
      }

      try {
        const outcome = this.compareTypes(
          endpoint,
          match.producer,
          match.consumer,
          producerTypeInfo?.type ?? this.findTypeInProject(project, match.producer.type_alias),
          consumerTypeInfo?.type ?? this.findTypeInProject(project, match.consumer.type_alias)
        );

        if (outcome.kind === "incompatible") {
          result.mismatches.push(outcome.mismatch);
          result.incompatiblePairs++;
        } else if (outcome.kind === "unverifiable") {
          result.unknownPairs.push(outcome.unknown);
        } else {
          result.compatiblePairs++;
        }
      } catch (error) {
        console.error(
          `[type-checker] Error comparing types for ${endpoint}:`,
          error
        );
        result.mismatches.push({
          endpoint,
          producerType: match.producer.type_alias,
          consumerCall: match.consumer.type_alias,
          consumerType: "UNKNOWN",
          isAssignable: false,
          errorDetails: `Failed to compare types: ${error instanceof Error ? error.message : String(error)}`,
          producerLocation: this.formatEntryLocation(match.producer),
          consumerLocation: this.formatEntryLocation(match.consumer),
          producerEvidence: match.producer.evidence,
          consumerEvidence: match.consumer.evidence,
        });
        result.incompatiblePairs++;
      }
    }

    console.log(`[type-checker] Type checking complete`);
    console.log(
      `[type-checker] Compatible: ${result.compatiblePairs}, Incompatible: ${result.incompatiblePairs}`
    );

    return result;
  }

  /**
   * Compare resolved producer/consumer types.
   *
   * The verdict comes from the compiler's own assignability relation on the
   * Type objects (same checker that resolved them). Re-serializing the types
   * into a probe file is deliberately avoided: the printed text references
   * `import("...")` specifiers that may not resolve in the probe's project,
   * which degraded every such check to a silent "compatible".
   *
   * Aliases that resolve to `any` are unverifiable, not compatible: `any` is
   * assignable to everything, and in bundled .d.ts surfaces it almost always
   * means a broken import rather than an intentional payload type.
   */
  private compareTypes(
    endpoint: string,
    producer: ManifestEntry,
    consumer: ManifestEntry,
    producerType: Type | null | undefined,
    consumerType: Type | null | undefined
  ):
    | { kind: "compatible" }
    | { kind: "incompatible"; mismatch: TypeMismatch }
    | { kind: "unverifiable"; unknown: UnknownTypePair } {
    const notFound = !producerType
      ? `Producer type '${producer.type_alias}' not found in project`
      : !consumerType
        ? `Consumer type '${consumer.type_alias}' not found in project`
        : undefined;
    if (notFound || !producerType || !consumerType) {
      return {
        kind: "incompatible",
        mismatch: {
          endpoint,
          producerType: producer.type_alias,
          consumerCall: consumer.type_alias,
          consumerType: consumer.type_alias,
          isAssignable: false,
          errorDetails: notFound ?? "Types are not compatible",
          producerLocation: this.formatEntryLocation(producer),
          consumerLocation: this.formatEntryLocation(consumer),
          producerEvidence: producer.evidence,
          consumerEvidence: consumer.evidence,
        },
      };
    }

    // `any` and `unknown` are both top-ish: everything is assignable to
    // `unknown`, so a side that resolves to either makes `isAssignableTo`
    // return true and the edge would read compatible without ever comparing
    // the real shapes. These almost always mean the type never reached the
    // bundle (a broken import, or an `= unknown` placeholder from
    // append_missing_aliases). Classify the edge unverifiable, not compatible.
    if (
      producerType.isAny() ||
      consumerType.isAny() ||
      producerType.isUnknown() ||
      consumerType.isUnknown()
    ) {
      const reasonParts = [];
      if (producerType.isAny() || producerType.isUnknown()) {
        reasonParts.push(
          `producer type resolves to ${producerType.isAny() ? "any" : "unknown"} (type missing from bundled types?)`
        );
      }
      if (consumerType.isAny() || consumerType.isUnknown()) {
        reasonParts.push(
          `consumer type resolves to ${consumerType.isAny() ? "any" : "unknown"} (type missing from bundled types?)`
        );
      }
      return {
        kind: "unverifiable",
        unknown: {
          endpoint,
          reason: reasonParts.join(", "),
          producerTypeAlias: producer.type_alias,
          consumerTypeAlias: consumer.type_alias,
          producerLocation: this.formatEntryLocation(producer),
          consumerLocation: this.formatEntryLocation(consumer),
          producerEvidence: producer.evidence,
          consumerEvidence: consumer.evidence,
        },
      };
    }

    // Producer payload must satisfy what the consumer expects.
    if (producerType.isAssignableTo(consumerType)) {
      return { kind: "compatible" };
    }

    const producerText = this.typeText(producerType);
    const consumerText = this.typeText(consumerType);
    return {
      kind: "incompatible",
      mismatch: {
        endpoint,
        producerType: producerText,
        consumerCall: consumer.type_alias,
        consumerType: consumerText,
        isAssignable: false,
        errorDetails: `Type '${producerText}' is not assignable to type '${consumerText}'`,
        producerLocation: this.formatEntryLocation(producer),
        consumerLocation: this.formatEntryLocation(consumer),
        producerEvidence: producer.evidence,
        consumerEvidence: consumer.evidence,
      },
    };
  }

  /**
   * Print a type without the compiler's ~160-char display truncation, which
   * inserts `...` into wide payload types and makes reports useless.
   */
  private typeText(type: Type): string {
    return type.getText(
      undefined,
      ts.TypeFormatFlags.NoTruncation | ts.TypeFormatFlags.InTypeAlias
    );
  }

  /**
   * Find a type alias or interface in the project by name
   *
   * @param project - The ts-morph project to search
   * @param typeName - The name of the type to find
   * @returns The Type if found, null otherwise
   */
  private findTypeInProject(project: Project, typeName: string): Type | null {
    for (const sourceFile of project.getSourceFiles()) {
      // Check type aliases
      const typeAlias = sourceFile.getTypeAlias(typeName);
      if (typeAlias) {
        return typeAlias.getType();
      }

      // Check interfaces
      const iface = sourceFile.getInterface(typeName);
      if (iface) {
        return iface.getType();
      }
    }

    return null;
  }

  private resolveTypeInfo(
    project: Project,
    typeName: string
  ): { type: Type | null; isPlaceholderUnknown: boolean } {
    for (const sourceFile of project.getSourceFiles()) {
      const typeAlias = sourceFile.getTypeAlias(typeName);
      if (typeAlias) {
        const typeNode = typeAlias.getTypeNode();
        const typeText = typeNode?.getText().trim();
        const isPlaceholderUnknown =
          typeNode?.getKind() === SyntaxKind.UnknownKeyword ||
          typeText === "unknown";
        return { type: typeAlias.getType(), isPlaceholderUnknown };
      }

      const iface = sourceFile.getInterface(typeName);
      if (iface) {
        return { type: iface.getType(), isPlaceholderUnknown: false };
      }
    }

    return { type: null, isPlaceholderUnknown: false };
  }

  private formatEntryLocation(entry: ManifestEntry): string {
    const filePath = entry.evidence?.file_path || entry.file_path;
    const lineNumber =
      typeof entry.evidence?.line_number === "number"
        ? entry.evidence.line_number
        : entry.line_number;
    return `${filePath}:${lineNumber}`;
  }

  /**
   * Load a manifest from a file path
   *
   * @param filePath - Path to the manifest JSON file
   * @returns The parsed TypeManifest
   */
  loadManifest(filePath: string): TypeManifest {
    return this.manifestMatcher.loadManifest(filePath);
  }

  /**
   * Parse a manifest from a JSON string
   *
   * @param jsonContent - The JSON string to parse
   * @returns The parsed TypeManifest
   */
  parseManifest(jsonContent: string): TypeManifest {
    return this.manifestMatcher.parseManifest(jsonContent);
  }

  /**
   * Create an empty manifest
   *
   * @param repoName - Name of the repository
   * @param commitHash - Git commit hash
   * @returns An empty TypeManifest
   */
  createEmptyManifest(repoName: string, commitHash: string): TypeManifest {
    return this.manifestMatcher.createEmptyManifest(repoName, commitHash);
  }

  /**
   * Get the underlying ManifestMatcher instance
   *
   * Useful for advanced operations like finding specific endpoints
   * or building custom matching logic.
   */
  getMatcher(): ManifestMatcher {
    return this.manifestMatcher;
  }
}
