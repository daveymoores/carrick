import * as fs from 'fs';
import * as path from 'path';

interface TypeMismatch {
  endpoint: string;
  producerType: string;
  consumerType: string;
  error: string;
  isCompatible: boolean;
}

interface TypeCheckResult {
  mismatches: TypeMismatch[];
  compatibleCount: number;
  totalChecked: number;
}

class SimpleTypeChecker {
  private typeDefinitions: Map<string, string> = new Map();

  constructor() {
    this.loadTypeDefinitions();
  }

  private loadTypeDefinitions() {
    const outputDir = path.join(__dirname, 'output');
    const typeFiles = fs.readdirSync(outputDir)
      .filter(file => file.endsWith('_types.ts'))
      .map(file => path.join(outputDir, file));

    for (const filePath of typeFiles) {
      const content = fs.readFileSync(filePath, 'utf8');
      this.parseTypeDefinitions(content);
    }
  }

  private parseTypeDefinitions(content: string) {
    // Extract interface definitions
    const interfaceRegex = /export interface (\w+)\s*\{([^}]+)\}/g;
    let match;
    while ((match = interfaceRegex.exec(content)) !== null) {
      const [, name, body] = match;
      this.typeDefinitions.set(name, this.normalizeTypeBody(body));
    }

    // Extract type alias definitions
    const typeAliasRegex = /export type (\w+)\s*=\s*([^;]+);/g;
    while ((match = typeAliasRegex.exec(content)) !== null) {
      const [, name, definition] = match;
      this.typeDefinitions.set(name, this.normalizeTypeDefinition(definition));
    }
  }

  private normalizeTypeBody(body: string): string {
    return body
      .split('\n')
      .map(line => line.trim())
      .filter(line => line.length > 0)
      .sort()
      .join('\n');
  }

  private normalizeTypeDefinition(definition: string): string {
    return definition.trim();
  }

  private resolveType(typeName: string): string {
    // Handle array types
    if (typeName.endsWith('[]')) {
      const elementType = typeName.slice(0, -2);
      const resolvedElement = this.resolveType(elementType);
      return resolvedElement ? `${resolvedElement}[]` : typeName;
    }

    // Direct lookup
    if (this.typeDefinitions.has(typeName)) {
      return this.typeDefinitions.get(typeName)!;
    }

    // Handle primitive types
    if (['string', 'number', 'boolean', 'any'].includes(typeName)) {
      return typeName;
    }

    return typeName; // Return as-is if not found
  }

  private areTypesCompatible(producerType: string, consumerType: string): boolean {
    const resolvedProducer = this.resolveType(producerType);
    const resolvedConsumer = this.resolveType(consumerType);

    // Exact match
    if (resolvedProducer === resolvedConsumer) {
      return true;
    }

    // Any type is compatible with everything
    if (resolvedProducer === 'any' || resolvedConsumer === 'any') {
      return true;
    }

    // Array compatibility
    if (resolvedProducer.endsWith('[]') && resolvedConsumer.endsWith('[]')) {
      const producerElement = resolvedProducer.slice(0, -2);
      const consumerElement = resolvedConsumer.slice(0, -2);
      return this.areTypesCompatible(producerElement, consumerElement);
    }

    // Basic structural comparison for object types
    if (resolvedProducer.includes('\n') && resolvedConsumer.includes('\n')) {
      return this.compareObjectTypes(resolvedProducer, resolvedConsumer);
    }

    return false;
  }

  private compareObjectTypes(producer: string, consumer: string): boolean {
    const producerProps = this.extractProperties(producer);
    const consumerProps = this.extractProperties(consumer);

    // Consumer properties must be satisfied by producer
    for (const [propName, propType] of consumerProps) {
      const producerPropType = producerProps.get(propName);
      if (!producerPropType) {
        return false; // Missing property
      }
      if (!this.areTypesCompatible(producerPropType, propType)) {
        return false; // Type mismatch
      }
    }

    return true;
  }

  private extractProperties(typeBody: string): Map<string, string> {
    const props = new Map<string, string>();
    const lines = typeBody.split('\n');
    
    for (const line of lines) {
      const propMatch = line.match(/(\w+)(\?)?\s*:\s*(.+);?/);
      if (propMatch) {
        const [, name, optional, type] = propMatch;
        props.set(name, type.trim());
      }
    }
    
    return props;
  }

  private getDetailedError(producerType: string, consumerType: string): string {
    const resolvedProducer = this.resolveType(producerType);
    const resolvedConsumer = this.resolveType(consumerType);

    if (!this.typeDefinitions.has(producerType) && !['string', 'number', 'boolean', 'any'].includes(producerType)) {
      return `Producer type '${producerType}' not found in type definitions`;
    }

    if (!this.typeDefinitions.has(consumerType) && !['string', 'number', 'boolean', 'any'].includes(consumerType)) {
      return `Consumer type '${consumerType}' not found in type definitions`;
    }

    if (resolvedProducer.includes('\n') && resolvedConsumer.includes('\n')) {
      const producerProps = this.extractProperties(resolvedProducer);
      const consumerProps = this.extractProperties(resolvedConsumer);
      
      const missingProps = [];
      const typeMismatches = [];
      
      for (const [propName, propType] of consumerProps) {
        const producerPropType = producerProps.get(propName);
        if (!producerPropType) {
          missingProps.push(propName);
        } else if (!this.areTypesCompatible(producerPropType, propType)) {
          typeMismatches.push(`${propName}: expected ${propType}, got ${producerPropType}`);
        }
      }
      
      const errors = [];
      if (missingProps.length > 0) {
        errors.push(`Missing properties: ${missingProps.join(', ')}`);
      }
      if (typeMismatches.length > 0) {
        errors.push(`Type mismatches: ${typeMismatches.join('; ')}`);
      }
      
      return errors.length > 0 ? errors.join('; ') : 'Structural type mismatch';
    }

    return `Type mismatch: expected ${consumerType}, got ${producerType}`;
  }

  public checkTypes(comparisons: Array<{endpoint: string, producerType: string, consumerType: string}>): TypeCheckResult {
    const mismatches: TypeMismatch[] = [];
    let compatibleCount = 0;

    for (const comparison of comparisons) {
      const isCompatible = this.areTypesCompatible(comparison.producerType, comparison.consumerType);

      if (isCompatible) {
        compatibleCount++;
      } else {
        const error = this.getDetailedError(comparison.producerType, comparison.consumerType);
        mismatches.push({
          endpoint: comparison.endpoint,
          producerType: comparison.producerType,
          consumerType: comparison.consumerType,
          error,
          isCompatible: false
        });
      }
    }

    return {
      mismatches,
      compatibleCount,
      totalChecked: comparisons.length
    };
  }
}

// CLI interface
if (require.main === module) {
  try {
    const checker = new SimpleTypeChecker();

    const input = process.argv[2];
    if (!input) {
      console.error('Usage: ts-node simple-type-checker.ts <comparisons-json-file>');
      process.exit(1);
    }

    const comparisons = JSON.parse(fs.readFileSync(input, 'utf8'));
    const result = checker.checkTypes(comparisons);

    console.log(JSON.stringify(result, null, 2));
    process.exit(0);
  } catch (error) {
    console.error('Error:', error);
    console.log(JSON.stringify({
      mismatches: [],
      compatibleCount: 0,
      totalChecked: 0,
      error: (error as Error).message
    }));
    process.exit(1);
  }
}

export { SimpleTypeChecker, TypeMismatch, TypeCheckResult };