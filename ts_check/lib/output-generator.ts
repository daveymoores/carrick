import { Project, SourceFile, Node } from "ts-morph";
import { CompositeAliasInfo, ProcessingResult, TypeInfo } from "./types";

export class OutputGenerator {
  generateOutput(
    outputFile: string,
    collectedDeclarations: Set<Node>,
    compositeAliases: Set<CompositeAliasInfo>,
    importStatements: string,
    typeInfos: TypeInfo[],
  ): ProcessingResult {
    try {
      const outputProject = new Project();
      const newSourceFile = outputProject.createSourceFile(outputFile, "", {
        overwrite: true,
      });

      // Sort declarations for better readability
      const sortedDeclarations = this.sortDeclarations(
        collectedDeclarations,
        typeInfos,
      );

      // Add declarations to the file
      for (const decl of sortedDeclarations) {
        newSourceFile.addStatements(decl.getText());
      }

      // Add import statements at the beginning
      if (importStatements) {
        newSourceFile.insertText(0, importStatements);
      }

      // TEMPORARY: Deduplicate composite aliases by (aliasName, typeString) pair.
      // TODO: Address root cause upstream so this is not needed.
      const seenAliasType = new Set<string>();
      const dedupedCompositeAliases = Array.from(compositeAliases)
        .filter(({ aliasName, typeString }) => {
          const key = `${aliasName}|${typeString}`;
          if (seenAliasType.has(key)) return false;
          seenAliasType.add(key);
          return true;
        })
        .sort((a, b) => a.aliasName.localeCompare(b.aliasName));

      for (const { aliasName, typeString } of dedupedCompositeAliases) {
        newSourceFile.addStatements(
          `export type ${aliasName} = ${typeString};\n`,
        );
      }

      // Format and save
      newSourceFile.formatText();
      newSourceFile.saveSync();

      const totalTypeCount =
        collectedDeclarations.size + dedupedCompositeAliases.length;

      console.log(
        `All recursively acquired type/interface/enum/class declarations written to ${outputFile}`,
      );

      console.log(
        JSON.stringify({
          success: true,
          output: outputFile,
          typeCount: totalTypeCount,
        }),
      );

      return {
        success: true,
        output: outputFile,
        typeCount: totalTypeCount,
      };
    } catch (e: any) {
      console.error("Error saving or formatting the output file:", e.message);

      const errorResult = {
        success: false,
        error: e.message,
      };

      console.log(JSON.stringify(errorResult));
      return errorResult;
    }
  }

  private sortDeclarations(
    collectedDeclarations: Set<Node>,
    typeInfos: TypeInfo[],
  ): Node[] {
    return Array.from(collectedDeclarations).sort((a, b) => {
      // Get the file path of the first file in typeInfos for prioritization
      const primaryFilePath = typeInfos.length > 0 ? typeInfos[0].filePath : "";

      const aIsFromPrimaryFile =
        a.getSourceFile().getFilePath() === primaryFilePath;
      const bIsFromPrimaryFile =
        b.getSourceFile().getFilePath() === primaryFilePath;

      if (aIsFromPrimaryFile && !bIsFromPrimaryFile) return -1;
      if (!aIsFromPrimaryFile && bIsFromPrimaryFile) return 1;

      const pathA = a.getSourceFile().getFilePath();
      const pathB = b.getSourceFile().getFilePath();
      if (pathA !== pathB) {
        return pathA.localeCompare(pathB);
      }
      return a.getStart() - b.getStart();
    });
  }
}
