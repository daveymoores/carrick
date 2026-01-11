// Active type checking exports (used by sidecar architecture)
export {
  TypeInfo,
  CompositeAliasInfo,
  Dependencies,
  PackageJsonContent,
  ProcessingResult,
} from "./types";
export { parseArguments, ParsedArguments } from "./argument-parser";
export { ProjectManager, isLocalType } from "./project-utils";
export {
  TypeCompatibilityChecker,
  TypeCheckResult,
  TypeMismatch,
} from "./type-checker";
export {
  ManifestMatcher,
  TypeManifest,
  ManifestEntry,
  MatchResult,
  OrphanedEntry,
  normalizePath,
  normalizeMethod,
  createManifestEntry,
  mergeManifests,
  defaultMatcher,
} from "./manifest-matcher";
export {
  MAX_PROPERTY_DEPTH,
  MAX_PROPERTIES_LIMIT,
  MAX_RECURSION_DEPTH,
  EXCLUDED_MODULE_SPECIFIERS,
  DEFAULT_OUTPUT_FILE,
  DEFAULT_PACKAGE_NAME,
  DEFAULT_PACKAGE_VERSION,
  DEFAULT_PACKAGE_TYPE,
  isNodeModulesPath,
  extractPackageName,
  getTypesPackageName,
  getMainPackageFromTypes,
} from "./constants";
export { ExtractorConfig, DEFAULT_CONFIG, ConfigManager } from "./config";
export {
  ExtractorError,
  FileNotFoundError,
  TypeNotFoundError,
  ParseError,
  OutputError,
  ErrorHandler,
} from "./error-handler";
export {
  LogLevel,
  LoggerConfig,
  Logger,
  createLogger,
  defaultLogger,
} from "./logger";

// Legacy exports (archived in _legacy/ directory)
// These are kept for backward compatibility but are deprecated.
// Type extraction is now handled by the TypeSidecar (src/sidecar).
export {
  findTypeReferenceAtPosition,
  findTypeDeclarationByPosition,
  getModuleSpecifierFromNodeModulesPath,
} from "./_legacy/type-resolver";
export { ImportHandler } from "./_legacy/import-handler";
export { TypeProcessor } from "./_legacy/type-processor";
export { DeclarationCollector } from "./_legacy/declaration-collector";
export { DependencyManager } from "./_legacy/dependency-manager";
export { OutputGenerator } from "./_legacy/output-generator";
export { TypeExtractor } from "./_legacy/type-extractor";
