export class ApiClient {
  constructor(private readonly projectId: string) {}

  async list(): Promise<string[]> {
    return [this.projectId];
  }

  async remove(id: string): Promise<void> {
    void id;
  }
}
