import { Node, TypeNode, Type, NamedTupleMember, RestTypeNode } from "ts-morph";
import {
  MAX_PROPERTY_DEPTH,
  MAX_PROPERTIES_LIMIT,
  isNodeModulesPath,
} from "./constants";

export class TypeProcessor {
  private seenNodesForRecursion = new Set<Node>();
  private seenTypeObjects = new WeakSet<Type>();

  constructor(
    private collectDeclarationsRecursively: (
      node: Node | undefined,
      isRoot?: boolean,
    ) => void,
  ) {}

  processTypeNode(typeNode: TypeNode | undefined): void {
    if (!typeNode || this.seenNodesForRecursion.has(typeNode)) {
      return;
    }
    this.seenNodesForRecursion.add(typeNode);

    try {
      // Handle TypeReference nodes (e.g., MyType, AnotherType<Arg>)
      if (Node.isTypeReference(typeNode)) {
        const nameNode = typeNode.getTypeName();
        const symbol = nameNode.getSymbol();

        if (symbol) {
          symbol
            .getDeclarations()
            .forEach((decl) => this.collectDeclarationsRecursively(decl));
        }

        typeNode
          .getTypeArguments()
          .forEach((argTypeNode) => this.processTypeNode(argTypeNode));
      }
      // Handle Union and Intersection types
      else if (
        Node.isUnionTypeNode(typeNode) ||
        Node.isIntersectionTypeNode(typeNode)
      ) {
        typeNode.getTypeNodes().forEach((tn) => this.processTypeNode(tn));
      }
      // Handle Array types
      else if (Node.isArrayTypeNode(typeNode)) {
        this.processTypeNode(typeNode.getElementTypeNode());
      }
      // Handle Tuple types
      else if (Node.isTupleTypeNode(typeNode)) {
        typeNode.getElements().forEach((element) => {
          if (Node.isTypeNode(element)) {
            this.processTypeNode(element);
          } else if (Node.isNamedTupleMember(element)) {
            const memberTypeNode = (element as NamedTupleMember).getTypeNode();
            if (memberTypeNode) this.processTypeNode(memberTypeNode);
          } else if (Node.isRestTypeNode(element)) {
            const restTypeNode = (element as RestTypeNode).getTypeNode();
            if (restTypeNode) this.processTypeNode(restTypeNode);
          } else {
            console.warn(
              `Unexpected tuple element type: ${(element as TypeNode).getKindName()}`,
            );
            (element as TypeNode)
              .getChildren()
              .filter(Node.isTypeNode)
              .forEach((childTypeNode) => this.processTypeNode(childTypeNode));
          }
        });
      }
      // Handle Mapped types
      else if (Node.isMappedTypeNode(typeNode)) {
        const typeParameter = typeNode.getTypeParameter();
        const constraint = typeParameter.getConstraint();
        if (constraint) {
          this.processTypeNode(constraint);
        }

        const nameType = typeNode.getNameTypeNode();
        if (nameType) {
          this.processTypeNode(nameType);
        }

        const valueTypeNode = typeNode.getTypeNode();
        if (valueTypeNode) {
          this.processTypeNode(valueTypeNode);
        }
      }
      // Handle parenthesized, type operator, and indexed access types
      else if (Node.isParenthesizedTypeNode(typeNode)) {
        this.processTypeNode(typeNode.getTypeNode());
      } else if (Node.isTypeOperatorTypeNode(typeNode)) {
        this.processTypeNode(typeNode.getTypeNode());
      } else if (Node.isIndexedAccessTypeNode(typeNode)) {
        this.processTypeNode(typeNode.getObjectTypeNode());
        this.processTypeNode(typeNode.getIndexTypeNode());
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
              this.collectDeclarationsRecursively(decl);
            }
          });
        }
      }
      // Handle import type nodes: import('./types').MyType
      else if (Node.isImportTypeNode(typeNode)) {
        try {
          const qualifier = typeNode.getQualifier();

          if (qualifier) {
            if (
              Node.isIdentifier(qualifier) ||
              Node.isQualifiedName(qualifier)
            ) {
              const symbol = qualifier.getSymbol?.();
              if (symbol) {
                symbol
                  .getDeclarations()
                  .forEach((decl) => this.collectDeclarationsRecursively(decl));
              }
            }
          }

          typeNode.getTypeArguments?.()?.forEach((argType) => {
            this.processTypeNode(argType);
          });

          try {
            const resolvedType = typeNode.getType();
            if (resolvedType) {
              this.collectFromTypeObject(resolvedType, typeNode);
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
        this.processTypeNode(typeNode.getCheckType());
        this.processTypeNode(typeNode.getExtendsType());
        this.processTypeNode(typeNode.getTrueType());
        this.processTypeNode(typeNode.getFalseType());
      }
      // Handle infer types (infer T)
      else if (Node.isInferTypeNode(typeNode)) {
        const typeParameter = typeNode.getTypeParameter();
        const constraint = typeParameter.getConstraint();
        if (constraint) this.processTypeNode(constraint);

        const defaultType = typeParameter.getDefault();
        if (defaultType) this.processTypeNode(defaultType);
      }
      // Handle template literal types (`foo${Bar}`)
      else if (Node.isTemplateLiteralTypeNode(typeNode)) {
        const spans = typeNode.getTemplateSpans();
        spans.forEach((span) => {
          this.processTypeNode(span);
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
              .forEach((decl) => this.collectDeclarationsRecursively(decl));
          }
        }
      }
      // Fallback: look for identifiers that might be types
      typeNode.forEachDescendant((descendant) => {
        if (Node.isIdentifier(descendant)) {
          const parent = descendant.getParent();

          if (
            parent &&
            (Node.isIdentifier(parent) || Node.isQualifiedName(parent)) &&
            parent.getParent() &&
            Node.isTypeReference(parent.getParent())
          ) {
            return;
          }

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
            return;
          }

          const symbol = descendant.getSymbol();
          if (symbol) {
            symbol.getDeclarations().forEach((decl) => {
              this.collectDeclarationsRecursively(decl);
            });
          }
        }
      });
    } catch (error) {
      console.warn(`Error processing type node: ${error}`);
      console.warn(`Type node text: ${typeNode.getText()}`);
    }
  }

  collectFromTypeObject(type: Type | undefined, contextNode: Node): void {
    if (!type || this.seenTypeObjects.has(type)) return;
    this.seenTypeObjects.add(type);

    try {
      const symbol = type.getAliasSymbol() || type.getSymbol();
      if (symbol) {
        for (const decl of symbol.getDeclarations()) {
          this.collectDeclarationsRecursively(decl);
        }
      }

      type
        .getTypeArguments()
        .forEach((argType) => this.collectFromTypeObject(argType, contextNode));

      if (type.isUnion()) {
        type
          .getUnionTypes()
          .forEach((ut) => this.collectFromTypeObject(ut, contextNode));
      }
      if (type.isIntersection()) {
        type
          .getIntersectionTypes()
          .forEach((it) => this.collectFromTypeObject(it, contextNode));
      }

      if (type.isArray())
        this.collectFromTypeObject(type.getArrayElementType(), contextNode);
      if (type.isTuple())
        type
          .getTupleElements()
          .forEach((te) => this.collectFromTypeObject(te, contextNode));

      this.traversePropertiesWithDepthLimit(
        type,
        MAX_PROPERTY_DEPTH,
        contextNode,
      );
    } catch (error) {
      console.warn(`Error processing type: ${error}`);
    }
  }

  processTypeArgument(typeArg: TypeNode): void {
    console.log(
      `processTypeNode: ${typeArg.getKindName()} - ${typeArg.getText()}`,
    );

    if (Node.isTypeReference(typeArg)) {
      const argTypeName = typeArg.getTypeName().getText();
      const argSymbol = typeArg.getTypeName().getSymbol();

      if (argSymbol) {
        for (const argDecl of argSymbol.getDeclarations()) {
          const isNodeModule = isNodeModulesPath(
            argDecl.getSourceFile().getFilePath(),
          );
          if (isNodeModule) {
            console.log(`External type argument: ${argTypeName}`);
          } else {
            console.log(`Found local type argument: ${argTypeName}`);
            this.collectDeclarationsRecursively(argDecl, true);
          }
        }
      }

      for (const innerArg of typeArg.getTypeArguments()) {
        this.processTypeArgument(innerArg);
      }
    } else if (Node.isUnionTypeNode(typeArg)) {
      console.log(`Processing union type argument: ${typeArg.getText()}`);
      typeArg.getTypeNodes().forEach((unionMember) => {
        console.log(
          `  Union member: ${unionMember.getKindName()} - ${unionMember.getText()}`,
        );
        this.processTypeArgument(unionMember); // Recursively process each union member
      });
    } else if (Node.isIntersectionTypeNode(typeArg)) {
      console.log(
        `Processing intersection type argument: ${typeArg.getText()}`,
      );
      typeArg.getTypeNodes().forEach((intersectionMember) => {
        console.log(
          `  Intersection member: ${intersectionMember.getKindName()} - ${intersectionMember.getText()}`,
        );
        this.processTypeArgument(intersectionMember); // Use processTypeArgument, not processTypeNode
      });
    } else if (Node.isArrayTypeNode(typeArg)) {
      this.processTypeArgument(typeArg.getElementTypeNode());
    } else if (Node.isParenthesizedTypeNode(typeArg)) {
      console.log(`Processing parenthesized type: ${typeArg.getText()}`);
      this.processTypeArgument(typeArg.getTypeNode()); // Use processTypeArgument, not processTypeNode
    } else if (Node.isTypeLiteral(typeArg)) {
      console.log(`Processing type literal: ${typeArg.getText()}`);
      typeArg.getProperties().forEach((prop) => {
        if (Node.isPropertySignature(prop)) {
          const propTypeNode = prop.getTypeNode();
          if (propTypeNode) {
            this.processTypeArgument(propTypeNode); // Use processTypeArgument consistently
          }
        }
      });
    } else {
      console.log(
        `Delegating complex type argument to main processor: ${typeArg.getKindName()}`,
      );
      this.processTypeNode(typeArg);
    }
  }

  private traversePropertiesWithDepthLimit(
    type: Type,
    depthRemaining: number,
    contextNode: Node,
  ): void {
    if (depthRemaining <= 0) {
      console.warn("Maximum property traversal depth reached");
      return;
    }

    try {
      const properties = type.getProperties();
      if (properties.length > MAX_PROPERTIES_LIMIT) {
        console.warn(
          `Type has ${properties.length} properties, limiting to avoid overflow`,
        );
        properties.slice(0, MAX_PROPERTIES_LIMIT).forEach((prop) => {
          try {
            const propType = prop.getTypeAtLocation(contextNode as Node);

            const symbol = propType.getAliasSymbol() || propType.getSymbol();
            if (symbol) {
              symbol
                .getDeclarations()
                .forEach((decl) => this.collectDeclarationsRecursively(decl));
            }

            if (depthRemaining > 1) {
              this.collectFromTypeObject(propType, contextNode);
            }
          } catch (propError) {
            console.warn(`Error processing property: ${propError}`);
          }
        });
      } else {
        properties.forEach((prop) => {
          try {
            const propType = prop.getTypeAtLocation(contextNode as Node);
            this.collectFromTypeObject(propType, contextNode);
          } catch (propError) {
            console.warn(`Error processing property: ${propError}`);
          }
        });
      }
    } catch (error) {
      console.warn(`Error traversing properties: ${error}`);
    }
  }

  clear(): void {
    this.seenNodesForRecursion.clear();
  }
}
