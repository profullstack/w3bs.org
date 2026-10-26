// Talking to Infernet (and c0mpute) as subprocesses. W3BS never imports Python or a model
// runtime: training is `infernet model train ... --json` (or c0mpute's remote equivalent) and
// inference is the runtime contract spoken over `infernet model serve --stdio` (NDJSON,
// protocol 1: load / open_session / predict / reset / close_session / kill_switch / shutdown).
//
// Binaries come from PATH, or from W3BS_INFERNET_BIN / W3BS_C0MPUTE_BIN. A value with spaces
// is a command line ("node /path/to/infernet/apps/cli/index.js"); a .js/.mjs path runs under
// the current JavaScript runtime. Tests point these at fake binaries.
import { spawn } from 'node:child_process';
import { existsSync } from 'node:fs';
import { delimiter, join } from 'node:path';
import { ERROR_CODES, RUNTIME_PROTOCOL_VERSION } from './contract.mjs';

export class LiquidRuntimeError extends Error {
  constructor({ code = 'E_RUNTIME', message, retriable, status = 502, ...extra }) {
    super(message);
    this.code = code;
    this.retriable = retriable ?? ERROR_CODES[code]?.retriable ?? false;
    this.status = status;
    Object.assign(this, extra);
  }
}

const BIN_ENV = { infernet: 'W3BS_INFERNET_BIN', c0mpute: 'W3BS_C0MPUTE_BIN' };
const INSTALL = {
  infernet:
    'Install the Infernet CLI (https://infernetprotocol.com) and run `infernet model runtime setup --yes`, or set W3BS_INFERNET_BIN.',
  c0mpute: 'Install c0mpute (https://github.com/profullstack/c0mpute), or set W3BS_C0MPUTE_BIN.',
};

export function resolveBin(name, env = process.env) {
  const configured = env[BIN_ENV[name]];
  if (configured) {
    const parts = configured.trim().split(/\s+/);
    if (parts.length === 1 && /\.(mjs|js)$/.test(parts[0])) return [process.execPath, parts[0]];
    return parts;
  }
  const exts = process.platform === 'win32' ? ['.exe', '.cmd', ''] : [''];
  for (const dir of (env.PATH ?? '').split(delimiter))
    for (const ext of exts)
      if (dir && existsSync(join(dir, name + ext))) return [join(dir, name + ext)];
  return null;
}

export function requireBin(name, env) {
  const argv = resolveBin(name, env);
  if (!argv)
    throw new LiquidRuntimeError({
      code: 'E_RUNTIME_MISSING',
      status: 409,
      message: `${name} was not found on PATH. ${INSTALL[name]}`,
    });
  return argv;
}

/**
 * Run a command that prints one JSON document on stdout. Resolves { json, code, stderr }.
 * Non-zero exit rejects with the tail of stderr (Infernet keeps dataset content out of it).
 */
export function runJson(
  argv,
  args,
  { env = process.env, timeoutMs = 6 * 3600 * 1000, onSpawn } = {},
) {
  return new Promise((resolve, reject) => {
    const child = spawn(argv[0], [...argv.slice(1), ...args], {
      stdio: ['ignore', 'pipe', 'pipe'],
      env,
    });
    onSpawn?.(child);
    let out = '';
    let err = '';
    child.stdout.setEncoding('utf8').on('data', (d) => (out += d));
    child.stderr.setEncoding('utf8').on('data', (d) => (err = (err + d).slice(-8192)));
    const timer = setTimeout(() => {
      child.kill('SIGTERM');
      reject(new LiquidRuntimeError({ code: 'E_TIMEOUT', message: `${argv.join(' ')} timed out` }));
    }, timeoutMs);
    child.on('error', (e) => {
      clearTimeout(timer);
      reject(
        new LiquidRuntimeError({ code: 'E_RUNTIME_MISSING', status: 409, message: e.message }),
      );
    });
    child.on('exit', (code, signal) => {
      clearTimeout(timer);
      if (code !== 0) {
        const tail = err.trim().split('\n').slice(-4).join(' | ');
        return reject(
          new LiquidRuntimeError({
            code: signal ? 'E_CANCELLED' : 'E_RUNTIME',
            message: `${argv[argv.length - 1].split('/').pop()} ${args.slice(0, 3).join(' ')} exited ${code ?? signal}${tail ? `: ${tail}` : ''}`,
            exit_code: code,
          }),
        );
      }
      try {
        resolve({ json: JSON.parse(out), code, stderr: err });
      } catch {
        reject(
          new LiquidRuntimeError({
            code: 'E_RUNTIME',
            message: 'the command did not print JSON on stdout',
          }),
        );
      }
    });
  });
}

/**
 * One `infernet model serve --stdio` process: the runtime contract of the Liquid Agent.
 * request(op, params) resolves with the result or rejects with LiquidRuntimeError carrying
 * the contract's error code (E_SEQ_DUPLICATE, E_SESSION_UNKNOWN, ...).
 */
export class PolicyRuntime {
  constructor({ env = process.env, argv } = {}) {
    const bin = argv ?? requireBin('infernet', env);
    this.child = spawn(bin[0], [...bin.slice(1), 'model', 'serve', '--stdio'], {
      stdio: ['pipe', 'pipe', 'pipe'],
      env,
    });
    this.pending = new Map();
    this.seq = 0;
    this.buf = '';
    this.stderr = '';
    this.closed = false;
    this.child.stdout.setEncoding('utf8').on('data', (chunk) => this.#onData(chunk));
    this.child.stderr.on('data', (b) => (this.stderr = (this.stderr + b.toString()).slice(-4096)));
    const failAll = (error) => {
      this.closed = true;
      for (const p of this.pending.values()) {
        clearTimeout(p.timer);
        p.reject(error);
      }
      this.pending.clear();
    };
    this.child.on('exit', (code) =>
      failAll(
        new LiquidRuntimeError({
          code: 'E_RUNTIME',
          message: `infernet model serve exited ${code}`,
        }),
      ),
    );
    this.child.on('error', (e) =>
      failAll(
        new LiquidRuntimeError({ code: 'E_RUNTIME_MISSING', status: 409, message: e.message }),
      ),
    );
    this.child.stdin.on('error', () => {});
  }

  #onData(chunk) {
    this.buf += chunk;
    let i;
    while ((i = this.buf.indexOf('\n')) >= 0) {
      const line = this.buf.slice(0, i).trim();
      this.buf = this.buf.slice(i + 1);
      if (!line) continue;
      let msg;
      try {
        msg = JSON.parse(line);
      } catch {
        continue;
      }
      const p = this.pending.get(msg.id);
      if (!p || msg.event) continue;
      this.pending.delete(msg.id);
      clearTimeout(p.timer);
      if (msg.ok) p.resolve(msg.result);
      else p.reject(new LiquidRuntimeError(msg.error ?? { message: 'unknown runtime error' }));
    }
  }

  request(op, params = {}, { timeoutMs = 120000 } = {}) {
    if (this.closed)
      return Promise.reject(
        new LiquidRuntimeError({ message: 'the policy runtime is not running' }),
      );
    const id = `w${++this.seq}`;
    return new Promise((resolve, reject) => {
      const timer = setTimeout(() => {
        this.pending.delete(id);
        reject(
          new LiquidRuntimeError({
            code: 'E_TIMEOUT',
            message: `${op} timed out after ${timeoutMs} ms`,
          }),
        );
      }, timeoutMs);
      this.pending.set(id, { resolve, reject, timer });
      this.child.stdin.write(
        `${JSON.stringify({ v: RUNTIME_PROTOCOL_VERSION, id, op, ...params })}\n`,
      );
    });
  }

  load(modelDir, expectedHash, abstainThreshold = 0.5) {
    return this.request('load', {
      model_dir: modelDir,
      expected_hash: expectedHash,
      abstain_threshold: abstainThreshold,
    });
  }
  openSession(modelHash, sessionId, { ttl_s = 600, account_scope } = {}) {
    return this.request('open_session', {
      model_hash: modelHash,
      session_id: sessionId,
      ttl_s,
      account_scope,
    });
  }
  predict(request, account_scope) {
    return this.request('predict', { request, account_scope }, { timeoutMs: 30000 });
  }
  reset(sessionId, account_scope) {
    return this.request('reset', { session_id: sessionId, account_scope });
  }
  closeSession(sessionId, account_scope) {
    return this.request('close_session', { session_id: sessionId, account_scope });
  }
  killSwitch(on = true) {
    return this.request('kill_switch', { on });
  }

  async close() {
    if (this.closed) return;
    try {
      await this.request('shutdown', {}, { timeoutMs: 5000 });
    } catch {
      /* fall through to kill */
    }
    if (!this.closed) this.child.kill('SIGTERM');
  }
}
