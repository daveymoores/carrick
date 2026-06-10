/**
 * Shared test helpers: sidecar process client and fixture paths.
 */

import { spawn, type ChildProcessWithoutNullStreams } from 'node:child_process';
import * as path from 'node:path';
import { fileURLToPath } from 'node:url';

const __dirname = path.dirname(fileURLToPath(import.meta.url));
// When running from dist/test/, go up one level to dist/, then into src/
export const SIDECAR_PATH = path.join(__dirname, '..', 'src', 'index.js');
// Fixtures are in the source test directory, not dist
export const FIXTURES_PATH = path.join(
  __dirname,
  '..',
  '..',
  'test',
  'fixtures',
  'sample-repo'
);

/**
 * Helper class to manage sidecar process communication
 */
export class SidecarClient {
  private process: ChildProcessWithoutNullStreams | null = null;
  private responseBuffer: string = '';
  private responsePromises: Array<{
    resolve: (response: unknown) => void;
    reject: (error: Error) => void;
  }> = [];

  /**
   * Start the sidecar process
   */
  async start(): Promise<void> {
    return new Promise((resolve, reject) => {
      this.process = spawn('node', [SIDECAR_PATH], {
        stdio: ['pipe', 'pipe', 'pipe'],
      });

      // Handle stdout (JSON responses)
      this.process.stdout.on('data', (data: Buffer) => {
        this.responseBuffer += data.toString();

        // Process complete lines
        const lines = this.responseBuffer.split('\n');
        this.responseBuffer = lines.pop() || '';

        for (const line of lines) {
          if (line.trim()) {
            try {
              const response = JSON.parse(line);
              const promise = this.responsePromises.shift();
              if (promise) {
                promise.resolve(response);
              }
            } catch (err) {
              console.error('Failed to parse response:', line);
            }
          }
        }
      });

      // Handle stderr (logs)
      this.process.stderr.on('data', (data: Buffer) => {
        // Log to console for debugging
        const msg = data.toString().trim();
        if (msg) {
          console.error('[sidecar stderr]', msg);
        }
      });

      this.process.on('error', (err) => {
        reject(err);
      });

      // Give it a moment to start up
      setTimeout(resolve, 100);
    });
  }

  /**
   * Send a request and wait for response
   */
  async send<T = unknown>(request: Record<string, unknown>): Promise<T> {
    if (!this.process) {
      throw new Error('Sidecar not started');
    }

    return new Promise((resolve, reject) => {
      this.responsePromises.push({ resolve: resolve as (r: unknown) => void, reject });

      const json = JSON.stringify(request);
      this.process!.stdin.write(json + '\n');

      // Timeout after 10 seconds
      setTimeout(() => {
        const index = this.responsePromises.findIndex((p) => p.resolve === resolve);
        if (index !== -1) {
          this.responsePromises.splice(index, 1);
          reject(new Error('Request timeout'));
        }
      }, 10000);
    });
  }

  /**
   * Stop the sidecar process
   */
  async stop(): Promise<void> {
    if (this.process) {
      try {
        await this.send({ action: 'shutdown', request_id: 'shutdown' });
      } catch {
        // Ignore shutdown errors
      }
      this.process.kill();
      this.process = null;
    }
  }
}
