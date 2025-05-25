export class ExtractorError extends Error {
  constructor(
    message: string,
    public readonly code: string,
    public readonly context?: any
  ) {
    super(message);
    this.name = 'ExtractorError';
  }
}

export class FileNotFoundError extends ExtractorError {
  constructor(filePath: string) {
    super(`File not found: ${filePath}`, 'FILE_NOT_FOUND', { filePath });
  }
}

export class TypeNotFoundError extends ExtractorError {
  constructor(filePath: string, position: number) {
    super(
      `Type not found at position ${position} in file ${filePath}`,
      'TYPE_NOT_FOUND',
      { filePath, position }
    );
  }
}

export class ParseError extends ExtractorError {
  constructor(message: string, context?: any) {
    super(`Parse error: ${message}`, 'PARSE_ERROR', context);
  }
}

export class OutputError extends ExtractorError {
  constructor(message: string, outputPath?: string) {
    super(`Output error: ${message}`, 'OUTPUT_ERROR', { outputPath });
  }
}

export class ErrorHandler {
  private errors: ExtractorError[] = [];
  private warnings: string[] = [];

  addError(error: ExtractorError): void {
    this.errors.push(error);
  }

  addWarning(warning: string): void {
    this.warnings.push(warning);
  }

  createError(message: string, code: string, context?: any): ExtractorError {
    const error = new ExtractorError(message, code, context);
    this.addError(error);
    return error;
  }

  hasErrors(): boolean {
    return this.errors.length > 0;
  }

  hasWarnings(): boolean {
    return this.warnings.length > 0;
  }

  getErrors(): ExtractorError[] {
    return [...this.errors];
  }

  getWarnings(): string[] {
    return [...this.warnings];
  }

  clear(): void {
    this.errors = [];
    this.warnings = [];
  }

  handleError(error: unknown, context?: string): ExtractorError {
    let extractorError: ExtractorError;

    if (error instanceof ExtractorError) {
      extractorError = error;
    } else if (error instanceof Error) {
      extractorError = new ExtractorError(
        error.message,
        'UNKNOWN_ERROR',
        { originalError: error, context }
      );
    } else {
      extractorError = new ExtractorError(
        String(error),
        'UNKNOWN_ERROR',
        { originalError: error, context }
      );
    }

    this.addError(extractorError);
    return extractorError;
  }

  safeExecute<T>(
    fn: () => T,
    context?: string,
    defaultValue?: T
  ): T | undefined {
    try {
      return fn();
    } catch (error) {
      this.handleError(error, context);
      return defaultValue;
    }
  }

  async safeExecuteAsync<T>(
    fn: () => Promise<T>,
    context?: string,
    defaultValue?: T
  ): Promise<T | undefined> {
    try {
      return await fn();
    } catch (error) {
      this.handleError(error, context);
      return defaultValue;
    }
  }

  getErrorSummary(): string {
    const errorCount = this.errors.length;
    const warningCount = this.warnings.length;
    
    if (errorCount === 0 && warningCount === 0) {
      return 'No errors or warnings';
    }

    const parts: string[] = [];
    
    if (errorCount > 0) {
      parts.push(`${errorCount} error${errorCount === 1 ? '' : 's'}`);
    }
    
    if (warningCount > 0) {
      parts.push(`${warningCount} warning${warningCount === 1 ? '' : 's'}`);
    }

    return parts.join(', ');
  }

  logSummary(): void {
    if (this.hasErrors()) {
      console.error(`Errors encountered:`);
      this.errors.forEach((error, index) => {
        console.error(`  ${index + 1}. [${error.code}] ${error.message}`);
        if (error.context) {
          console.error(`     Context:`, error.context);
        }
      });
    }

    if (this.hasWarnings()) {
      console.warn(`Warnings:`);
      this.warnings.forEach((warning, index) => {
        console.warn(`  ${index + 1}. ${warning}`);
      });
    }
  }
}