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
  entryLabel,
} from "./manifest-matcher";

/**
 * Trailing marker the Rust scanner (`append_missing_aliases` in
 * `src/engine/mod.rs`) stamps onto every `= unknown` alias it injects for a
 * manifest entry missing from the bundle. ts_check uses it to recognise the
 * injected placeholder without misclassifying a developer-authored
 * `type X = unknown` as one (#244). Must stay byte-identical to the Rust
 * `MISSING_ALIAS_MARKER` constant.
 */
const MISSING_ALIAS_MARKER = "// carrick:missing-alias";

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
          `${entryLabel(o.entry)} (${o.entry.type_kind}, ${o.entry.type_alias})`
      ),
      orphanedConsumers: orphanedConsumers.map(
        (o) =>
          `${entryLabel(o.entry)} (${o.entry.type_kind}, ${o.entry.type_alias})`
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

    // Assignability runs in the DATA-flow direction: the value that is sent
    // must satisfy the type the receiver expects.
    //
    // HTTP responses: the manifest producer (endpoint) emits the response and the
    // manifest consumer (call) receives it, so the producer payload must satisfy
    // the consumer — `producer ⊑ consumer`. HTTP REQUEST bodies invert this: the
    // consumer (caller) sends the body the producer (endpoint) must accept, so
    // `consumer ⊑ producer` (keyed on `type_kind` via `httpRequest` below).
    //
    // GraphQL: same data-flow direction as HTTP. The producer (schema resolver,
    // server) RETURNS the full object; the consumer (document, client) SELECTS a
    // subset of its fields, so the producer payload must satisfy what the
    // consumer reads — `producer ⊑ consumer`. (NOT the socket inversion: GraphQL
    // is request/response server→client like HTTP, and a consumer that selects
    // fewer fields than the producer provides is compatible — `OrderView`
    // dropping the producer's `status` field is fine, whereas the socket
    // direction would wrongly read it as incompatible because `Order` requires
    // `status`.) The producer type is the SDL field PAYLOAD, structurally
    // unwrapped from the resolver's return ENVELOPE below.
    //
    // Socket: the role mapping is inverted relative to data flow. Carrick keys a
    // *listener* (`socket.on`) as the producer (endpoint) and an *emitter*
    // (`socket.emit`) as the consumer (call). The bytes flow emitter → listener,
    // so the *emitter's* payload (manifest consumer) must satisfy what the
    // *listener* expects (manifest producer) — `consumer ⊑ producer`. Checking
    // the HTTP direction here would read a widening listener type (e.g.
    // `status: string` accepting a producer `"pending" | "settled"`) as
    // incompatible, the opposite of the truth. (See xrepo-corpus-1 README,
    // "Socket producer/consumer direction".) The same `isAssignableTo` relation
    // and the same resolved Type objects are used either way.
    //
    // Pub/Sub: shares the SAME inverted direction as socket. Carrick keys a
    // *subscriber* (the handler registration) as the producer (endpoint) and a
    // *publisher* (the send) as the consumer (call). The bytes flow publisher →
    // subscriber, so the *publisher's* payload (manifest consumer) must satisfy
    // what the *subscriber* accepts (manifest producer) — `consumer ⊑ producer`.
    // A subscriber declaring a WIDER accepted payload than the publisher sends is
    // compatible, identical to socket. Pub/sub payload types are already DECODED
    // application types (the LLM emits `Order`, not the wire `Buffer`/`string`),
    // so there is NO envelope/codec unwrap — pub/sub stays on the non-graphql
    // `producerComparand` branch below.
    const socket = producer.protocol === "socket";
    const graphql = producer.protocol === "graphql";
    const pubsub = producer.protocol === "pubsub";
    // HTTP request bodies flow consumer → producer (the caller sends the body the
    // endpoint must accept), the inverse of the HTTP response direction. The
    // matcher pairs strictly per `type_kind`, so producer and consumer agree here.
    const httpRequest =
      producer.protocol === "http" && producer.type_kind === "request";

    // For GraphQL the producer's resolved type is the resolver's return ENVELOPE
    // (e.g. `{ data: Order; errors: string[] }`), kept verbatim for the
    // resolution metric. Compat must compare against the SDL field PAYLOAD
    // (`Order`), so unwrap the envelope structurally — pick the single
    // object/array-of-objects property among otherwise scalar/`string[]`
    // siblings. Framework-agnostic: never matches the literal name `data`. If the
    // shape is not a single-payload envelope (already bare, or ambiguous), the
    // unwrap returns the type unchanged and logs a warning.
    const producerComparand = graphql
      ? this.unwrapGraphqlPayload(producerType, endpoint)
      : producerType;

    // Socket, pub/sub, AND HTTP request bodies share the inverted direction: the
    // consumer (emitter / publisher / caller) sends, the producer (listener /
    // subscriber / endpoint) accepts — `consumer ⊑ producer`. HTTP responses and
    // GraphQL keep the data-flow-forward `producer ⊑ consumer`. Pub/sub and HTTP
    // requests do NOT unwrap an envelope (only graphql does), so they ride the
    // non-graphql `producerComparand` above.
    const inverted = socket || pubsub || httpRequest;
    const sentType = inverted ? consumerType : producerComparand;
    const expectedType = inverted ? producerComparand : consumerType;

    // Re-run the `any`/`unknown` → unverifiable guard on the COMPARANDS, not just
    // the raw `producerType`/`consumerType` checked at the top. The GraphQL
    // envelope unwrap (`unwrapGraphqlPayload`) can select a payload property that
    // resolves to `any`/`unknown` even though the envelope object around it does
    // not — e.g. `{ data: <unresolved>; errors }` whose `data` member dangles to
    // `any`. Comparing a real producer against such a payload (or the inverse)
    // would hit `isAssignableTo` with a top-ish side and read compatible: the
    // exact `graphql|subscription|orderUpdated` false-positive. The raw-type
    // guard never sees the unwrapped payload, so the comparands must be guarded
    // again here, before `isAssignableTo`. (For HTTP/socket/pubsub the comparands
    // equal the raw types, so this is a redundant no-op that preserves their
    // behavior.)
    if (
      sentType.isAny() ||
      expectedType.isAny() ||
      sentType.isUnknown() ||
      expectedType.isUnknown()
    ) {
      const producerComparandIsTop = producerComparand.isAny() || producerComparand.isUnknown();
      const consumerComparandIsTop = consumerType.isAny() || consumerType.isUnknown();
      const reasonParts = [];
      if (producerComparandIsTop) {
        reasonParts.push(
          `producer payload resolves to ${producerComparand.isAny() ? "any" : "unknown"} (type missing from bundled types?)`
        );
      }
      if (consumerComparandIsTop) {
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

    if (sentType.isAssignableTo(expectedType)) {
      return { kind: "compatible" };
    }

    const sentText = this.typeText(sentType);
    const expectedText = this.typeText(expectedType);
    return {
      kind: "incompatible",
      mismatch: {
        endpoint,
        producerType: sentText,
        consumerCall: consumer.type_alias,
        consumerType: expectedText,
        isAssignable: false,
        errorDetails: `Type '${sentText}' is not assignable to type '${expectedText}'`,
        producerLocation: this.formatEntryLocation(producer),
        consumerLocation: this.formatEntryLocation(consumer),
        producerEvidence: producer.evidence,
        consumerEvidence: consumer.evidence,
      },
    };
  }

  /**
   * Unwrap a GraphQL resolver's return ENVELOPE to its SDL field PAYLOAD type.
   *
   * A GraphQL producer's resolved type is the resolver function's return type
   * (e.g. `Promise<ApiResponse<Order>>` → `{ data: Order; errors: string[] }`),
   * kept verbatim on the manifest entry for the resolution metric. For compat we
   * must compare the consumer against the SDL field's payload (`Order`), not the
   * transport envelope.
   *
   * The unwrap is STRUCTURAL and framework-agnostic — it never matches the
   * literal property name `data`/`errors` or a wrapper symbol like `ApiResponse`.
   * Among the envelope's own properties it keeps exactly those whose type is an
   * object or an array-of-objects (the candidate payload), discarding scalar and
   * scalar-array siblings (`errors: string[]`, status codes, flags). If exactly
   * one payload candidate remains, that property's type is the payload. In every
   * other case — the type is already a bare object that is not a single-payload
   * envelope, has no payload candidate, or has several (ambiguous) — the original
   * type is returned unchanged and a warning is logged, so an unrecognised
   * envelope shape degrades to comparing the whole type rather than guessing.
   */
  private unwrapGraphqlPayload(producerType: Type, endpoint: string): Type {
    // Only object-like types can be envelopes; a bare scalar/union is returned
    // as-is (the producer already resolved to its payload, e.g. SDL `String`).
    if (!producerType.isObject() || producerType.isArray()) {
      return producerType;
    }

    const props = producerType.getProperties();
    // A single-property wrapper whose property is itself the payload is the most
    // common envelope shape, but we classify generally rather than assume arity.
    const payloadCandidates: Type[] = [];
    for (const prop of props) {
      const decl = prop.getDeclarations()[0];
      if (!decl) {
        // No declaration to anchor the property type — can't classify, so this
        // isn't a clean single-payload envelope. Bail out to whole-type compare.
        return producerType;
      }
      const propType = prop.getTypeAtLocation(decl);
      if (this.isPayloadShape(propType)) {
        payloadCandidates.push(propType);
      }
    }

    if (payloadCandidates.length === 1) {
      return payloadCandidates[0];
    }

    // Zero candidates → the type may already BE the bare payload object (its own
    // properties are scalars, e.g. `{ id; total }`); comparing it whole is
    // correct, so this is the silent expected path, not a warning. More than one
    // candidate → a genuinely ambiguous envelope; warn and compare whole.
    if (payloadCandidates.length > 1) {
      console.warn(
        `[type-checker] GraphQL producer envelope for ${endpoint} has ${payloadCandidates.length} ` +
          `object-typed properties; cannot pick a single SDL payload, comparing the whole type`
      );
    }
    return producerType;
  }

  /**
   * Whether a property type looks like a GraphQL SDL payload: an object, or an
   * array whose element is an object (`Order` / `Order[]`). Scalars, unions of
   * scalars, and scalar arrays (`string[]`, `number[]`) are NOT payloads — they
   * are envelope metadata (`errors`, status flags). Kept deliberately structural
   * so no property NAME is ever consulted.
   */
  private isPayloadShape(t: Type): boolean {
    if (t.isArray()) {
      const element = t.getArrayElementType();
      return !!element && element.isObject() && !element.isArray();
    }
    return t.isObject();
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
        const resolvesToUnknown =
          typeNode?.getKind() === SyntaxKind.UnknownKeyword ||
          typeNode?.getText().trim() === "unknown";
        // Only the Carrick-injected `= unknown` placeholder carries the marker
        // comment. A developer-authored `type X = unknown` resolves to unknown
        // too, but it is a genuine (if uninformative) API type, not our
        // failed-inference stand-in (#244). The compiler-level isUnknown() gate
        // in compareTypes still surfaces a genuine `unknown` as unverifiable, so
        // dropping it here does not let a real shape mismatch read compatible.
        const isPlaceholderUnknown =
          resolvesToUnknown && this.hasMissingAliasMarker(typeAlias);
        return { type: typeAlias.getType(), isPlaceholderUnknown };
      }

      const iface = sourceFile.getInterface(typeName);
      if (iface) {
        return { type: iface.getType(), isPlaceholderUnknown: false };
      }
    }

    return { type: null, isPlaceholderUnknown: false };
  }

  /**
   * True when the type alias carries the Carrick `MISSING_ALIAS_MARKER` trailing
   * comment that marks it as an injected `= unknown` placeholder. ts-morph's
   * `getText()` omits trailing same-line comments, so this inspects the source
   * line that holds the alias rather than the node text.
   */
  private hasMissingAliasMarker(typeAlias: TypeAliasDeclaration): boolean {
    const sourceFile = typeAlias.getSourceFile();
    const fullText = sourceFile.getFullText();
    const declEnd = typeAlias.getEnd();
    const lineEnd = fullText.indexOf("\n", declEnd);
    const tail = fullText.slice(declEnd, lineEnd === -1 ? undefined : lineEnd);
    return tail.includes(MISSING_ALIAS_MARKER);
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
