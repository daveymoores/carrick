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

      if (producerUnknown || consumerUnknown) {
        const reasonParts = [];
        if (producerUnknown) {
          reasonParts.push("producer type_state=unknown");
        }
        if (consumerUnknown) {
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
        const mismatch = await this.compareTypes(
          project,
          endpoint,
          match.producer,
          match.consumer
        );

        if (mismatch) {
          result.mismatches.push(mismatch);
          result.incompatiblePairs++;
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
   * Compare types from manifest entries
   *
   * @param project - The ts-morph project containing bundled types
   * @param endpoint - The endpoint being checked
   * @param producer - The producer manifest entry
   * @param consumer - The consumer manifest entry
   * @returns TypeMismatch if incompatible, null if compatible
   */
  private async compareTypes(
    project: Project,
    endpoint: string,
    producer: ManifestEntry,
    consumer: ManifestEntry
  ): Promise<TypeMismatch | null> {
    // Try to find the type aliases in the project
    const producerType = this.findTypeInProject(project, producer.type_alias);
    const consumerType = this.findTypeInProject(project, consumer.type_alias);

    if (!producerType) {
      return {
        endpoint,
        producerType: producer.type_alias,
        consumerCall: consumer.type_alias,
        consumerType: consumer.type_alias,
        isAssignable: false,
        errorDetails: `Producer type '${producer.type_alias}' not found in project`,
        producerLocation: this.formatEntryLocation(producer),
        consumerLocation: this.formatEntryLocation(consumer),
        producerEvidence: producer.evidence,
        consumerEvidence: consumer.evidence,
      };
    }

    if (!consumerType) {
      return {
        endpoint,
        producerType: producer.type_alias,
        consumerCall: consumer.type_alias,
        consumerType: consumer.type_alias,
        isAssignable: false,
        errorDetails: `Consumer type '${consumer.type_alias}' not found in project`,
        producerLocation: this.formatEntryLocation(producer),
        consumerLocation: this.formatEntryLocation(consumer),
        producerEvidence: producer.evidence,
        consumerEvidence: consumer.evidence,
      };
    }

    // Check assignability: consumer type should be assignable from producer type
    // This means the producer should provide at least what the consumer expects
    const diagnosticMessage = this.getTypeCompatibilityError(
      producerType,
      consumerType
    );

    const isAssignable = !diagnosticMessage;

    if (!isAssignable) {
      return {
        endpoint,
        producerType: producerType.getText(),
        consumerCall: consumer.type_alias,
        consumerType: consumerType.getText(),
        isAssignable: false,
        errorDetails: diagnosticMessage || "Types are not compatible",
        producerLocation: this.formatEntryLocation(producer),
        consumerLocation: this.formatEntryLocation(consumer),
        producerEvidence: producer.evidence,
        consumerEvidence: consumer.evidence,
      };
    }

    return null;
  }

  /**
   * Get a human-readable error message if types are not compatible
   *
   * @param producerType - The producer's type
   * @param consumerType - The consumer's expected type
   * @returns Error message if incompatible, undefined if compatible
   */
  private getTypeCompatibilityError(
    producerType: Type,
    consumerType: Type
  ): string | undefined {
    // Create a temporary file to check type assignability
    const testCode = `
      type Producer = ${producerType.getText()};
      type Consumer = ${consumerType.getText()};
      declare const producer: Producer;
      const consumer: Consumer = producer;
    `;

    const tempFile = this.project.createSourceFile(
      `__type_check_${Date.now()}.ts`,
      testCode,
      { overwrite: true }
    );

    try {
      const diagnostics = tempFile.getPreEmitDiagnostics();

      // Find assignment error
      const assignmentError = diagnostics.find((d) => {
        const message = d.getMessageText();
        const messageStr =
          typeof message === "string" ? message : message.getMessageText();
        return (
          messageStr.includes("not assignable") ||
          messageStr.includes("is missing")
        );
      });

      if (assignmentError) {
        const message = assignmentError.getMessageText();
        return typeof message === "string" ? message : message.getMessageText();
      }

      return undefined;
    } finally {
      // Clean up temporary file
      this.project.removeSourceFile(tempFile);
    }
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
