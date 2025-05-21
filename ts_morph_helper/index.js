#!/usr/bin/env node

const { Project, Node, ts } = require("ts-morph");

// Parse command line arguments
// Expected format: node index.js fileA typeA fileB typeB [posA] [posB]
const [, , fileA, typeA, fileB, typeB, posA, posB] = process.argv;

if (!fileA || !typeA || !fileB || !typeB) {
  console.error("Usage: node index.js fileA typeA fileB typeB [posA] [posB]");
  process.exit(1);
}

// Create a project and add the files
const project = new Project({
  tsConfigFilePath: "../../../optaxe/optaxe-ts-monorepo/apps/event-api/tsconfig.json",
  skipAddingFilesFromTsConfig: false,
});

const sourceFileA = project.getSourceFile(fileA) || project.addSourceFileAtPath(fileA);
const sourceFileB = project.getSourceFile(fileB) || project.addSourceFileAtPath(fileB);

if (!sourceFileA || !sourceFileB) {
  console.error(sourceFileA ? `File not found: ${fileB}` : `File not found: ${fileA}`);
  process.exit(1);
}

// Find type by name or position
function findTypeByNameOrPosition(sourceFile, typeName, position) {
  // If position is provided, use it to find the node
  if (position && !isNaN(parseInt(position))) {
    const pos = parseInt(position);
    const node = sourceFile.getDescendantAtPos(pos);
    
    if (!node) {
      console.error(`No node found at position ${pos} in ${sourceFile.getFilePath()}`);
      return null;
    }
    
    // Verify we found the right identifier
    if (Node.isIdentifier(node) && node.getText() === typeName) {
      const symbol = node.getSymbol();
      if (symbol) {
        const declarations = symbol.getDeclarations();
        if (declarations.length > 0) {
          return declarations[0].getType();
        }
      }
    } else {
      // Try to find parent nodes that might have the type info
      let currentNode = node;
      while (currentNode && currentNode !== sourceFile) {
        if (Node.isTypedNode(currentNode)) {
          return currentNode.getType();
        }
        if (Node.isTypeReferenceNode(currentNode)) {
          const typeSymbol = currentNode.getSymbol();
          if (typeSymbol) {
            const declarations = typeSymbol.getDeclarations();
            if (declarations.length > 0) {
              return declarations[0].getType();
            }
          }
        }
        currentNode = currentNode.getParent();
      }
      console.error(`Node at position ${pos} is not the expected identifier '${typeName}' or a typed node.`);
    }
    return null;
  }
  
  // Fallback to finding by name
  const typeAlias = sourceFile.getTypeAlias(typeName);
  if (typeAlias) return typeAlias.getType();
  
  const interfaceDecl = sourceFile.getInterface(typeName);
  if (interfaceDecl) return interfaceDecl.getType();
  
  const classDecl = sourceFile.getClass(typeName);
  if (classDecl) return classDecl.getType();
  
  // Check imports and global scope
  const allNodes = sourceFile.getDescendantsOfKind(ts.SyntaxKind.Identifier)
    .filter(identifier => identifier.getText() === typeName);
  
  for (const node of allNodes) {
    const symbol = node.getSymbol();
    if (symbol) {
      const declarations = symbol.getDeclarations();
      if (declarations.length > 0) {
        return declarations[0].getType();
      }
    }
  }
  
  return null;
}

// Find the types
const typeObjA = findTypeByNameOrPosition(sourceFileA, typeA, posA);
const typeObjB = findTypeByNameOrPosition(sourceFileB, typeB, posB);

if (!typeObjA || !typeObjB) {
  const result = {
    isAssignable: false,
    error: !typeObjA ? `Type '${typeA}' not found in ${fileA}` : `Type '${typeB}' not found in ${fileB}`
  };
  console.log(JSON.stringify(result));
  process.exit(1);
}

// Check assignability
const isAssignable = typeObjA.isAssignableTo(typeObjB);

// Return the result as JSON
const result = {
  isAssignable,
  typeA: {
    name: typeA,
    text: typeObjA.getText(),
  },
  typeB: {
    name: typeB,
    text: typeObjB.getText(),
  }
};

console.log(JSON.stringify(result));