// The hosted backend (training/hosted.mjs) against a real PostgreSQL. Runs when
// W3BS_TEST_DATABASE_URL is set (CI sets it); skipped otherwise.
import test from 'node:test';
import assert from 'node:assert/strict';
import { createHostedTraining, schemaFor } from '../training/hosted.mjs';
import { readZip } from '../training/core/zip.mjs';
import { sampleBundle, PNG_1X1 } from '../training/fixtures/sample.mjs';

const url = process.env.W3BS_TEST_DATABASE_URL;
const human = { kind: 'human', id: 'reviewer' };
const run = `${Date.now()}-${process.pid}`;
const MASTER = 'a'.repeat(64);

async function dropSchemas(users) {
  const { default: pg } = await import('pg');
  const c = new pg.Client({ connectionString: url });
  await c.connect();
  for (const u of users) await c.query(`DROP SCHEMA IF EXISTS "${schemaFor(u)}" CASCADE`);
  await c.end();
}

test(
  'hosted workspaces: full lifecycle on PostgreSQL, isolated per account',
  { skip: !url },
  async () => {
    const alice = `alice-${run}`;
    const bob = `bob-${run}`;
    const hosted = createHostedTraining({ databaseUrl: url, masterKey: MASTER });
    try {
      const t = await hosted.forUser(alice);
      assert.equal((await t.status()).key_store, 'server-managed');
      const imp = await t.importFile({ data: sampleBundle(), filename: 'a.zip' }, human);
      const { episodes } = await t.commitImport(imp.import_id, human);
      await t.review(episodes[0], { decision: 'complete', confirm_goal: true }, human);
      const ds = await t.createDataset({ name: 'Release workflows' }, human);
      await t.addEpisodes(ds.dataset_id, episodes, human);
      const v = await t.createVersion(ds.dataset_id, {}, human);
      assert.equal(v.readiness.admissible_by_purpose.private_retrieval, 1);
      await t.approve(ds.dataset_id, 1, ['private_retrieval', 'model_training'], human);
      await t.release(ds.dataset_id, 1, human);
      await t.grant({ consumer: 'claude-code', dataset_id: ds.dataset_id, version: 1 }, human);
      const r = await t.retrieve(
        { query: 'release announcement' },
        { kind: 'agent', id: 'claude-code' },
      );
      assert.equal(r.episodes.length, 1, 'PostgreSQL full-text retrieval');
      // No word matches: the newest granted task comes back, labelled as such.
      const loose = await t.retrieve({ query: 'kubernetes' }, { kind: 'agent', id: 'claude-code' });
      assert.equal(loose.match, 'recent');
      assert.equal(loose.episodes.length, 1);
      const exp = await t.export(
        ds.dataset_id,
        1,
        { format: 'next-action', download: true },
        human,
      );
      const files = readZip(exp.data);
      assert.ok(files.get('examples.jsonl').toString().trim().split('\n').length === 3);
      const image = [...files.keys()].find((n) => n.startsWith('images/'));
      assert.ok(files.get(image).equals(PNG_1X1), 'artifact decrypts back to the original PNG');
      assert.equal((await t.status()).artifact_bytes > 0, true);

      // Another account has its own, empty workspace and cannot see Alice's grant.
      const b = await hosted.forUser(bob);
      assert.equal((await b.listEpisodes()).length, 0);
      await assert.rejects(
        b.retrieve({ query: 'release' }, { kind: 'agent', id: 'claude-code' }),
        /SCOPE_DENIED|No active retrieval grant/,
      );

      // Survives a restart of every pool (nothing is held in memory).
      await hosted.close();
      const again = createHostedTraining({ databaseUrl: url, masterKey: MASTER });
      const t2 = await again.forUser(alice);
      assert.equal((await t2.listEpisodes()).length, 1);
      const report = await t2.forget({ from: '2026-10-15T15:00:00Z' }, human);
      assert.equal(report.removed.artifacts, 4);
      assert.equal((await t2.status()).artifact_bytes, 0);
      await again.close();
    } finally {
      await hosted.close();
      await dropSchemas([alice, bob]);
    }
  },
);

test('hosting refuses to start a workspace without a master key', { skip: !url }, async () => {
  const hosted = createHostedTraining({ databaseUrl: url, masterKey: '' });
  assert.equal(hosted.configured, false);
  await assert.rejects(hosted.forUser('nobody'), /not configured/);
  await hosted.close();
});
