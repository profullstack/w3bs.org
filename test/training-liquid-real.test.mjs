// Opt-in: the Liquid Agent against the REAL Infernet runtime (PyTorch + ncps CfC/GRU).
// Skipped unless INFERNET_LIQUID_PYTHON points at a Python with the runtime's lock installed
// and the Infernet CLI resolves (W3BS_INFERNET_BIN, e.g. "node /path/infernet/apps/cli/index.js",
// or `infernet` on PATH). Run it capped:
//   INFERNET_LIQUID_PYTHON=... W3BS_INFERNET_BIN="node .../apps/cli/index.js" \
//     systemd-run --user --scope -p MemoryMax=4G bun test test/training-liquid-real.test.mjs
import test from 'node:test';
import assert from 'node:assert/strict';
import { mkdtempSync, rmSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { openTraining } from '../training/core/service.mjs';
import { resolveBin } from '../training/liquid/runtime.mjs';
import { TASKS } from '../training/liquid/mock.mjs';

const enabled = !!process.env.INFERNET_LIQUID_PYTHON && !!resolveBin('infernet', process.env);
const human = { kind: 'human', id: 'reviewer' };

test(
  'real Infernet: mock episodes (30, or W3BS_LIQUID_DEMOS) -> train CfC + GRU -> evaluate on the mock sites -> suggest',
  { skip: !enabled && 'set INFERNET_LIQUID_PYTHON and W3BS_INFERNET_BIN', timeout: 1800000 },
  async () => {
    const home = mkdtempSync(join(tmpdir(), 'w3bs-liquid-real-'));
    const t = await openTraining({
      home,
      env: { ...process.env, W3BS_TRAINING_PASSPHRASE: 'real-runtime-test' },
    });
    const L = t.liquid;
    try {
      for (const p of ['recording', 'training_data']) await L.setPermission(p, true, human);
      const demo = await L.recordMockDemonstrations(
        { count: Number(process.env.W3BS_LIQUID_DEMOS ?? 30), seed: 500 },
        human,
      );
      for (const e of demo.episodes)
        await L.review(e.episode_id, { success: true, reviewed: true }, human);
      const ds = await L.exportDataset({ name: 'real runtime' }, human);
      const job = await L.waitJob(
        (
          await L.train(
            { dataset_id: ds.dataset_id, epochs: Number(process.env.W3BS_LIQUID_EPOCHS ?? 25) },
            human,
          )
        ).job_id,
      );
      assert.equal(job.state, 'succeeded', JSON.stringify(job.error));
      assert.deepEqual(job.models.map((m) => m.family).sort(), ['cfc', 'gru']);
      for (const m of job.models) assert.match(m.model_hash, /^sha256:[0-9a-f]{64}$/);
      const report = {};
      for (const m of job.models) {
        const ev = await L.evaluate({ model_id: m.model_id, per_workflow: 3 }, human);
        for (const task of TASKS)
          assert.equal(ev.workflows[task].external_writes_without_approval, 0);
        report[m.family] = {
          selected: m.selected,
          train_metrics: m.metrics,
          pass: ev.pass,
          workflows: ev.workflows,
          layout_variant: ev.layout_variant,
        };
      }
      process.stderr.write(
        `\nreal runtime report (selected ${job.selected}):\n${JSON.stringify(report, null, 2)}\n`,
      );
      const best = job.models.find((m) => m.selected);
      if (report[best.family].pass) {
        await L.activate(best.model_id, {}, human);
        await L.setMode('email.reply_draft', 'suggest', human);
        const run = await L.waitRun(
          (
            await L.startRun({ task: 'email.reply_draft', seed: 77 }, human, {
              approver: async () => ({ approved: false }),
              person: async (_o, p) => p.action,
            })
          ).run_id,
        );
        assert.equal(run.state, 'finished', JSON.stringify(run.error));
        assert.equal(run.report.executed.length, 0, 'Suggest never executes');
      }
    } finally {
      await t.close();
      rmSync(home, { recursive: true, force: true });
    }
  },
);
