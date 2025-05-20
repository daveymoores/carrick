#!/usr/bin/env ts-node

import {
  Project,
  Node,
  Type,
  TypeNode,
  SourceFile,
  NamedTupleMember,
  RestTypeNode,
} from "ts-morph";

// --- Argument Parsing ---
// inputFile: The .ts file containing the target type.
// targetTypeName: The name of the type/interface/enum/class to extract.
// outputFile: Optional path for the output .ts file.
const [
  ,
  ,
  inputFile,
  targetTypeName,
  outputFile = "out/all-types-recursive.ts",
] = process.argv;

if (!inputFile || !targetTypeName) {
  console.error(
    "Usage: ts-node extract-type-definitions.ts <inputFile> <targetTypeName> [outputFile]",
  );
  process.exit(1);
}

// --- Project Setup ---
const project = new Project({
  tsConfigFilePath:
    "../../../optaxe/optaxe-ts-monorepo/apps/event-api/tsconfig.json", // Ensure this points to your project's tsconfig
  skipAddingFilesFromTsConfig: false, // Allow loading files from tsconfig paths
});

const sourceFile =
  project.getSourceFile(inputFile) || project.addSourceFileAtPath(inputFile);
if (!sourceFile) {
  console.error(`Input file '${inputFile}' could not be found or loaded.`);
  process.exit(1);
}

// --- Find Initial Target Declaration ---
function findInitialDeclaration(
  sf: SourceFile,
  name: string,
): Node | undefined {
  // Search for the type by name across different kinds of declarations
  const declarations = [
    ...sf.getTypeAliases().filter((ta) => ta.getName() === name),
    ...sf.getInterfaces().filter((i) => i.getName() === name),
    ...sf.getEnums().filter((e) => e.getName() === name),
    ...sf.getClasses().filter((c) => c.getName() === name),
  ];
  if (declarations.length > 1) {
    console.warn(
      `Multiple declarations found for '${name}'. Using the first one found.`,
    );
  }
  return declarations[0];
}

const initialDeclarationNode = findInitialDeclaration(
  sourceFile,
  targetTypeName,
);

if (!initialDeclarationNode) {
  console.error(
    `Type '${targetTypeName}' not found in '${sourceFile.getFilePath()}'.`,
  );
  process.exit(1);
}

// --- Dependency Collection Logic ---
// `collectedDeclarations`: Stores the actual TypeAlias, Interface, Enum, Class declaration nodes to be written.
const collectedDeclarations = new Set<Node>();
// `seenNodesForRecursion`: Prevents infinite loops and re-processing during the traversal of any Node.
const seenNodesForRecursion = new Set<Node>();

/**
 * Processes a TypeNode (AST representation of a type, e.g., string, MyInterface, TypeA | TypeB)
 * to find and collect its referenced type declarations.
 */
function processTypeNode(typeNode: TypeNode | undefined): void {
  if (!typeNode || seenNodesForRecursion.has(typeNode)) {
    return;
  }
  seenNodesForRecursion.add(typeNode);

  try {
    // Handle TypeReference nodes (e.g., MyType, AnotherType<Arg>)
    if (Node.isTypeReference(typeNode)) {
      const nameNode = typeNode.getTypeName();
      const symbol = nameNode.getSymbol();
      if (symbol) {
        symbol
          .getDeclarations()
          .forEach((decl) => collectDeclarationsRecursively(decl));
      }

      typeNode
        .getTypeArguments()
        .forEach((argTypeNode) => processTypeNode(argTypeNode));
    }
    // Handle Union and Intersection types
    else if (
      Node.isUnionTypeNode(typeNode) ||
      Node.isIntersectionTypeNode(typeNode)
    ) {
      typeNode.getTypeNodes().forEach((tn) => processTypeNode(tn));
    }
    // Handle Array types
    else if (Node.isArrayTypeNode(typeNode)) {
      processTypeNode(typeNode.getElementTypeNode());
    }
    // Handle Tuple types
    else if (Node.isTupleTypeNode(typeNode)) {
      // In ts-morph 25.0.1, use getElements()
      typeNode.getElements().forEach((element) => {
        if (Node.isTypeNode(element)) {
          processTypeNode(element);
        } else if (Node.isNamedTupleMember(element)) {
          // Named tuple members have a type node
          const memberTypeNode = (element as NamedTupleMember).getTypeNode();
          if (memberTypeNode) processTypeNode(memberTypeNode);
        } else if (Node.isRestTypeNode(element)) {
          // Rest elements have a type node
          const restTypeNode = (element as RestTypeNode).getTypeNode();
          if (restTypeNode) processTypeNode(restTypeNode);
        } else {
          // Safety check - log any unexpected element types
          console.warn(
            `Unexpected tuple element type: ${(element as TypeNode).getKindName()}`,
          );
          // Try to process any type nodes found in the children
          (element as TypeNode)
            .getChildren()
            .filter(Node.isTypeNode)
            .forEach((childTypeNode) => processTypeNode(childTypeNode));
        }
      });
    }
    // Handle Mapped types
    else if (Node.isMappedTypeNode(typeNode)) {
      // Handle type parameter constraint
      const typeParameter = typeNode.getTypeParameter();
      const constraint = typeParameter.getConstraint();
      if (constraint) {
        processTypeNode(constraint);
      }

      // Handle the parameter type - in ts-morph 25.0.1, use getNameType()
      const nameType = typeNode.getNameTypeNode();
      if (nameType) {
        processTypeNode(nameType);
      }

      // Handle the value type
      const valueTypeNode = typeNode.getTypeNode();
      if (valueTypeNode) {
        processTypeNode(valueTypeNode);
      }
    }
    // Handle parenthesized, type operator, and indexed access types
    else if (Node.isParenthesizedTypeNode(typeNode)) {
      processTypeNode(typeNode.getTypeNode());
    } else if (Node.isTypeOperatorTypeNode(typeNode)) {
      processTypeNode(typeNode.getTypeNode());
    } else if (Node.isIndexedAccessTypeNode(typeNode)) {
      processTypeNode(typeNode.getObjectTypeNode());
      processTypeNode(typeNode.getIndexTypeNode());
    }
    // Handle 'typeof X' expressions
    else if (Node.isTypeQuery(typeNode)) {
      const exprName = typeNode.getExprName();
      const symbol = exprName.getSymbol();

      if (symbol) {
        symbol.getDeclarations().forEach((decl) => {
          if (
            Node.isClassDeclaration(decl) ||
            Node.isEnumDeclaration(decl) ||
            Node.isVariableDeclaration(decl) ||
            Node.isFunctionDeclaration(decl) ||
            Node.isInterfaceDeclaration(decl)
          ) {
            collectDeclarationsRecursively(decl);
          }
        });
      }
    }
    // Handle import type nodes: import('./types').MyType
    else if (Node.isImportTypeNode(typeNode)) {
      try {
        // Process the qualifier (the part after the import)
        const qualifier = typeNode.getQualifier();

        // Check if qualifier exists and is a valid entity name
        // In ts-morph 25.0.1, we might need to use a different approach
        if (qualifier) {
          // Alternatives to isEntityName:
          if (Node.isIdentifier(qualifier) || Node.isQualifiedName(qualifier)) {
            const symbol = qualifier.getSymbol?.();
            if (symbol) {
              symbol
                .getDeclarations()
                .forEach((decl) => collectDeclarationsRecursively(decl));
            }
          }
        }

        // Process any type arguments of the import type
        typeNode.getTypeArguments?.()?.forEach((argType) => {
          processTypeNode(argType);
        });

        // Also process using the resolved type, which is more reliable
        try {
          const resolvedType = typeNode.getType();
          if (resolvedType) {
            collectFromTypeObject(resolvedType);
          }
        } catch (typeError) {
          console.warn(`Error resolving import type: ${typeError}`);
        }
      } catch (importError) {
        console.warn(`Error processing import type: ${importError}`);
      }
    }
    // Handle conditional types (A extends B ? C : D)
    else if (Node.isConditionalTypeNode(typeNode)) {
      processTypeNode(typeNode.getCheckType());
      processTypeNode(typeNode.getExtendsType());
      processTypeNode(typeNode.getTrueType());
      processTypeNode(typeNode.getFalseType());
    }
    // Handle infer types (infer T)
    else if (Node.isInferTypeNode(typeNode)) {
      const typeParameter = typeNode.getTypeParameter();
      const constraint = typeParameter.getConstraint();
      if (constraint) processTypeNode(constraint);

      const defaultType = typeParameter.getDefault();
      if (defaultType) processTypeNode(defaultType);
    }
    // Handle template literal types (`foo${Bar}`)
    else if (Node.isTemplateLiteralTypeNode(typeNode)) {
      const spans = typeNode.getTemplateSpans();
      spans.forEach((span) => {
        processTypeNode(span);
      });
    }
    // Handle literal types (string, number, boolean literals)
    else if (Node.isLiteralTypeNode(typeNode)) {
      const literal = typeNode.getLiteral();
      if (Node.isIdentifier(literal)) {
        const symbol = literal.getSymbol();
        if (symbol) {
          symbol
            .getDeclarations()
            .forEach((decl) => collectDeclarationsRecursively(decl));
        }
      }
    }
    // Fallback: look for identifiers that might be types
    typeNode.forEachDescendant((descendant) => {
      if (Node.isIdentifier(descendant)) {
        // Skip identifiers we've already handled via TypeReference
        const parent = descendant.getParent();

        // Check if the parent is part of a TypeReference structure
        // Without using isEntityName (which appears to be missing in ts-morph 25.0.1)
        if (
          parent &&
          (Node.isIdentifier(parent) || Node.isQualifiedName(parent)) &&
          parent.getParent() &&
          Node.isTypeReference(parent.getParent())
        ) {
          return; // Already processed
        }

        // Alternative check that captures more cases:
        // Check if this identifier appears to be part of a type reference path
        let currentNode: Node | undefined = descendant;
        let isPartOfTypeRef = false;

        while (currentNode?.getParent()) {
          currentNode = currentNode.getParent();
          if (Node.isTypeReference(currentNode)) {
            isPartOfTypeRef = true;
            break;
          }
        }

        if (isPartOfTypeRef) {
          return; // Skip processing if it's part of a type reference
        }

        // Process other identifiers that might reference types
        const symbol = descendant.getSymbol();
        if (symbol) {
          symbol.getDeclarations().forEach((decl) => {
            collectDeclarationsRecursively(decl);
          });
        }
      }
    });
  } catch (error) {
    console.warn(`Error processing type node: ${error}`);
    // For debugging you might want to see which node caused the issue:
    console.warn(`Type node text: ${typeNode.getText()}`);
  }
}

// Add tracking for type objects to prevent infinite recursion
const seenTypeObjects = new WeakSet<Type>();

/**
 * Processes a ts-morph Type object to find and collect declarations of types it refers to.
 */
function collectFromTypeObject(type: Type | undefined): void {
  if (!type || seenTypeObjects.has(type)) return;
  seenTypeObjects.add(type);

  try {
    // Use alias symbol first if available (e.g., for type aliases)
    const symbol = type.getAliasSymbol() || type.getSymbol();
    if (symbol) {
      for (const decl of symbol.getDeclarations()) {
        collectDeclarationsRecursively(decl);
      }
    }

    // Process type arguments (for generics)
    type
      .getTypeArguments()
      .forEach((argType) => collectFromTypeObject(argType));

    // Process union and intersection types
    if (type.isUnion()) {
      type.getUnionTypes().forEach((ut) => collectFromTypeObject(ut));
    }
    if (type.isIntersection()) {
      type.getIntersectionTypes().forEach((it) => collectFromTypeObject(it));
    }

    // Process array and tuple element types
    if (type.isArray()) collectFromTypeObject(type.getArrayElementType());
    if (type.isTuple())
      type.getTupleElements().forEach((te) => collectFromTypeObject(te));

    // For object/interface types, process the types of their properties.
    // This is crucial for anonymous object types within type aliases (e.g., type X = { a: TypeA };)

    // IMPORTANT: Add depth limit for property traversal to prevent infinite recursion
    const MAX_PROPERTY_DEPTH = 5; // Adjust this value as needed
    traversePropertiesWithDepthLimit(type, MAX_PROPERTY_DEPTH);
  } catch (error) {
    console.warn(`Error processing type: ${error}`);
  }
}

// Helper function to limit property traversal depth
function traversePropertiesWithDepthLimit(
  type: Type,
  depthRemaining: number,
): void {
  if (depthRemaining <= 0) {
    console.warn("Maximum property traversal depth reached");
    return;
  }

  try {
    // Only get direct properties, limit their processing
    const properties = type.getProperties();
    if (properties.length > 50) {
      console.warn(
        `Type has ${properties.length} properties, limiting to avoid overflow`,
      );
      properties.slice(0, 50).forEach((prop) => {
        try {
          // Get the type of the property
          const propType = prop.getTypeAtLocation(
            initialDeclarationNode as Node,
          );

          // Only process the immediate symbol of the property type, don't go deeper
          const symbol = propType.getAliasSymbol() || propType.getSymbol();
          if (symbol) {
            symbol
              .getDeclarations()
              .forEach((decl) => collectDeclarationsRecursively(decl));
          }

          // Only recurse to property types with decreased depth
          if (depthRemaining > 1) {
            collectFromTypeObject(propType);
          }
        } catch (propError) {
          console.warn(`Error processing property: ${propError}`);
        }
      });
    } else {
      properties.forEach((prop) => {
        try {
          const propType = prop.getTypeAtLocation(
            initialDeclarationNode as Node,
          );
          collectFromTypeObject(propType);
        } catch (propError) {
          console.warn(`Error processing property: ${propError}`);
        }
      });
    }
  } catch (error) {
    console.warn(`Error traversing properties: ${error}`);
  }
}

/**
 * Collects utility types and their inner type arguments
 */
function collectUtilityTypesWithInnerTypes(): void {
  // Keep track of utility type names we've found
  const utilityTypeNames = new Set<string>();

  // Examine all collected declarations
  const declarationsToProcess = [...collectedDeclarations];

  // Process all declarations, including newly added ones as we go
  for (let i = 0; i < declarationsToProcess.length; i++) {
    const decl = declarationsToProcess[i];

    // Function to find and process TypeReference nodes
    function processTypeReferences(node: Node): void {
      if (Node.isTypeReference(node)) {
        const typeName = node.getTypeName().getText();
        const typeArgs = node.getTypeArguments();

        // If this type has type arguments, process both the wrapper and inner types
        if (typeArgs.length > 0) {
          // Find the utility type declaration
          const sourceFile = node.getSourceFile();
          const utilityType = sourceFile
            .getTypeAliases()
            .find((ta) => ta.getName() === typeName);

          // Add the utility type if we haven't already
          if (utilityType && !collectedDeclarations.has(utilityType)) {
            collectedDeclarations.add(utilityType);
            declarationsToProcess.push(utilityType); // Add to our processing queue
            console.log(`Added utility type: ${typeName}`);
          }

          // Process each type argument to extract their declarations too
          typeArgs.forEach((argNode) => {
            // Handle direct type references in the arguments
            if (Node.isTypeReference(argNode)) {
              const argTypeName = argNode.getTypeName().getText();

              // Find the type declaration for this argument
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

              if (argTypeDecl && !collectedDeclarations.has(argTypeDecl)) {
                collectedDeclarations.add(argTypeDecl);
                declarationsToProcess.push(argTypeDecl); // Add to our processing queue
                console.log(`Added type argument: ${argTypeName}`);
              }
            }

            // Also process any complex types in the arguments
            processTypeNode(argNode);
          });
        }
      }

      // Handle indexed access for Scalars type
      if (Node.isIndexedAccessTypeNode(node)) {
        const objectType = node.getObjectTypeNode();
        if (Node.isTypeReference(objectType)) {
          const typeName = objectType.getTypeName().getText();
          if (typeName === "Scalars") {
            // Find and add the Scalars type declaration
            const sourceFile = node.getSourceFile();
            const scalarsType = sourceFile
              .getTypeAliases()
              .find((ta) => ta.getName() === "Scalars");

            if (scalarsType && !collectedDeclarations.has(scalarsType)) {
              collectedDeclarations.add(scalarsType);
              declarationsToProcess.push(scalarsType);
              console.log(`Added Scalars type`);
            }
          }
        }
      }

      // Recursively process all children
      node.forEachChild((child) => processTypeReferences(child));
    }

    // Start processing on this declaration
    processTypeReferences(decl);
  }
}

/**
 * Main recursive function to collect declarations.
 * Starts from a Node (typically a declaration), adds it if relevant,
 * and then explores its structure and associated Type for further dependencies.
 */
function collectDeclarationsRecursively(node: Node | undefined): void {
  if (!node || seenNodesForRecursion.has(node)) {
    return;
  }
  seenNodesForRecursion.add(node);

  const sourceFilePath = node.getSourceFile().getFilePath();
  // Avoid collecting built-in TypeScript library types (lib.d.ts)
  if (sourceFilePath.includes("node_modules/typescript/lib/")) {
    return;
  }

  // If 'node' is a type declaration we want to keep (TypeAlias, Interface, Enum, Class)
  if (
    Node.isTypeAliasDeclaration(node) ||
    Node.isInterfaceDeclaration(node) ||
    Node.isEnumDeclaration(node) ||
    Node.isClassDeclaration(node)
  ) {
    collectedDeclarations.add(node); // Add this declaration to our output set

    // For these declarations, also explore their internal structure for type references

    // 1. Process type parameters - replaces isGenericableNode
    if (
      Node.isClassDeclaration(node) ||
      Node.isInterfaceDeclaration(node) ||
      Node.isTypeAliasDeclaration(node) ||
      Node.isFunctionDeclaration(node) ||
      Node.isMethodDeclaration(node)
    ) {
      try {
        // Safe access to type parameters
        if (typeof (node as any).getTypeParameters === "function") {
          const typeParams = (node as any).getTypeParameters();

          if (Array.isArray(typeParams)) {
            typeParams.forEach((tp) => {
              // Process constraint
              if (typeof tp.getConstraint === "function") {
                const constraint = tp.getConstraint();
                if (constraint) processTypeNode(constraint);
              }

              // Process default
              if (typeof tp.getDefault === "function") {
                const defaultType = tp.getDefault();
                if (defaultType) processTypeNode(defaultType);
              }
            });
          }
        }
      } catch (e) {
        console.warn(`Error processing type parameters: ${e}`);
      }
    }

    // 2. Heritage Clauses (extends, implements)
    if (Node.isInterfaceDeclaration(node) || Node.isClassDeclaration(node)) {
      try {
        const heritageClauses = node.getHeritageClauses();

        heritageClauses.forEach((hc) => {
          // In ts-morph 25.0.1, we should use getTypeNodes()
          // If that doesn't exist, try alternatives
          let typeNodes: TypeNode[] = [];

          // Try different methods that might exist based on the API version
          if (typeof hc.getTypeNodes === "function") {
            typeNodes = hc.getTypeNodes();
          } else if (typeof hc.getType === "function") {
            typeNodes = hc.getTypeNodes();
          } else if (typeof hc.getChildren === "function") {
            // Fallback: try to get all TypeNode children
            typeNodes = hc.getChildren().filter(Node.isTypeNode) as TypeNode[];
          }

          // Process all found type nodes
          typeNodes.forEach((typeNode) => {
            if (typeNode) {
              processTypeNode(typeNode);
            }
          });
        });
      } catch (e) {
        console.warn(`Error processing heritage clauses: ${e}`);
      }
    }

    // 3. Members (properties, methods - their types, parameter types, return types)
    if (Node.isInterfaceDeclaration(node) || Node.isClassDeclaration(node)) {
      node.getMembers().forEach((member) => {
        if (
          Node.isPropertySignature(member) ||
          Node.isPropertyDeclaration(member)
        ) {
          processTypeNode(member.getTypeNode());
        } else if (
          Node.isMethodSignature(member) ||
          Node.isMethodDeclaration(member) ||
          Node.isConstructorDeclaration(member) ||
          Node.isGetAccessorDeclaration(member) ||
          Node.isSetAccessorDeclaration(member)
        ) {
          if (typeof member.getParameters === "function") {
            // Check if getParameters exists
            member
              .getParameters()
              .forEach((param) => processTypeNode(param.getTypeNode()));
          }
          if (
            (Node.isMethodSignature(member) ||
              Node.isMethodDeclaration(member) ||
              Node.isGetAccessorDeclaration(member)) &&
            typeof member.getReturnTypeNode === "function"
          ) {
            processTypeNode(member.getReturnTypeNode());
          }
        }
      });
    }
    // For TypeAliasDeclaration, process its underlying TypeNode
    if (Node.isTypeAliasDeclaration(node)) {
      processTypeNode(node.getTypeNode());
    }
    // For Enums, members usually don't define further type dependencies we need to process via TypeNodes here.
    // The enum declaration itself is the collected item.

    // Also, get the general Type object for the declaration and explore it.
    // This can catch dependencies not easily found by AST walking alone (e.g. complex conditional types).
    collectFromTypeObject(node.getType());
  } else if (Node.isVariableDeclaration(node)) {
    // If we encounter a variable declaration (e.g., from `typeof X` where X is a const),
    // try to get its type and collect dependencies from that type.
    collectFromTypeObject(node.getType());
  }
  // If 'node' is not a primary declaration type but might be a constituent part (like an ImportSpecifier),
  // its symbol should lead to the actual declaration, which will then be processed.
  // The `collectFromTypeObject` and `processTypeNode` functions handle resolving symbols to declarations.
}

// --- Start Dependency Collection ---
console.log(`Starting dependency collection for type: ${targetTypeName}`);
collectDeclarationsRecursively(initialDeclarationNode);
collectUtilityTypesWithInnerTypes();
console.log(
  `After utility collection: ${collectedDeclarations.size} declarations.`,
);

// --- Create Output File ---
const outputProject = new Project({
  // It can be beneficial to initialize the output project with compiler options
  // that match the source project, especially if using advanced TS features.
  // compilerOptions: project.getCompilerOptions().get(), // If needed
});
const newSourceFile = outputProject.createSourceFile(outputFile, "", {
  overwrite: true,
});

// Sort declarations to potentially improve readability and handle some ordering issues.
// A simple sort: declarations from the original target file first, then by file path, then by position.
// This is NOT a full topological sort, which would be more robust for complex inter-dependencies.
const sortedDeclarations = Array.from(collectedDeclarations).sort((a, b) => {
  const aIsFromInputFile = a.getSourceFile() === sourceFile;
  const bIsFromInputFile = b.getSourceFile() === sourceFile;

  if (aIsFromInputFile && !bIsFromInputFile) return -1;
  if (!aIsFromInputFile && bIsFromInputFile) return 1;

  const pathA = a.getSourceFile().getFilePath();
  const pathB = b.getSourceFile().getFilePath();
  if (pathA !== pathB) {
    return pathA.localeCompare(pathB);
  }
  return a.getStart() - b.getStart();
});

for (const decl of sortedDeclarations) {
  newSourceFile.addStatements(decl.getText());
}

try {
  newSourceFile.formatText(); // Apply standard formatting
  newSourceFile.saveSync();
  console.log(
    `All recursively required type/interface/enum/class declarations for '${targetTypeName}' written to ${outputFile}`,
  );
} catch (e: any) {
  console.error("Error saving or formatting the output file:", e.message);
  // As a fallback, you could try writing the raw text without formatting
  // fs.writeFileSync(outputFile, sortedDeclarations.map(d => d.getText()).join('\n\n'));
  // console.log(`Collected types (unformatted due to error) written to ${outputFile}`);
}
