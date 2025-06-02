export interface TypeInfo {
  filePath: string; // Path to the file where the type usage occurs
  startPosition: number; // UTF-16 offset of the main identifier in the composite type
  compositeTypeString: string; // The full text, e.g., "Response<User[]>"
  alias: string; // The generated alias name, e.g., "ResUsersGet"
}

export interface CompositeAliasInfo {
  aliasName: string;
  typeString: string;
}

export interface Dependencies {
  [key: string]: { name: string; version: string; source_path: string };
}

export interface PackageJsonContent {
  name: string;
  version: string;
  type: string;
  dependencies: Record<string, string>;
}

export interface ProcessingResult {
  success: boolean;
  output?: string;
  typeCount?: number;
  error?: string;
}