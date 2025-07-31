import { Node } from "ts-morph";
import { CompositeAliasInfo } from "./types";
import { ImportHandler } from "./import-handler";
import { TypeProcessor } from "./type-processor";

export class DeclarationCollector {
  private collectedDeclarations = new Set<Node>();
  private seenNodesForRecursion = new Set<Node>();
  private compositeAliasesToGenerate = new Set<CompositeAliasInfo>();
  private typeProcessor: TypeProcessor;

  constructor(private importHandler: ImportHandler) {
    this.typeProcessor = new TypeProcessor((node, isRoot) =>
      this.collectDeclarationsRecursively(node, isRoot),
    );
  }

  getCollectedDeclarations(): Set<Node> {
    return this.collectedDeclarations;
  }

  getCompositeAliasesToGenerate(): Set<CompositeAliasInfo> {
    return this.compositeAliasesToGenerate;
  }

  addCompositeAlias(alias: CompositeAliasInfo): void {
    this.compositeAliasesToGenerate.add(alias);
  }

  processTypeReference(typeRef: Node): void {
    if (!Node.isTypeReference(typeRef)) return;

    const typeName = typeRef.getTypeName().getText();

    // Debug User type specifically
    if (typeName === "User") {
      console.log(`[DEBUG] Processing User type reference`);
    }

    const symbol = typeRef.getTypeName().getSymbol();
    if (symbol) {
      if (typeName === "User") {
        console.log(
          `[DEBUG] User symbol found with ${symbol.getDeclarations().length} declarations`,
        );
      }
      for (const decl of symbol.getDeclarations()) {
        const declFilePath = decl.getSourceFile().getFilePath();
        const isNodeModule = declFilePath.includes("node_modules");
        if (typeName === "User") {
          console.log(
            `[DEBUG] User declaration: ${decl.getKindName()} in ${declFilePath} (isNodeModule: ${isNodeModule})`,
          );
        }
        if (isNodeModule) {
          this.importHandler.addImportForExternalType(decl);
        } else {
          this.collectDeclarationsRecursively(decl, true);
        }
      }
    } else if (typeName === "User") {
      console.log(`[DEBUG] No symbol found for User type reference`);
    }

    for (const typeArg of typeRef.getTypeArguments()) {
      this.typeProcessor.processTypeArgument(typeArg);
    }
  }

  collectDeclarationsRecursively(node: Node | undefined, isRoot = false): void {
    if (!node || this.seenNodesForRecursion.has(node)) {
      return;
    }
    this.seenNodesForRecursion.add(node);

    if (Node.isImportSpecifier(node)) {
      const aliasedSymbol = node.getSymbol()?.getAliasedSymbol();
      if (aliasedSymbol) {
        for (const aliasedDecl of aliasedSymbol.getDeclarations()) {
          this.collectDeclarationsRecursively(aliasedDecl, true);
        }
      }
      return;
    }

    const sourceFilePath = node.getSourceFile().getFilePath();

    if (sourceFilePath.includes("node_modules")) {
      this.importHandler.addImportForExternalType(node);
      return;
    }

    if (
      Node.isTypeAliasDeclaration(node) ||
      Node.isInterfaceDeclaration(node) ||
      Node.isEnumDeclaration(node) ||
      Node.isClassDeclaration(node)
    ) {
      const nodeName = node.getName?.();
      if (nodeName === "User") {
        console.log(
          `[DEBUG] Adding User interface to collected declarations from ${sourceFilePath}`,
        );
      }
      this.collectedDeclarations.add(node);

      // Process type parameters
      if (
        Node.isClassDeclaration(node) ||
        Node.isInterfaceDeclaration(node) ||
        Node.isTypeAliasDeclaration(node) ||
        Node.isFunctionDeclaration(node) ||
        Node.isMethodDeclaration(node)
      ) {
        if (typeof (node as any).getTypeParameters === "function") {
          const typeParams = (node as any).getTypeParameters() as Node[];
          if (Array.isArray(typeParams)) {
            typeParams.forEach((tpNode) => {
              const tp = tpNode as import("ts-morph").TypeParameterDeclaration;
              const constraint = tp.getConstraint();
              if (constraint) this.typeProcessor.processTypeNode(constraint);
              const defaultType = tp.getDefault();
              if (defaultType) this.typeProcessor.processTypeNode(defaultType);
            });
          }
        }
      }

      // Heritage Clauses (extends, implements)
      if (Node.isInterfaceDeclaration(node) || Node.isClassDeclaration(node)) {
        node.getHeritageClauses().forEach((hc) => {
          hc.getTypeNodes().forEach((typeNode) =>
            this.typeProcessor.processTypeNode(typeNode),
          );
        });
      }

      // Members (properties, methods - their types, parameter types, return types)
      if (Node.isInterfaceDeclaration(node) || Node.isClassDeclaration(node)) {
        node.getMembers().forEach((member) => {
          if (
            Node.isPropertySignature(member) ||
            Node.isPropertyDeclaration(member)
          ) {
            this.typeProcessor.processTypeNode(member.getTypeNode());
          } else if (
            Node.isMethodSignature(member) ||
            Node.isMethodDeclaration(member) ||
            Node.isConstructorDeclaration(member) ||
            Node.isGetAccessorDeclaration(member) ||
            Node.isSetAccessorDeclaration(member)
          ) {
            if (typeof (member as any).getParameters === "function") {
              (member as any)
                .getParameters()
                .forEach((param: any) =>
                  this.typeProcessor.processTypeNode(param.getTypeNode()),
                );
            }
            if (typeof (member as any).getReturnTypeNode === "function") {
              this.typeProcessor.processTypeNode(
                (member as any).getReturnTypeNode(),
              );
            }
          }
        });
      }

      if (Node.isTypeAliasDeclaration(node)) {
        this.typeProcessor.processTypeNode(node.getTypeNode());
      }

      this.typeProcessor.collectFromTypeObject(node.getType(), node);
    } else if (Node.isVariableDeclaration(node)) {
      this.typeProcessor.collectFromTypeObject(node.getType(), node);
    }
  }

  collectUtilityTypesWithInnerTypes(): void {
    const utilityTypeNames = new Set<string>();
    const declarationsToProcess = [...this.collectedDeclarations];

    for (let i = 0; i < declarationsToProcess.length; i++) {
      const decl = declarationsToProcess[i];

      const processTypeReferences = (node: Node): void => {
        if (Node.isTypeReference(node)) {
          const typeName = node.getTypeName().getText();
          const typeArgs = node.getTypeArguments();

          if (typeArgs.length > 0) {
            const sourceFile = node.getSourceFile();
            const utilityType = sourceFile
              .getTypeAliases()
              .find((ta) => ta.getName() === typeName);

            if (utilityType && !this.collectedDeclarations.has(utilityType)) {
              this.collectedDeclarations.add(utilityType);
              declarationsToProcess.push(utilityType);
              console.log(`Added utility type: ${typeName}`);
            }

            typeArgs.forEach((argNode) => {
              if (Node.isTypeReference(argNode)) {
                const argTypeName = argNode.getTypeName().getText();

                const argTypeDecl =
                  sourceFile
                    .getTypeAliases()
                    .find((ta) => ta.getName() === argTypeName) ||
                  sourceFile
                    .getInterfaces()
                    .find((i) => i.getName() === argTypeName) ||
                  sourceFile
                    .getEnums()
                    .find((e) => e.getName() === argTypeName) ||
                  sourceFile
                    .getClasses()
                    .find((c) => c.getName() === argTypeName);

                if (
                  argTypeDecl &&
                  !this.collectedDeclarations.has(argTypeDecl)
                ) {
                  this.collectedDeclarations.add(argTypeDecl);
                  declarationsToProcess.push(argTypeDecl);
                  console.log(`Added type argument: ${argTypeName}`);
                }
              }

              this.typeProcessor.processTypeNode(argNode);
            });
          }
        }

        if (Node.isIndexedAccessTypeNode(node)) {
          const objectType = node.getObjectTypeNode();
          if (Node.isTypeReference(objectType)) {
            const typeName = objectType.getTypeName().getText();
            if (typeName === "Scalars") {
              const sourceFile = node.getSourceFile();
              const scalarsType = sourceFile
                .getTypeAliases()
                .find((ta) => ta.getName() === "Scalars");

              if (scalarsType && !this.collectedDeclarations.has(scalarsType)) {
                this.collectedDeclarations.add(scalarsType);
                declarationsToProcess.push(scalarsType);
                console.log(`Added Scalars type`);
              }
            }
          }
        }

        node.forEachChild((child) => processTypeReferences(child));
      };

      processTypeReferences(decl);
    }
  }

  clear(): void {
    this.collectedDeclarations.clear();
    this.seenNodesForRecursion.clear();
    this.compositeAliasesToGenerate.clear();
    this.typeProcessor.clear();
  }
}
