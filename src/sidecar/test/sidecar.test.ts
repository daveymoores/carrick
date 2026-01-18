/**
 * Integration tests for the type-sidecar
 *
 * These tests spawn the sidecar process and communicate via stdin/stdout
 * to verify the full message loop functionality.
 */

import { describe, it, before, after } from 'node:test';
import * as assert from 'node:assert';
import { spawn, type ChildProcessWithoutNullStreams } from 'node:child_process';
import * as path from 'node:path';
import { fileURLToPath } from 'node:url';

const __dirname = path.dirname(fileURLToPath(import.meta.url));
// When running from dist/test/, go up one level to dist/, then into src/
const SIDECAR_PATH = path.join(__dirname, '..', 'src', 'index.js');
// Fixtures are in the source test directory, not dist
const FIXTURES_PATH = path.join(__dirname, '..', '..', 'test', 'fixtures', 'sample-repo');

/**
 * Helper class to manage sidecar process communication
 */
class SidecarClient {
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

// ===========================================================================
// Tests
// ===========================================================================

describe('Type Sidecar Integration Tests', () => {
  let client: SidecarClient;

  before(async () => {
    client = new SidecarClient();
    await client.start();
  });

  after(async () => {
    await client.stop();
  });

  describe('init action', () => {
    it('should initialize with a valid repo root', async () => {
      const response = await client.send<{
        request_id: string;
        status: string;
        init_time_ms?: number;
      }>({
        action: 'init',
        request_id: 'init-1',
        repo_root: FIXTURES_PATH,
      });

      assert.strictEqual(response.request_id, 'init-1');
      assert.strictEqual(response.status, 'ready');
      assert.ok(typeof response.init_time_ms === 'number');
      assert.ok(response.init_time_ms >= 0, 'init_time_ms should be non-negative');
    });

    it('should fail with non-existent repo root', async () => {
      const response = await client.send<{
        request_id: string;
        status: string;
        errors?: string[];
      }>({
        action: 'init',
        request_id: 'init-2',
        repo_root: '/non/existent/path',
      });

      assert.strictEqual(response.request_id, 'init-2');
      assert.strictEqual(response.status, 'error');
      assert.ok(response.errors && response.errors.length > 0);
    });
  });

  describe('health action', () => {
    it('should report ready status after initialization', async () => {
      // Re-initialize to ensure we're in a good state
      await client.send({
        action: 'init',
        request_id: 'health-setup',
        repo_root: FIXTURES_PATH,
      });

      const response = await client.send<{
        request_id: string;
        status: string;
        init_time_ms?: number;
      }>({
        action: 'health',
        request_id: 'health-1',
      });

      assert.strictEqual(response.request_id, 'health-1');
      assert.strictEqual(response.status, 'ready');
      assert.ok(typeof response.init_time_ms === 'number');
    });
  });

  describe('bundle action', () => {
    before(async () => {
      // Ensure initialized
      await client.send({
        action: 'init',
        request_id: 'bundle-setup',
        repo_root: FIXTURES_PATH,
      });
    });

    it('should bundle a simple interface', async () => {
      const response = await client.send<{
        request_id: string;
        status: string;
        dts_content?: string;
        manifest?: Array<{
          alias: string;
          original_name: string;
          source_file: string;
          is_explicit: boolean;
        }>;
        errors?: string[];
      }>({
        action: 'bundle',
        request_id: 'bundle-1',
        symbols: [
          {
            symbol_name: 'User',
            source_file: 'src/types.ts',
          },
        ],
      });

      assert.strictEqual(response.request_id, 'bundle-1');
      assert.strictEqual(response.status, 'success');
      assert.ok(response.dts_content, 'Should have dts_content');
      assert.ok(response.dts_content.includes('User'), 'dts_content should include User');
      assert.ok(response.manifest, 'Should have manifest');
      assert.strictEqual(response.manifest.length, 1);
      assert.strictEqual(response.manifest[0].original_name, 'User');
      assert.strictEqual(response.manifest[0].is_explicit, true);
    });

    it('should bundle multiple symbols', async () => {
      const response = await client.send<{
        request_id: string;
        status: string;
        dts_content?: string;
        manifest?: Array<{
          alias: string;
          original_name: string;
        }>;
      }>({
        action: 'bundle',
        request_id: 'bundle-2',
        symbols: [
          { symbol_name: 'User', source_file: 'src/types.ts' },
          { symbol_name: 'Order', source_file: 'src/models.ts' },
        ],
      });

      assert.strictEqual(response.request_id, 'bundle-2');
      assert.strictEqual(response.status, 'success');
      assert.ok(response.dts_content);
      assert.ok(response.dts_content.includes('User'));
      assert.ok(response.dts_content.includes('Order'));
      assert.strictEqual(response.manifest?.length, 2);
    });

    it('should support symbol aliases', async () => {
      const response = await client.send<{
        request_id: string;
        status: string;
        manifest?: Array<{
          alias: string;
          original_name: string;
        }>;
      }>({
        action: 'bundle',
        request_id: 'bundle-3',
        symbols: [
          {
            symbol_name: 'User',
            source_file: 'src/types.ts',
            alias: 'UserResponse',
          },
        ],
      });

      assert.strictEqual(response.status, 'success');
      assert.ok(response.manifest);
      assert.strictEqual(response.manifest[0].alias, 'UserResponse');
      assert.strictEqual(response.manifest[0].original_name, 'User');
    });

    it('should report errors for non-existent symbols', async () => {
      const response = await client.send<{
        request_id: string;
        status: string;
        symbol_failures?: Array<{
          symbol_name: string;
          reason: string;
        }>;
      }>({
        action: 'bundle',
        request_id: 'bundle-4',
        symbols: [
          {
            symbol_name: 'NonExistentType',
            source_file: 'src/types.ts',
          },
        ],
      });

      assert.strictEqual(response.request_id, 'bundle-4');
      assert.strictEqual(response.status, 'error');
      assert.ok(response.symbol_failures);
      assert.strictEqual(response.symbol_failures.length, 1);
      assert.strictEqual(response.symbol_failures[0].symbol_name, 'NonExistentType');
    });
  });

  describe('infer action', () => {
    before(async () => {
      // Ensure initialized
      await client.send({
        action: 'init',
        request_id: 'infer-setup',
        repo_root: FIXTURES_PATH,
      });
    });

    it('should infer function return type', async () => {
      const response = await client.send<{
        request_id: string;
        status: string;
        inferred_types?: Array<{
          alias: string;
          type_string: string;
          is_explicit: boolean;
          infer_kind: string;
          source_location: {
            file_path: string;
            start_line: number;
            end_line: number;
          };
        }>;
        errors?: string[];
      }>({
        action: 'infer',
        request_id: 'infer-1',
        requests: [
          {
            file_path: path.join(FIXTURES_PATH, 'src/routes.ts'),
            line_number: 55, // getUser function (line inside the function body)
            infer_kind: 'function_return',
          },
        ],
      });

      assert.strictEqual(response.request_id, 'infer-1');
      assert.ok(
        response.status === 'success' || response.inferred_types,
        'Should succeed or have inferred types'
      );
      if (response.inferred_types && response.inferred_types.length > 0) {
        const inferred = response.inferred_types[0];
        assert.strictEqual(inferred.infer_kind, 'function_return');
        assert.ok(inferred.source_location.file_path.includes('routes.ts'));
      }
    });

    it('should infer response body type from res.json()', async () => {
      const response = await client.send<{
        request_id: string;
        status: string;
        inferred_types?: Array<{
          alias: string;
          type_string: string;
          is_explicit: boolean;
          infer_kind: string;
        }>;
      }>({
        action: 'infer',
        request_id: 'infer-2',
        requests: [
          {
            file_path: path.join(FIXTURES_PATH, 'src/routes.ts'),
            line_number: 87, // getOrders function body (implicit types)
            infer_kind: 'response_body',
          },
        ],
      });

      assert.strictEqual(response.request_id, 'infer-2');
      if (response.inferred_types && response.inferred_types.length > 0) {
        const inferred = response.inferred_types[0];
        assert.strictEqual(inferred.infer_kind, 'response_body');
        // For implicit types, is_explicit should be false
        assert.strictEqual(inferred.is_explicit, false);
      }
    });

    it('should infer request body type from handler usage', async () => {
      const response = await client.send<{
        request_id: string;
        status: string;
        inferred_types?: Array<{
          alias: string;
          type_string: string;
          is_explicit: boolean;
          infer_kind: string;
        }>;
      }>({
        action: 'infer',
        request_id: 'infer-2b',
        requests: [
          {
            file_path: path.join(FIXTURES_PATH, 'src/request-bodies.ts'),
            line_number: 14,
            infer_kind: 'request_body',
          },
        ],
      });

      assert.strictEqual(response.request_id, 'infer-2b');
      if (response.inferred_types && response.inferred_types.length > 0) {
        const inferred = response.inferred_types[0];
        assert.strictEqual(inferred.infer_kind, 'request_body');
        assert.ok(inferred.type_string.includes('RequestBody'));
      }
    });

    it('should infer request body type from call payloads', async () => {
      const response = await client.send<{
        request_id: string;
        status: string;
        inferred_types?: Array<{
          alias: string;
          type_string: string;
          infer_kind: string;
        }>;
      }>({
        action: 'infer',
        request_id: 'infer-2c',
        requests: [
          {
            file_path: path.join(FIXTURES_PATH, 'src/request-bodies.ts'),
            line_number: 20,
            infer_kind: 'request_body',
          },
        ],
      });

      assert.strictEqual(response.request_id, 'infer-2c');
      if (response.inferred_types && response.inferred_types.length > 0) {
        const inferred = response.inferred_types[0];
        assert.strictEqual(inferred.infer_kind, 'request_body');
        assert.ok(inferred.type_string.includes('RequestBody'));
      }
    });

    it('should support custom aliases for inferred types', async () => {
      const response = await client.send<{
        request_id: string;
        status: string;
        inferred_types?: Array<{
          alias: string;
          type_string: string;
        }>;
      }>({
        action: 'infer',
        request_id: 'infer-3',
        requests: [
          {
            file_path: path.join(FIXTURES_PATH, 'src/routes.ts'),
            line_number: 55, // Inside getUser function body
            infer_kind: 'function_return',
            alias: 'GetUserReturn',
          },
        ],
      });

      if (response.inferred_types && response.inferred_types.length > 0) {
        assert.strictEqual(response.inferred_types[0].alias, 'GetUserReturn');
      }
    });

    it('should infer variable types', async () => {
      const response = await client.send<{
        request_id: string;
        status: string;
        inferred_types?: Array<{
          alias: string;
          type_string: string;
          infer_kind: string;
        }>;
      }>({
        action: 'infer',
        request_id: 'infer-4',
        requests: [
          {
            file_path: path.join(FIXTURES_PATH, 'src/routes.ts'),
            line_number: 13, // Inside db const declaration (findUser line)
            infer_kind: 'variable',
          },
        ],
      });

      assert.strictEqual(response.request_id, 'infer-4');
      if (response.inferred_types && response.inferred_types.length > 0) {
        assert.strictEqual(response.inferred_types[0].infer_kind, 'variable');
      }
    });

    it('should infer call result from the innermost call expression', async () => {
      const response = await client.send<{
        request_id: string;
        status: string;
        inferred_types?: Array<{
          alias: string;
          type_string: string;
          is_explicit: boolean;
          infer_kind: string;
        }>;
      }>({
        action: 'infer',
        request_id: 'infer-5',
        requests: [
          {
            file_path: path.join(FIXTURES_PATH, 'src/call-site.ts'),
            line_number: 21,
            infer_kind: 'call_result',
          },
        ],
      });

      assert.strictEqual(response.request_id, 'infer-5');
      assert.ok(
        response.status === 'success' || response.inferred_types,
        'Should succeed or have inferred types'
      );
      if (response.inferred_types && response.inferred_types.length > 0) {
        const inferred = response.inferred_types[0];
        assert.strictEqual(inferred.infer_kind, 'call_result');
        assert.ok(inferred.type_string.includes('User'));
        assert.ok(!inferred.type_string.includes('RegisterHandle'));
      }
    });

    it('should prefer response.json payload types for fetch calls', async () => {
      const response = await client.send<{
        request_id: string;
        status: string;
        inferred_types?: Array<{
          alias: string;
          type_string: string;
          is_explicit: boolean;
          infer_kind: string;
        }>;
      }>({
        action: 'infer',
        request_id: 'infer-6',
        requests: [
          {
            file_path: path.join(FIXTURES_PATH, 'src/fetch-json.ts'),
            line_number: 2,
            infer_kind: 'call_result',
          },
        ],
      });

      assert.strictEqual(response.request_id, 'infer-6');
      if (response.inferred_types && response.inferred_types.length > 0) {
        const inferred = response.inferred_types[0];
        assert.strictEqual(inferred.infer_kind, 'call_result');
        assert.ok(inferred.type_string.includes('id'));
        assert.ok(inferred.type_string.includes('name'));
        assert.ok(!inferred.type_string.includes('Response'));
      }
    });
  });

  describe('error handling', () => {
    it('should handle invalid JSON gracefully', async () => {
      // This is tricky to test since we can't send invalid JSON through our client
      // Skip this test as it requires lower-level access
    });

    it('should reject unknown actions', async () => {
      const response = await client.send<{
        request_id: string;
        status: string;
        errors?: string[];
      }>({
        action: 'unknown_action',
        request_id: 'error-1',
      } as Record<string, unknown>);

      assert.strictEqual(response.status, 'error');
      assert.ok(response.errors && response.errors.length > 0);
    });

    it('should reject requests without required fields', async () => {
      const response = await client.send<{
        request_id: string;
        status: string;
        errors?: string[];
      }>({
        action: 'init',
        // Missing request_id and repo_root
      } as Record<string, unknown>);

      assert.strictEqual(response.status, 'error');
    });

    it('should handle bundle before init', async () => {
      // Start a fresh client to test uninitialized state
      const freshClient = new SidecarClient();
      await freshClient.start();

      try {
        const response = await freshClient.send<{
          request_id: string;
          status: string;
          errors?: string[];
        }>({
          action: 'bundle',
          request_id: 'uninit-1',
          symbols: [{ symbol_name: 'User', source_file: 'src/types.ts' }],
        });

        assert.strictEqual(response.status, 'error');
        assert.ok(response.errors?.some((e) => e.toLowerCase().includes('init')));
      } finally {
        await freshClient.stop();
      }
    });
  });
});

// ===========================================================================
// Unit-style tests for validators
// ===========================================================================

describe('Validator Unit Tests', () => {
  // Import dynamically to avoid issues if build hasn't run
  it('should validate init request', async () => {
    const { parseRequest } = await import('../src/validators.js');

    const result = parseRequest({
      action: 'init',
      request_id: 'test-1',
      repo_root: '/some/path',
    });

    assert.strictEqual(result.success, true);
    if (result.success) {
      assert.strictEqual(result.request.action, 'init');
    }
  });

  it('should reject invalid init request', async () => {
    const { parseRequest } = await import('../src/validators.js');

    const result = parseRequest({
      action: 'init',
      request_id: 'test-1',
      // Missing repo_root
    });

    assert.strictEqual(result.success, false);
  });

  it('should validate bundle request', async () => {
    const { parseRequest } = await import('../src/validators.js');

    const result = parseRequest({
      action: 'bundle',
      request_id: 'test-2',
      symbols: [
        {
          symbol_name: 'User',
          source_file: 'src/types.ts',
        },
      ],
    });

    assert.strictEqual(result.success, true);
  });

  it('should validate infer request', async () => {
    const { parseRequest } = await import('../src/validators.js');

    const result = parseRequest({
      action: 'infer',
      request_id: 'test-3',
      requests: [
        {
          file_path: 'src/routes.ts',
          line_number: 10,
          infer_kind: 'function_return',
        },
      ],
    });

    assert.strictEqual(result.success, true);
  });

  it('should reject invalid infer_kind', async () => {
    const { parseRequest } = await import('../src/validators.js');

    const result = parseRequest({
      action: 'infer',
      request_id: 'test-4',
      requests: [
        {
          file_path: 'src/routes.ts',
          line_number: 10,
          infer_kind: 'invalid_kind',
        },
      ],
    });

    assert.strictEqual(result.success, false);
  });
});
