// Contract between the Rust recorder (desktop/crates/adl-broker/src/training_bundle.rs) and the
// JavaScript importer: the golden bundle the Rust tests pin must import cleanly here.
import test from 'node:test';
import assert from 'node:assert/strict';
import { mkdtempSync, readFileSync, rmSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { spawnSync } from 'node:child_process';
import { openTraining } from '../training/core/service.mjs';

const GOLDEN = 'training/fixtures/recorder/teach.w3bs-training.zip';
const human = { kind: 'human', id: 'reviewer' };

test('the recorder golden bundle imports as a visual-review teach episode', async () => {
  const home = mkdtempSync(join(tmpdir(), 'w3bs-rec-'));
  try {
    const t = await openTraining({ home, env: { W3BS_TRAINING_PASSPHRASE: 'p' } });
    const imp = await t.importFile(
      { data: readFileSync(GOLDEN), filename: 'teach.w3bs-training.zip' },
      human,
    );
    assert.equal(imp.preview.capture_profile, 'visual-review');
    assert.deepEqual(
      [
        imp.preview.counts.episodes,
        imp.preview.counts.observations,
        imp.preview.counts.actions,
        imp.preview.counts.artifacts,
      ],
      [1, 3, 3, 3],
    );
    assert.equal(imp.preview.gaps, 1);
    const { episodes } = await t.commitImport(imp.import_id, human);
    const e = await t.getEpisode(episodes[0]);
    assert.equal(e.episode.goal.value, 'Prepare a release-announcement draft');
    assert.equal(
      e.episode.goal.status,
      'proposed',
      're-proposed on import; the person confirms in review',
    );
    assert.equal(e.episode.explanation.author, 'human');
    assert.ok(
      e.steps.every(
        (s) =>
          s.action.type === 'focus' &&
          s.action.origin === 'unknown' &&
          s.alignment.status === 'uncertain',
      ),
    );
    assert.equal(e.gaps[0].kind, 'excluded_scope');
    // Polled frames never qualify for next-action supervision.
    await t.review(episodes[0], { decision: 'complete', confirm_goal: true }, human);
    const ds = await t.createDataset({ name: 'd' }, human);
    await t.addEpisodes(ds.dataset_id, episodes, human);
    const v = await t.createVersion(ds.dataset_id, {}, human);
    assert.equal(v.readiness.profiles['action-supervision'].eligible_steps, 0);
    t.close();
  } finally {
    rmSync(home, { recursive: true, force: true });
  }
});

test('w3bs training import - reads a bundle from stdin', () => {
  const home = mkdtempSync(join(tmpdir(), 'w3bs-rec-'));
  try {
    const run = (args, input) =>
      spawnSync(process.execPath, ['training/bin/w3bs-training.mjs', ...args], {
        input,
        encoding: 'utf8',
        env: { ...process.env, W3BS_TRAINING_HOME: home, W3BS_TRAINING_PASSPHRASE: 'p' },
      });
    const missing = run(['import', '-'], readFileSync(GOLDEN));
    assert.equal(JSON.parse(missing.stderr).error.code, 'MISSING_OPTION');
    const ok = run(
      ['import', '-', '--filename', 'ses_rec1.w3bs-training.zip'],
      readFileSync(GOLDEN),
    );
    assert.equal(ok.status, 0, ok.stderr);
    assert.equal(JSON.parse(ok.stdout).status, 'committed');
  } finally {
    rmSync(home, { recursive: true, force: true });
  }
});
