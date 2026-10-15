// Local Training store: one SQLite file plus sealed artifact files.
//
// Records are kept as validated JSON bodies with the columns needed for
// authorization joins, review queues, retention and deletion lineage.
// The hosted backend (Postgres, M2) implements the same tables.
import { chmodSync, existsSync, mkdirSync } from 'node:fs';
import { homedir } from 'node:os';
import { resolve } from 'node:path';

export const TRAINING_SCHEMA = `
PRAGMA journal_mode=WAL; PRAGMA busy_timeout=5000; PRAGMA foreign_keys=ON;
CREATE TABLE IF NOT EXISTS meta (key TEXT PRIMARY KEY, value TEXT NOT NULL);
CREATE TABLE IF NOT EXISTS imports (
  import_id TEXT PRIMARY KEY, status TEXT NOT NULL, kind TEXT NOT NULL, filename TEXT,
  received_at TEXT NOT NULL, committed_at TEXT, actor TEXT NOT NULL, preview TEXT, error TEXT);
CREATE TABLE IF NOT EXISTS sessions (
  session_id TEXT PRIMARY KEY, import_id TEXT, started_at TEXT NOT NULL, profile TEXT NOT NULL, body TEXT NOT NULL);
CREATE TABLE IF NOT EXISTS artifacts (
  artifact_id TEXT PRIMARY KEY, session_id TEXT NOT NULL, media_type TEXT NOT NULL, bytes INTEGER NOT NULL,
  captured_at TEXT NOT NULL, body TEXT NOT NULL);
CREATE TABLE IF NOT EXISTS observations (
  observation_id TEXT PRIMARY KEY, session_id TEXT NOT NULL, monotonic_ns TEXT NOT NULL,
  captured_at TEXT NOT NULL, artifact_id TEXT, body TEXT NOT NULL);
CREATE INDEX IF NOT EXISTS observations_session ON observations(session_id);
CREATE INDEX IF NOT EXISTS observations_time ON observations(captured_at);
CREATE TABLE IF NOT EXISTS steps (
  step_id TEXT PRIMARY KEY, session_id TEXT NOT NULL, sequence INTEGER NOT NULL, monotonic_ns TEXT NOT NULL,
  captured_at TEXT NOT NULL, body TEXT NOT NULL);
CREATE INDEX IF NOT EXISTS steps_session ON steps(session_id, sequence);
CREATE INDEX IF NOT EXISTS steps_time ON steps(captured_at);
CREATE TABLE IF NOT EXISTS episodes (
  episode_id TEXT PRIMARY KEY, import_id TEXT, state TEXT NOT NULL, profile TEXT NOT NULL,
  family TEXT, captured_from TEXT NOT NULL, captured_to TEXT NOT NULL, rights TEXT NOT NULL,
  pinned INTEGER NOT NULL DEFAULT 0, created_at TEXT NOT NULL, updated_at TEXT NOT NULL,
  revision INTEGER NOT NULL DEFAULT 1, body TEXT NOT NULL);
CREATE INDEX IF NOT EXISTS episodes_state ON episodes(state);
CREATE TABLE IF NOT EXISTS labels (
  revision_id TEXT PRIMARY KEY, subject_kind TEXT NOT NULL, subject_id TEXT NOT NULL, field_path TEXT NOT NULL,
  status TEXT NOT NULL, source TEXT NOT NULL, created_at TEXT NOT NULL, body TEXT NOT NULL);
CREATE INDEX IF NOT EXISTS labels_subject ON labels(subject_id, field_path);
CREATE TABLE IF NOT EXISTS warnings (
  warning_id INTEGER PRIMARY KEY AUTOINCREMENT, episode_id TEXT NOT NULL, kind TEXT NOT NULL, path TEXT,
  blocking INTEGER NOT NULL, resolved_at TEXT, resolved_by TEXT);
CREATE INDEX IF NOT EXISTS warnings_episode ON warnings(episode_id);
CREATE TABLE IF NOT EXISTS datasets (
  dataset_id TEXT PRIMARY KEY, name TEXT NOT NULL, metadata TEXT NOT NULL, created_at TEXT NOT NULL, updated_at TEXT NOT NULL);
CREATE TABLE IF NOT EXISTS dataset_members (
  dataset_id TEXT NOT NULL REFERENCES datasets(dataset_id) ON DELETE CASCADE, episode_id TEXT NOT NULL,
  added_at TEXT NOT NULL, PRIMARY KEY (dataset_id, episode_id));
CREATE TABLE IF NOT EXISTS dataset_versions (
  dataset_id TEXT NOT NULL REFERENCES datasets(dataset_id) ON DELETE CASCADE, version INTEGER NOT NULL,
  state TEXT NOT NULL, digest TEXT NOT NULL, created_at TEXT NOT NULL, released_at TEXT, revoked_at TEXT,
  body TEXT NOT NULL, PRIMARY KEY (dataset_id, version));
CREATE TABLE IF NOT EXISTS version_episodes (
  dataset_id TEXT NOT NULL, version INTEGER NOT NULL, episode_id TEXT NOT NULL, split TEXT NOT NULL,
  body TEXT NOT NULL, PRIMARY KEY (dataset_id, version, episode_id),
  FOREIGN KEY (dataset_id, version) REFERENCES dataset_versions(dataset_id, version) ON DELETE CASCADE);
CREATE INDEX IF NOT EXISTS version_episodes_episode ON version_episodes(episode_id);
CREATE TABLE IF NOT EXISTS version_purposes (
  dataset_id TEXT NOT NULL, version INTEGER NOT NULL, purpose TEXT NOT NULL, approved_by TEXT NOT NULL,
  approved_at TEXT NOT NULL, admitted INTEGER NOT NULL, PRIMARY KEY (dataset_id, version, purpose),
  FOREIGN KEY (dataset_id, version) REFERENCES dataset_versions(dataset_id, version) ON DELETE CASCADE);
CREATE TABLE IF NOT EXISTS grants (
  grant_id TEXT PRIMARY KEY, consumer TEXT NOT NULL, dataset_id TEXT NOT NULL, version INTEGER NOT NULL,
  purpose TEXT NOT NULL, created_by TEXT NOT NULL, created_at TEXT NOT NULL, expires_at TEXT NOT NULL, revoked_at TEXT);
CREATE INDEX IF NOT EXISTS grants_consumer ON grants(consumer);
CREATE TABLE IF NOT EXISTS runs (
  run_id TEXT PRIMARY KEY, consumer TEXT NOT NULL, kind TEXT NOT NULL, outcome TEXT NOT NULL,
  review_state TEXT NOT NULL, created_at TEXT NOT NULL, body TEXT NOT NULL);
CREATE TABLE IF NOT EXISTS exports (
  export_id TEXT PRIMARY KEY, dataset_id TEXT NOT NULL, version INTEGER NOT NULL, format TEXT NOT NULL,
  purpose TEXT NOT NULL, destination TEXT NOT NULL, created_by TEXT NOT NULL, created_at TEXT NOT NULL,
  episodes TEXT NOT NULL, body TEXT NOT NULL);
CREATE TABLE IF NOT EXISTS workflows (
  workflow_id TEXT PRIMARY KEY, status TEXT NOT NULL, created_at TEXT NOT NULL, sources TEXT NOT NULL, body TEXT NOT NULL);
CREATE TABLE IF NOT EXISTS approval_requests (
  request_id TEXT PRIMARY KEY, requested_by TEXT NOT NULL, kind TEXT NOT NULL, payload TEXT NOT NULL,
  status TEXT NOT NULL, created_at TEXT NOT NULL, decided_at TEXT, decided_by TEXT);
CREATE TABLE IF NOT EXISTS deletion_requests (
  request_id TEXT PRIMARY KEY, created_at TEXT NOT NULL, actor TEXT NOT NULL, scope TEXT NOT NULL, report TEXT NOT NULL);
CREATE TABLE IF NOT EXISTS tombstones (
  subject_id TEXT PRIMARY KEY, kind TEXT NOT NULL, deleted_at TEXT NOT NULL, request_id TEXT NOT NULL);
CREATE TABLE IF NOT EXISTS audit (
  sequence INTEGER PRIMARY KEY AUTOINCREMENT, at TEXT NOT NULL, actor TEXT NOT NULL, operation TEXT NOT NULL,
  subject TEXT, detail TEXT);
CREATE VIRTUAL TABLE IF NOT EXISTS episode_fts USING fts5(episode_id UNINDEXED, goal, family, steps, apps);
`;

export function defaultHome(env = process.env) {
  if (env.W3BS_TRAINING_HOME) return resolve(env.W3BS_TRAINING_HOME);
  if (process.platform === 'win32' && env.LOCALAPPDATA) return resolve(env.LOCALAPPDATA, 'w3bs', 'training');
  const data = env.XDG_DATA_HOME || resolve(homedir(), '.local/share');
  return resolve(data, 'w3bs/training');
}

export async function openTrainingDb(directory) {
  const { DatabaseSync } = await import('node:sqlite');
  mkdirSync(resolve(directory, 'artifacts'), { recursive: true, mode: 0o700 });
  mkdirSync(resolve(directory, 'quarantine'), { recursive: true, mode: 0o700 });
  const path = resolve(directory, 'training.sqlite');
  const fresh = !existsSync(path);
  const db = new DatabaseSync(path);
  if (fresh) chmodSync(path, 0o600);
  db.exec(TRAINING_SCHEMA);
  const all = (sql, params = []) => db.prepare(sql).all(...params);
  const get = (sql, params = []) => db.prepare(sql).get(...params);
  const run = (sql, params = []) => db.prepare(sql).run(...params);
  let depth = 0;
  return {
    all,
    get,
    run,
    path,
    // Nested calls join the outer transaction.
    transaction(fn) {
      if (depth > 0) return fn();
      db.exec('BEGIN IMMEDIATE');
      depth += 1;
      try {
        const result = fn();
        db.exec('COMMIT');
        return result;
      } catch (error) {
        db.exec('ROLLBACK');
        throw error;
      } finally {
        depth -= 1;
      }
    },
    close: () => db.close(),
  };
}
