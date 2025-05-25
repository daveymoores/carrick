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
  findTypeReferenceAtPosition,
  findTypeDeclarationByPosition,
  getModuleSpecifierFromNodeModulesPath,
} from "./type-resolver";
export { ImportHandler } from "./import-handler";
export { TypeProcessor } from "./type-processor";
export { DeclarationCollector } from "./declaration-collector";
export { DependencyManager } from "./dependency-manager";
export { OutputGenerator } from "./output-generator";
export { TypeExtractor } from "./type-extractor";
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
