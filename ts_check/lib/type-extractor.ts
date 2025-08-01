import { TypeInfo } from "./types";
import { ProjectManager } from "./project-utils";
import { findTypeReferenceAtPosition } from "./type-resolver";
import { ImportHandler } from "./import-handler";
import { DeclarationCollector } from "./declaration-collector";
import { DependencyManager } from "./dependency-manager";
import { OutputGenerator } from "./output-generator";

export class TypeExtractor {
  private projectManager: ProjectManager;
  private importHandler: ImportHandler;
  private declarationCollector: DeclarationCollector;
  private dependencyManager: DependencyManager;
  private outputGenerator: OutputGenerator;

  constructor(
    tsconfigPath?: string,
    allDependencies: Record<
      string,
      { name: string; version: string; source_path: string }
    > = {},
  ) {
    this.projectManager = new ProjectManager(tsconfigPath);
    this.importHandler = new ImportHandler();
    this.declarationCollector = new DeclarationCollector(this.importHandler);
    this.dependencyManager = new DependencyManager(allDependencies);
    this.outputGenerator = new OutputGenerator();
  }

  async extractTypes(typeInfos: TypeInfo[], outputFile: string) {
    console.log(`Processing ${typeInfos.length} types from input`);

    // Process each type info
    for (const typeInfo of typeInfos) {
      const { filePath, startPosition, compositeTypeString, alias } = typeInfo;

      const sourceFile = this.projectManager.getSourceFile(filePath);
      if (!sourceFile) {
        console.error(`Input file '${filePath}' could not be found or loaded.`);
        continue;
      }

      const typeRefNode = findTypeReferenceAtPosition(
        sourceFile,
        startPosition,
      );

      if (!typeRefNode) {
        console.error(
          `Type not found in '${sourceFile.getFilePath()}' at position ${startPosition}`,
        );
        console.error(
          `Looking for type: ${compositeTypeString} with alias: ${alias}`,
        );
        console.error(`Source file content around position ${startPosition}:`);
        const content = sourceFile.getFullText();
        const start = Math.max(0, startPosition - 50);
        const end = Math.min(content.length, startPosition + 50);
        console.error(`"${content.slice(start, end)}"`);
        continue;
      }

      console.log(
        `Found type reference for ${compositeTypeString} at position ${startPosition}`,
      );
      console.log(`Type reference text: "${typeRefNode.getText()}"`);
      console.log(`Type reference kind: ${typeRefNode.getKindName()}`);

      console.log(
        `Found type reference at ${startPosition} in ${filePath}: ${typeRefNode.getText()}`,
      );

      // Process the type reference
      this.declarationCollector.processTypeReference(typeRefNode);

      if (compositeTypeString && alias) {
        this.declarationCollector.addCompositeAlias({
          aliasName: alias,
          typeString: compositeTypeString,
        });
        console.log(
          `Queued composite alias: export type ${alias} = ${compositeTypeString};`,
        );
      }
    }

    // Collect utility types
    this.declarationCollector.collectUtilityTypesWithInnerTypes();
    console.log(
      `After utility collection: ${this.declarationCollector.getCollectedDeclarations().size} declarations.`,
    );

    // Extract and manage dependencies
    this.dependencyManager.extractUsedDependencies(
      this.importHandler.getExternalTypeImports(),
    );

    // Write package.json
    this.dependencyManager.writePackageJson(outputFile);

    // Generate import statements
    const importStatements = this.importHandler.generateImportStatements(
      this.dependencyManager.getUsedDependencies(),
    );

    // Generate output
    const result = this.outputGenerator.generateOutput(
      outputFile,
      this.declarationCollector.getCollectedDeclarations(),
      this.declarationCollector.getCompositeAliasesToGenerate(),
      importStatements,
      typeInfos,
    );

    return result;
  }

  getProject() {
    return this.projectManager.getProject();
  }

  clear(): void {
    this.projectManager.clearCache();
    this.importHandler.clear();
    this.declarationCollector.clear();
    this.dependencyManager.clear();
  }
}
