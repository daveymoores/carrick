import { TypeInfo, Dependencies } from "./types";
import { DEFAULT_OUTPUT_FILE } from "./constants";

export interface ParsedArguments {
  inputTypesJson: string;
  outputFile: string;
  tsconfigPath?: string;
  dependenciesJson?: string;
  typeInfos: TypeInfo[];
  allDependencies: Dependencies;
}

export function parseArguments(): ParsedArguments {
  const [
    ,
    ,
    inputTypesJson,
    outputFile = DEFAULT_OUTPUT_FILE,
    tsconfigPath,
    dependenciesJson,
  ] = process.argv;

  if (!inputTypesJson) {
    console.error(
      'Usage: ts-node extract-type-definitions.ts \'[{"filePath":"path/to/file.ts","typeName":"TypeName","startPosition":123},...]\' [outputFile] [tsconfigPath] [dependencies]',
    );
    process.exit(1);
  }

  let typeInfos: TypeInfo[] = [];
  let allDependencies: Dependencies = {};

  // Parse dependencies
  try {
    if (dependenciesJson) {
      allDependencies = JSON.parse(dependenciesJson);
    }
  } catch (error) {
    console.error("Error parsing dependencies JSON:", error);
  }

  // Parse type information
  try {
    typeInfos = JSON.parse(inputTypesJson);
  } catch (error) {
    console.error("Error parsing input types JSON:", error);
    process.exit(1);
  }

  if (typeInfos.length === 0) {
    console.error("No type information provided");
    process.exit(1);
  }

  return {
    inputTypesJson,
    outputFile,
    tsconfigPath,
    dependenciesJson,
    typeInfos,
    allDependencies,
  };
}
