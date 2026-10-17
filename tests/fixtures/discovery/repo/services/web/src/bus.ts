import { fork, type ChildProcess } from "node:child_process";

export class Bus {
  private child: ChildProcess = fork("./worker.js");

  emit(name: string, data: unknown): void {
    this.child.send({ name, data });
  }
}
