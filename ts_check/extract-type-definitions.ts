#!/usr/bin/env ts-node

import {
  Project,
  Node,
  Type,
  TypeNode,
  SourceFile,
  NamedTupleMember,
  RestTypeNode,
  ts,
} from "ts-morph";

// --- Argument Parsing ---
// Format: JSON string containing array of type information objects
// outputFile: Optional path for the output .ts file.
// tsconfigPath: Optional path to the tsconfig.json file for the project
const [
  ,
  ,
  inputTypesJson,
  outputFile = "out/all-types-recursive.ts",
  tsconfigPath,
] = process.argv;

interface TypeInfo {
  filePath: string; // Path to the file where the type usage occurs
  startPosition: number; // UTF-16 offset of the main identifier in the composite type
  compositeTypeString: string; // The full text, e.g., "Response<User[]>"
  alias: string; // The generated alias name, e.g., "ResUsersGet"
}

interface CompositeAliasInfo {
  aliasName: string;
  typeString: string;
}

let typeInfos: TypeInfo[] = [];

try {
  typeInfos = JSON.parse(inputTypesJson);
} catch (error) {
  console.error(
    'Usage: ts-node extract-type-definitions.ts \'[{"filePath":"path/to/file.ts","typeName":"TypeName","startPosition":123},...]\' [outputFile] [tsconfigPath]',
  );
  console.error("Error:", error);
  process.exit(1);
}

if (typeInfos.length === 0) {
  console.error("No type information provided");
  process.exit(1);
}

// --- Project Setup ---
const project = new Project({
  tsConfigFilePath: tsconfigPath, // Use the provided tsconfig path
  skipAddingFilesFromTsConfig: false, // Allow loading files from tsconfig paths
});

function isLocalType(node: Node): boolean {
  const filePath = node.getSourceFile().getFilePath();
  return !filePath.includes("node_modules");
}

function findTypeReferenceAtPosition(
  sf: SourceFile,
  position: number,
): Node | undefined {
  const node = sf.getDescendantAtPos(position);
  if (!node) return undefined;

  // If we landed on an identifier, check if it's part of a type reference
  if (Node.isIdentifier(node)) {
    // Look for a TypeReference parent
    let current: Node | undefined = node;
    while (current) {
      if (Node.isTypeReference(current)) {
        return current; // Found the TypeReference
      }
      current = current.getParent();
      if (!current) break;
    }
  }

  return node;
}

function processTypeReference(typeRef: Node): void {
  if (!Node.isTypeReference(typeRef)) return;

  const typeName = typeRef.getTypeName().getText();
  console.log(`Processing type reference: ${typeName}`);
  console.log("--------> " + typeRef.getSourceFile().getFilePath());

  // Get the symbol for the main type (e.g., Request, Response)
  const symbol = typeRef.getTypeName().getSymbol();
  if (symbol) {
    // Collect the main type (as an import if it's from node_modules)
    for (const decl of symbol.getDeclarations()) {
      const isNodeModule = decl
        .getSourceFile()
        .getFilePath()
        .includes("node_modules");
      if (isNodeModule) {
        // Add to imports, but don't recurse deeply
        addImportForExternalType(decl);
      } else {
        // Local type - recurse deeply
        collectDeclarationsRecursively(decl, true);
      }
    }
  }

  // Process type arguments (whether local or external)
  for (const typeArg of typeRef.getTypeArguments()) {
    console.log(`  - Type argument: ${typeArg.getText()}`);
    console.log("--------> " + typeArg.getSourceFile().getFilePath());

    // Process type arguments recursively
    processTypeArgument(typeArg);
  }
}

// Helper function to add import for external types
function addImportForExternalType(node: Node): void {
  const sourceFilePath = node.getSourceFile().getFilePath();

  // Try to get the module specifier (e.g. "express")
  const importDecl = node.getFirstAncestorByKind?.(
    ts.SyntaxKind.ImportDeclaration,
  );
  let moduleSpecifier: string | undefined;
  if (importDecl) {
    moduleSpecifier = importDecl.getModuleSpecifierValue();
  } else {
    moduleSpecifier = getModuleSpecifierFromNodeModulesPath(sourceFilePath);
  }

  // Get the type name
  const typeName =
    Node.isInterfaceDeclaration(node) ||
    Node.isClassDeclaration(node) ||
    Node.isTypeAliasDeclaration(node) ||
    Node.isEnumDeclaration(node)
      ? node.getName?.()
      : undefined;

  if (typeName && moduleSpecifier) {
    // Prevent imports from "typescript" (standard library) and "@types/node" (Node.js globals)
    if (moduleSpecifier === "typescript" || moduleSpecifier === "@types/node") {
      return; // Do not add to externalTypeImports
    }

    if (!externalTypeImports.has(moduleSpecifier)) {
      externalTypeImports.set(moduleSpecifier, new Set());
    }
    externalTypeImports.get(moduleSpecifier)!.add(typeName);
  }
}

// Function to find declaration by position
function findTypeDeclarationByPosition(
  sf: SourceFile,
  position: number,
): Node | undefined {
  const identifierNode = findTypeReferenceAtPosition(sf, position);

  console.log("Node text:", identifierNode?.getText());
  console.log("Node kind:", identifierNode?.getKindName());
  console.log("Node file:", identifierNode?.getSourceFile().getFilePath());

  if (!identifierNode) return undefined;
  const symbol = identifierNode.getSymbol();
  if (!symbol) return undefined;

  function resolveTypeSymbol(
    symbol: import("ts-morph").Symbol,
    depth = 0,
  ): Node | undefined {
    if (depth > 10) return undefined;
    for (const d of symbol.getDeclarations()) {
      if (
        Node.isTypeAliasDeclaration(d) ||
        Node.isInterfaceDeclaration(d) ||
        Node.isEnumDeclaration(d) ||
        Node.isClassDeclaration(d)
      ) {
        return d;
      }
      // Parameter/variable/property: follow type reference
      if (
        Node.isParameterDeclaration(d) ||
        Node.isVariableDeclaration(d) ||
        Node.isPropertySignature(d) ||
        Node.isPropertyDeclaration(d)
      ) {
        const typeNode = d.getTypeNode();
        if (typeNode && Node.isTypeReference(typeNode)) {
          const typeName = typeNode.getTypeName();
          const typeSymbol = typeName.getSymbol();
          if (typeSymbol) {
            const resolved = resolveTypeSymbol(typeSymbol, depth + 1);
            if (resolved) return resolved;
          }
        }
      }
      // Import: follow aliased symbol
      if (Node.isImportSpecifier(d)) {
        const aliasedSymbol = d.getSymbol()?.getAliasedSymbol();
        if (aliasedSymbol) {
          const resolved = resolveTypeSymbol(aliasedSymbol, depth + 1);
          if (resolved) return resolved;
        }
      }
    }
    return undefined;
  }

  const decl = resolveTypeSymbol(symbol);
  if (decl) return decl;
  return symbol.getDeclarations()[0];
}

// --- Dependency Collection Logic ---
// `collectedDeclarations`: Stores the actual TypeAlias, Interface, Enum, Class declaration nodes to be written.
const collectedDeclarations = new Set<Node>();
// `seenNodesForRecursion`: Prevents infinite loops and re-processing during the traversal of any Node.
const seenNodesForRecursion = new Set<Node>();
// Add tracking for type objects to prevent infinite recursion
const seenTypeObjects = new WeakSet<Type>();
// Track node_module imports for types
const externalTypeImports = new Map<string, Set<string>>();

const compositeAliasesToGenerate = new Set<CompositeAliasInfo>();

// Function to get or add a source file to the project
const sourceFileCache = new Map<string, SourceFile>();
function getSourceFile(filePath: string): SourceFile | undefined {
  if (sourceFileCache.has(filePath)) {
    return sourceFileCache.get(filePath);
  }

  const sf =
    project.getSourceFile(filePath) || project.addSourceFileAtPath(filePath);
  if (sf) {
    sourceFileCache.set(filePath, sf);
  }
  return sf;
}

// Process each type info and collect declarations
console.log(`Processing ${typeInfos.length} types from input`);

for (const typeInfo of typeInfos) {
  const { filePath, startPosition, compositeTypeString, alias } = typeInfo;

  const sourceFile = getSourceFile(filePath);
  if (!sourceFile) {
    console.error(`Input file '${filePath}' could not be found or loaded.`);
    continue;
  }

  const typeRefNode = findTypeReferenceAtPosition(sourceFile, startPosition);

  if (!typeRefNode) {
    console.error(
      `Type not found in '${sourceFile.getFilePath()}' at position ${startPosition}`,
    );
    continue;
  }

  console.log(
    `Found type reference at ${startPosition} in ${filePath}: ${typeRefNode.getText()}`,
  );

  // Process the type reference
  processTypeReference(typeRefNode);

  if (compositeTypeString && alias) {
    compositeAliasesToGenerate.add({
      aliasName: alias,
      typeString: compositeTypeString,
    });
    console.log(
      `Queued composite alias: export type ${alias} = ${compositeTypeString};`,
    );
  }
}

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
            collectFromTypeObject(resolvedType, typeNode);
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

/**
 * Processes a ts-morph Type object to find and collect declarations of types it refers to.
 */
function collectFromTypeObject(
  type: Type | undefined,
  contextNode: Node,
): void {
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
      .forEach((argType) => collectFromTypeObject(argType, contextNode));

    // Process union and intersection types
    if (type.isUnion()) {
      type
        .getUnionTypes()
        .forEach((ut) => collectFromTypeObject(ut, contextNode));
    }
    if (type.isIntersection()) {
      type
        .getIntersectionTypes()
        .forEach((it) => collectFromTypeObject(it, contextNode));
    }

    // Process array and tuple element types
    if (type.isArray())
      collectFromTypeObject(type.getArrayElementType(), contextNode);
    if (type.isTuple())
      type
        .getTupleElements()
        .forEach((te) => collectFromTypeObject(te, contextNode));

    // For object/interface types, process the types of their properties.
    // This is crucial for anonymous object types within type aliases (e.g., type X = { a: TypeA };)

    // IMPORTANT: Add depth limit for property traversal to prevent infinite recursion
    const MAX_PROPERTY_DEPTH = 5; // Adjust this value as needed
    traversePropertiesWithDepthLimit(type, MAX_PROPERTY_DEPTH, contextNode);
  } catch (error) {
    console.warn(`Error processing type: ${error}`);
  }
}

// Helper function to limit property traversal depth
function traversePropertiesWithDepthLimit(
  type: Type,
  depthRemaining: number,
  contextNode: Node,
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
          const propType = prop.getTypeAtLocation(contextNode as Node);

          // Only process the immediate symbol of the property type, don't go deeper
          const symbol = propType.getAliasSymbol() || propType.getSymbol();
          if (symbol) {
            symbol
              .getDeclarations()
              .forEach((decl) => collectDeclarationsRecursively(decl));
          }

          // Only recurse to property types with decreased depth
          if (depthRemaining > 1) {
            collectFromTypeObject(propType, contextNode);
          }
        } catch (propError) {
          console.warn(`Error processing property: ${propError}`);
        }
      });
    } else {
      properties.forEach((prop) => {
        try {
          const propType = prop.getTypeAtLocation(contextNode as Node);
          collectFromTypeObject(propType, contextNode);
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

function getModuleSpecifierFromNodeModulesPath(
  filePath: string,
): string | undefined {
  // Handles both scoped and unscoped packages
  // e.g. node_modules/express/...
  //      node_modules/@types/express/...
  const nodeModulesIdx = filePath.lastIndexOf("node_modules/");
  if (nodeModulesIdx === -1) return undefined;
  const afterNodeModules = filePath.slice(
    nodeModulesIdx + "node_modules/".length,
  );
  const parts = afterNodeModules.split(/[\\/]/); // split on / or \
  if (parts[0].startsWith("@")) {
    // Scoped package
    return parts.length >= 2 ? `${parts[0]}/${parts[1]}` : undefined;
  } else {
    // Unscoped package
    return parts[0];
  }
}

function processTypeArgument(typeArg: TypeNode): void {
  if (Node.isTypeReference(typeArg)) {
    const argTypeName = typeArg.getTypeName().getText();
    const argSymbol = typeArg.getTypeName().getSymbol();

    if (argSymbol) {
      // Find declarations for this type argument
      for (const argDecl of argSymbol.getDeclarations()) {
        const isNodeModule = argDecl
          .getSourceFile()
          .getFilePath()
          .includes("node_modules");
        if (isNodeModule) {
          // Add to imports, but don't recurse deeply
          addImportForExternalType(argDecl);
        } else {
          // Local type - recurse deeply
          console.log(`  - Found local type argument: ${argTypeName}`);
          collectDeclarationsRecursively(argDecl, true);
        }
      }
    }

    // Also process any type arguments of this type argument (nested generics)
    for (const innerArg of typeArg.getTypeArguments()) {
      processTypeArgument(innerArg);
    }
  }
  // Handle array types
  else if (Node.isArrayTypeNode(typeArg)) {
    processTypeArgument(typeArg.getElementTypeNode());
  }
  // Add handling for other type constructs as needed
}

/**
 * Main recursive function to collect declarations.
 * Starts from a Node (typically a declaration), adds it if relevant,
 * and then explores its structure and associated Type for further dependencies.
 */
function collectDeclarationsRecursively(
  node: Node | undefined,
  isRoot = false, // isRoot helps decide if we should deeply explore a local type
): void {
  if (!node || seenNodesForRecursion.has(node)) {
    return;
  }
  // Add to seen *before* any recursive calls to prevent loops, especially with imports
  seenNodesForRecursion.add(node);

  // If node is an import specifier, resolve it and recurse on its actual declaration(s)
  if (Node.isImportSpecifier(node)) {
    const aliasedSymbol = node.getSymbol()?.getAliasedSymbol(); // Get the symbol of what's being imported
    if (aliasedSymbol) {
      console.log(
        `CDR: Resolving import specifier "${node.getName()}" to its original symbol.`,
      );
      for (const aliasedDecl of aliasedSymbol.getDeclarations()) {
        // Recurse on the actual declaration.
        // Treat it as a new "root" type we want to process fully if it's local.
        console.log(
          `CDR:   ↳ Found aliased declaration: ${aliasedDecl.getKindName()} in ${aliasedDecl.getSourceFile().getFilePath()}`,
        );
        collectDeclarationsRecursively(aliasedDecl, true); // Pass true for isRoot
      }
    } else {
      console.warn(
        `CDR: Could not get aliased symbol for import specifier "${node.getName()}"`,
      );
    }
    return; // We've handled the import specifier by recursing on its target.
  }

  const sourceFilePath = node.getSourceFile().getFilePath();

  if (sourceFilePath.includes("node_modules")) {
    addImportForExternalType(node); // This function should only add if it's a named exportable type
    // We might still want to process its type arguments if this 'node' is a TypeReference itself
    // This was handled by processExternalTypeReference, let's ensure it's called if needed.
    // For now, addImportForExternalType is the main action.
    // If 'node' is a TypeReference from node_modules, its type args are handled by processTypeReference.
    return;
  }

  // If 'node' is a type declaration we want to keep (TypeAlias, Interface, Enum, Class)
  if (
    Node.isTypeAliasDeclaration(node) ||
    Node.isInterfaceDeclaration(node) ||
    Node.isEnumDeclaration(node) ||
    Node.isClassDeclaration(node)
  ) {
    console.log(
      `CDR: Adding to collectedDeclarations: ${node.getKindName()} "${node.getName?.()}" from ${sourceFilePath}`,
    );
    collectedDeclarations.add(node); // Add this declaration to our output set

    // If it's a local type, we always want to explore its structure.
    // The `isRoot` flag was mainly to ensure that initial types from TypeInfo are explored.
    // Since we've passed the node_modules check, this node is local.
    // So, we should explore its structure.

    // 1. Process type parameters
    if (
      Node.isClassDeclaration(node) ||
      Node.isInterfaceDeclaration(node) ||
      Node.isTypeAliasDeclaration(node) ||
      Node.isFunctionDeclaration(node) || // Added FunctionDeclaration for completeness
      Node.isMethodDeclaration(node) // Added MethodDeclaration for completeness
    ) {
      // Check if getTypeParameters method exists
      if (typeof (node as any).getTypeParameters === "function") {
        const typeParams = (node as any).getTypeParameters() as Node[]; // Assuming it returns Node[] or similar
        if (Array.isArray(typeParams)) {
          typeParams.forEach((tpNode) => {
            // tpNode is likely a TypeParameterDeclaration
            // A TypeParameterDeclaration has .getConstraint() and .getDefault()
            // which return TypeNode | undefined
            const tp = tpNode as import("ts-morph").TypeParameterDeclaration;
            const constraint = tp.getConstraint();
            if (constraint) processTypeNode(constraint);
            const defaultType = tp.getDefault();
            if (defaultType) processTypeNode(defaultType);
          });
        }
      }
    }

    // 2. Heritage Clauses (extends, implements)
    if (Node.isInterfaceDeclaration(node) || Node.isClassDeclaration(node)) {
      node.getHeritageClauses().forEach((hc) => {
        hc.getTypeNodes().forEach((typeNode) => processTypeNode(typeNode));
      });
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
          Node.isConstructorDeclaration(member) || // Added ConstructorDeclaration
          Node.isGetAccessorDeclaration(member) ||
          Node.isSetAccessorDeclaration(member)
        ) {
          if (typeof (member as any).getParameters === "function") {
            (member as any)
              .getParameters()
              .forEach((param: any) => processTypeNode(param.getTypeNode()));
          }
          if (typeof (member as any).getReturnTypeNode === "function") {
            processTypeNode((member as any).getReturnTypeNode());
          }
        }
      });
    }
    // For TypeAliasDeclaration, process its underlying TypeNode
    if (Node.isTypeAliasDeclaration(node)) {
      processTypeNode(node.getTypeNode());
    }

    // Also, get the general Type object for the declaration and explore it.
    collectFromTypeObject(node.getType(), node);
  } else if (Node.isVariableDeclaration(node)) {
    // If we encounter a variable declaration (e.g., from `typeof X` where X is a const),
    // try to get its type and collect dependencies from that type.
    collectFromTypeObject(node.getType(), node);
  }
  // Other node types (like ImportSpecifier) are handled at the top or not directly collected.
}

// --- Start Dependency Collection ---
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
// Sort by file path, then by position.
// This is NOT a full topological sort, which would be more robust for complex inter-dependencies.
const sortedDeclarations = Array.from(collectedDeclarations).sort((a, b) => {
  // Get the file paths of the first file in typeInfos for prioritization
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

for (const decl of sortedDeclarations) {
  newSourceFile.addStatements(decl.getText());
}

let importStatements = "";
for (const [moduleSpecifier, typeNames] of externalTypeImports.entries()) {
  importStatements += `import { ${Array.from(typeNames).join(", ")} } from "${moduleSpecifier}";\n`;
}
newSourceFile.insertText(0, importStatements);

const sortedCompositeAliases = Array.from(compositeAliasesToGenerate).sort(
  (a, b) => a.aliasName.localeCompare(b.aliasName),
);

for (const { aliasName, typeString } of sortedCompositeAliases) {
  newSourceFile.addStatements(`export type ${aliasName} = ${typeString};\n`);
}

try {
  newSourceFile.formatText(); // Apply standard formatting
  newSourceFile.saveSync();
  console.log(
    `All recursively aquired type/interface/enum/class declarations written to ${outputFile}`,
  );

  // Output a success message in JSON format for easy parsing by the calling process
  console.log(
    JSON.stringify({
      success: true,
      output: outputFile,
      typeCount: collectedDeclarations.size,
    }),
  );
} catch (e: any) {
  console.error("Error saving or formatting the output file:", e.message);
  // Output error in JSON format
  console.log(
    JSON.stringify({
      success: false,
      error: e.message,
    }),
  );
}
