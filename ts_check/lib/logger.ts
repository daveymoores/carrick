export enum LogLevel {
  ERROR = 0,
  WARN = 1,
  INFO = 2,
  DEBUG = 3,
}

export interface LoggerConfig {
  level: LogLevel;
  enableColors: boolean;
  enableTimestamps: boolean;
  prefix?: string;
}

export class Logger {
  private config: LoggerConfig;

  constructor(config: Partial<LoggerConfig> = {}) {
    this.config = {
      level: LogLevel.INFO,
      enableColors: true,
      enableTimestamps: false,
      ...config,
    };
  }

  private formatMessage(level: string, message: string, ...args: any[]): string {
    const parts: string[] = [];

    if (this.config.enableTimestamps) {
      parts.push(`[${new Date().toISOString()}]`);
    }

    if (this.config.prefix) {
      parts.push(`[${this.config.prefix}]`);
    }

    parts.push(`[${level}]`);
    parts.push(message);

    return parts.join(' ') + (args.length > 0 ? ' ' + args.map(arg => 
      typeof arg === 'object' ? JSON.stringify(arg, null, 2) : String(arg)
    ).join(' ') : '');
  }

  private colorize(text: string, color: string): string {
    if (!this.config.enableColors) return text;
    
    const colors: Record<string, string> = {
      red: '\x1b[31m',
      yellow: '\x1b[33m',
      blue: '\x1b[34m',
      gray: '\x1b[90m',
      reset: '\x1b[0m',
    };

    return `${colors[color] || ''}${text}${colors.reset}`;
  }

  error(message: string, ...args: any[]): void {
    if (this.config.level >= LogLevel.ERROR) {
      const formatted = this.formatMessage('ERROR', message, ...args);
      console.error(this.colorize(formatted, 'red'));
    }
  }

  warn(message: string, ...args: any[]): void {
    if (this.config.level >= LogLevel.WARN) {
      const formatted = this.formatMessage('WARN', message, ...args);
      console.warn(this.colorize(formatted, 'yellow'));
    }
  }

  info(message: string, ...args: any[]): void {
    if (this.config.level >= LogLevel.INFO) {
      const formatted = this.formatMessage('INFO', message, ...args);
      console.log(this.colorize(formatted, 'blue'));
    }
  }

  debug(message: string, ...args: any[]): void {
    if (this.config.level >= LogLevel.DEBUG) {
      const formatted = this.formatMessage('DEBUG', message, ...args);
      console.log(this.colorize(formatted, 'gray'));
    }
  }

  setLevel(level: LogLevel): void {
    this.config.level = level;
  }

  setPrefix(prefix: string): void {
    this.config.prefix = prefix;
  }

  enableColors(enable: boolean): void {
    this.config.enableColors = enable;
  }

  enableTimestamps(enable: boolean): void {
    this.config.enableTimestamps = enable;
  }
}

export const createLogger = (config?: Partial<LoggerConfig>): Logger => {
  return new Logger(config);
};

export const defaultLogger = createLogger();