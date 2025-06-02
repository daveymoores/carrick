import { Node, SourceFile, ts } from "ts-morph";

export function findTypeReferenceAtPosition(
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

export function findTypeDeclarationByPosition(
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

export function getModuleSpecifierFromNodeModulesPath(
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