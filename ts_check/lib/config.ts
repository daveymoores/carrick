export interface ExtractorConfig {
  maxPropertyDepth: number;
  maxPropertiesLimit: number;
  maxRecursionDepth: number;
  excludedModuleSpecifiers: Set<string>;
  defaultOutputFile: string;
  defaultPackageName: string;
  defaultPackageVersion: string;
  defaultPackageType: string;
  enableLogging: boolean;
  enableDebugMode: boolean;
}

export const DEFAULT_CONFIG: ExtractorConfig = {
  maxPropertyDepth: 5,
  maxPropertiesLimit: 50,
  maxRecursionDepth: 10,
  excludedModuleSpecifiers: new Set(["typescript", "@types/node"]),
  defaultOutputFile: "out/all-types-recursive.ts",
  defaultPackageName: "extracted-types",
  defaultPackageVersion: "1.0.0",
  defaultPackageType: "module",
  enableLogging: true,
  enableDebugMode: false,
};

export class ConfigManager {
  private config: ExtractorConfig;

  constructor(overrides: Partial<ExtractorConfig> = {}) {
    this.config = { ...DEFAULT_CONFIG, ...overrides };
  }

  get(): ExtractorConfig {
    return this.config;
  }

  update(overrides: Partial<ExtractorConfig>): void {
    this.config = { ...this.config, ...overrides };
  }

  isExcludedModule(moduleSpecifier: string): boolean {
    return this.config.excludedModuleSpecifiers.has(moduleSpecifier);
  }

  log(message: string, ...args: any[]): void {
    if (this.config.enableLogging) {
      console.log(message, ...args);
    }
  }

  debug(message: string, ...args: any[]): void {
    if (this.config.enableDebugMode) {
      console.debug(`[DEBUG] ${message}`, ...args);
    }
  }

  warn(message: string, ...args: any[]): void {
    if (this.config.enableLogging) {
      console.warn(message, ...args);
    }
  }

  error(message: string, ...args: any[]): void {
    console.error(message, ...args);
  }
}