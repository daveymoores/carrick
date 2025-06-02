import { Project, SourceFile } from "ts-morph";

export class ProjectManager {
  private project: Project;
  private sourceFileCache = new Map<string, SourceFile>();

  constructor(tsconfigPath?: string) {
    this.project = new Project({
      tsConfigFilePath: tsconfigPath,
      skipAddingFilesFromTsConfig: false,
    });
  }

  getProject(): Project {
    return this.project;
  }

  getSourceFile(filePath: string): SourceFile | undefined {
    if (this.sourceFileCache.has(filePath)) {
      return this.sourceFileCache.get(filePath);
    }

    const sf =
      this.project.getSourceFile(filePath) || 
      this.project.addSourceFileAtPath(filePath);
    
    if (sf) {
      this.sourceFileCache.set(filePath, sf);
    }
    return sf;
  }

  createOutputProject(): Project {
    return new Project();
  }

  clearCache(): void {
    this.sourceFileCache.clear();
  }
}

export function isLocalType(filePath: string): boolean {
  return !filePath.includes("node_modules");
}