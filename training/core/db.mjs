// Storage adapters for the Training service: an async SQL interface over SQLite (the local
// store) or PostgreSQL (w3bs.org/training, one schema per workspace), plus a blob store for
// sealed artifacts and quarantined imports.
//
// The interface: all(sql, params) -> rows, get(sql, params) -> row, run(sql, params) ->
// { changes }, transaction(async fn) -> fn's result, close(). SQL uses `?` placeholders.
// Calls made anywhere inside a transaction's async scope join that transaction (tracked with
// AsyncLocalStorage), so service code never threads a handle through.
import { AsyncLocalStorage } from 'node:async_hooks';
import {
  chmodSync,
  existsSync,
  mkdirSync,
  readFileSync,
  readdirSync,
  rmSync,
  statSync,
  writeFileSync,
} from 'node:fs';
import { dirname, resolve, sep } from 'node:path';
import { TRAINING_SCHEMA, TRAINING_SCHEMA_PG } from './store.mjs';

// A FIFO lock: SQLite has one connection, so a transaction must not interleave with other
// statements at its await points.
function mutex() {
  let tail = Promise.resolve();
  return (fn) => {
    const run = tail.then(fn, fn);
    tail = run.then(
      () => undefined,
      () => undefined,
    );
    return run;
  };
}

export async function openSqliteDb(path) {
  const { DatabaseSync } = await import('node:sqlite');
  if (path !== ':memory:') mkdirSync(dirname(path), { recursive: true, mode: 0o700 });
  const fresh = path !== ':memory:' && !existsSync(path);
  const raw = new DatabaseSync(path);
  if (fresh) chmodSync(path, 0o600);
  raw.exec(TRAINING_SCHEMA);
  const als = new AsyncLocalStorage();
  const locked = mutex();
  const exec = {
    all: (sql, params = []) => raw.prepare(sql).all(...params),
    get: (sql, params = []) => raw.prepare(sql).get(...params),
    run: (sql, params = []) => ({ changes: Number(raw.prepare(sql).run(...params).changes) }),
  };
  const call = (name) => async (sql, params) =>
    als.getStore() ? exec[name](sql, params) : locked(() => exec[name](sql, params));
  return {
    dialect: 'sqlite',
    path,
    all: call('all'),
    get: call('get'),
    run: call('run'),
    async transaction(fn) {
      if (als.getStore()) return fn();
      return locked(async () => {
        raw.exec('BEGIN IMMEDIATE');
        try {
          const result = await als.run(true, fn);
          raw.exec('COMMIT');
          return result;
        } catch (error) {
          raw.exec('ROLLBACK');
          throw error;
        }
      });
    },
    close: async () => raw.close(),
  };
}

const positional = (sql) => {
  let index = 0;
  return sql.replaceAll('?', () => `$${++index}`);
};
// COUNT(*) and BIGSERIAL come back as int8; parse them as numbers for this adapter only.
const TYPES = {
  getTypeParser(oid, format) {
    return oid === 20 ? (v) => (v === null ? null : Number(v)) : pgTypes.getTypeParser(oid, format);
  },
};
let pgTypes;

// `pool` must already be bound to the workspace schema (search_path in its connection options),
// see openWorkspaceDb in training/hosted.mjs.
export async function openPostgresDb(pool, { migrate = true } = {}) {
  pgTypes ??= (await import('pg')).default.types;
  if (migrate) await pool.query(TRAINING_SCHEMA_PG);
  const als = new AsyncLocalStorage();
  const target = () => als.getStore() ?? pool;
  const q = async (sql, params = []) =>
    target().query({ text: positional(sql), values: params, types: TYPES });
  return {
    dialect: 'postgres',
    all: async (sql, params) => (await q(sql, params)).rows,
    get: async (sql, params) => (await q(sql, params)).rows[0],
    run: async (sql, params) => ({ changes: (await q(sql, params)).rowCount ?? 0 }),
    async transaction(fn) {
      if (als.getStore()) return fn();
      const client = await pool.connect();
      try {
        await client.query('BEGIN');
        const result = await als.run(client, fn);
        await client.query('COMMIT');
        return result;
      } catch (error) {
        await client.query('ROLLBACK').catch(() => {});
        throw error;
      } finally {
        client.release();
      }
    },
    close: () => pool.end(),
  };
}

// ------------------------------------------------------------------ blobs
// Keys look like paths: artifacts/<id>.bin, quarantine/<import>/stage.bin. Values are already
// sealed by the service; blob stores never see plaintext.
const safeKey = (key) => {
  if (!/^[A-Za-z0-9_-]+(\/[A-Za-z0-9._-]+)*$/.test(key) || key.split('/').includes('..'))
    throw new Error(`bad blob key ${key}`);
  return key;
};

export function fsBlobs(home) {
  const at = (key) => resolve(home, safeKey(key));
  return {
    async put(key, data) {
      mkdirSync(dirname(at(key)), { recursive: true, mode: 0o700 });
      writeFileSync(at(key), data, { mode: 0o600 });
    },
    async get(key) {
      return existsSync(at(key)) ? readFileSync(at(key)) : null;
    },
    async has(key) {
      return existsSync(at(key));
    },
    async del(key) {
      rmSync(at(key), { force: true });
    },
    async delPrefix(prefix) {
      rmSync(at(prefix.replace(/\/$/, '')), { recursive: true, force: true });
    },
    async totalBytes(prefix) {
      const dir = at(prefix.replace(/\/$/, ''));
      if (!existsSync(dir)) return 0;
      let n = 0;
      for (const name of readdirSync(dir)) n += statSync(dir + sep + name).size;
      return n;
    },
  };
}

export function sqlBlobs(db) {
  return {
    async put(key, data) {
      await db.run(
        'INSERT INTO blobs (key, bytes, data) VALUES (?, ?, ?) ON CONFLICT (key) DO UPDATE SET bytes = excluded.bytes, data = excluded.data',
        [safeKey(key), data.length, data],
      );
    },
    async get(key) {
      const row = await db.get('SELECT data FROM blobs WHERE key = ?', [safeKey(key)]);
      return row ? Buffer.from(row.data) : null;
    },
    async has(key) {
      return !!(await db.get('SELECT 1 AS x FROM blobs WHERE key = ?', [safeKey(key)]));
    },
    async del(key) {
      await db.run('DELETE FROM blobs WHERE key = ?', [safeKey(key)]);
    },
    async delPrefix(prefix) {
      await db.run('DELETE FROM blobs WHERE key LIKE ?', [
        `${safeKey(prefix.replace(/\/$/, ''))}/%`,
      ]);
    },
    async totalBytes(prefix) {
      const row = await db.get('SELECT COALESCE(SUM(bytes), 0) AS n FROM blobs WHERE key LIKE ?', [
        `${safeKey(prefix.replace(/\/$/, ''))}/%`,
      ]);
      return Number(row?.n ?? 0);
    },
  };
}
