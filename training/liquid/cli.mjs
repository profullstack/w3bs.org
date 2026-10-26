// `w3bs training teach ...`: the Liquid Agent from a terminal. Granting, recording, reviewing,
// exporting, training, activating and approving need a person: an interactive terminal and a
// typed code (humanActor). Revoking a permission and throwing the kill switch never do.
import { createInterface } from 'node:readline/promises';
import { MODE_LABELS } from './executor.mjs';

export const TEACH_HELP = `  w3bs training teach status | audit
  w3bs training teach permissions [grant|revoke NAME]   recording, training_data, network_upload,
                                                       remote_inference, auto_execute (all off at first)
  w3bs training teach kill-switch on|off               stop every agent action at once
  w3bs training teach record --task T --app A [--window W] [--profile strict|standard] [--screenshots]
  w3bs training teach pause|resume|stop CAPTURE_ID [--discard]
  w3bs training teach demo [--count 20]                demonstrations on the mock sites, recorded
  w3bs training teach episodes [--state S] | show ID | review ID --success yes|no [--risk R]
                         [--goal-file F] [--failure-reason-file F] | redact ID --step N --field F
                         | unredact ID --redaction R | dedupe | import TRAINING_EPISODE_ID [--task T]
                         | accept ID | discard ID
  w3bs training teach dataset [--name N] [--retention-days N]
  w3bs training teach train DATASET_ID [--epochs N] [--worker PEER_ID --max-price USD [--public-url URL]]
  w3bs training teach jobs [JOB_ID] | models | evaluate MODEL_ID [--per-workflow N]
  w3bs training teach activate MODEL_ID [--task T] | deactivate TASK
  w3bs training teach mode TASK off|suggest|approve|bounded [--max-steps N]
  w3bs training teach run TASK [--seed N] [--variant N]   on a mock site; you answer each approval
`;

export async function runTeach(
  service,
  sub,
  rest,
  values,
  { humanActor, plainActor, out, fail, readText },
) {
  const L = service.liquid ?? fail('UNAVAILABLE', 'The Liquid Agent needs the local store.');
  const need = (value, what) => value ?? fail('MISSING_ARGUMENT', `${what} is required.`);
  const required = (name) => values[name] ?? fail('MISSING_OPTION', `--${name} is required.`);
  switch (sub ?? 'status') {
    case 'status':
      return out(await L.status());
    case 'audit':
      return out(await L.auditLog({ limit: 200 }));
    case 'permissions': {
      const [action, name] = rest;
      if (!action) return out(await L.permissions());
      if (action === 'revoke')
        return out(await L.setPermission(need(name, 'NAME'), false, plainActor()));
      if (action === 'grant') {
        const desc =
          L.PERMISSIONS[need(name, 'NAME')] ?? fail('INVALID_INPUT', `Unknown permission ${name}.`);
        return out(
          await L.setPermission(
            name,
            true,
            await humanActor(`Turn on "${name}": ${desc}.\nYou can revoke it at any time.`),
          ),
        );
      }
      return fail('UNKNOWN_COMMAND', 'teach permissions [grant|revoke NAME]');
    }
    case 'kill-switch': {
      const [state] = rest;
      if (state === 'on') return out(await L.setKillSwitch(true, plainActor()));
      if (state === 'off')
        return out(
          await L.setKillSwitch(
            false,
            await humanActor('Release the kill switch: agents may act again in the modes you set.'),
          ),
        );
      return fail('UNKNOWN_COMMAND', 'teach kill-switch on|off');
    }
    case 'record': {
      const task = required('task');
      const app = required('app');
      const actor = await humanActor(
        `Record ONE task (${task}) in ${app}${values.window ? ` / ${values.window}` : ''} only.\nProfile ${values.profile ?? 'strict'}: secret fields never carry text${values.profile === 'standard' ? '' : ', and no typed text is kept'}. Screenshots: ${values.screenshots ? 'hash references only' : 'none'}.\nStop with: w3bs training teach stop CAPTURE_ID`,
      );
      const result = await L.startCapture(
        {
          task,
          app,
          window: values.window,
          profile: values.profile,
          screenshots: !!values.screenshots,
        },
        actor,
      );
      process.stderr.write(`\n  ● RECORDING ${app} for ${task} (${result.capture_id})\n\n`);
      return out(result);
    }
    case 'pause':
      return out(await L.pauseCapture(need(rest[0], 'CAPTURE_ID'), plainActor()));
    case 'resume':
      return out(
        await L.resumeCapture(
          need(rest[0], 'CAPTURE_ID'),
          await humanActor(`Resume recording ${rest[0]}.`),
        ),
      );
    case 'stop':
      return out(
        await L.stopCapture(need(rest[0], 'CAPTURE_ID'), plainActor(), {
          discard: !!values.discard,
        }),
      );
    case 'demo': {
      const count = Number(values.count ?? 20);
      return out(
        await L.recordMockDemonstrations(
          { count },
          await humanActor(
            `Record ${count} demonstrations on the mock email, Reddit and LinkedIn sites (nothing real is touched).`,
          ),
        ),
      );
    }
    case 'episodes': {
      const [action, id] = rest;
      if (!action) return out(await L.listEpisodes({ state: values.state }));
      if (action === 'show') return out(await L.getEpisode(need(id, 'EPISODE_ID')));
      if (action === 'dedupe') return out(await L.dedupe(plainActor()));
      if (action === 'review') {
        const input = { reviewed: true, success: required('success') === 'yes' };
        if (values.risk) input.risk = values.risk;
        if (values['goal-file']) input.goal = readText(values['goal-file']);
        if (values['failure-reason-file'])
          input.failure_reason = readText(values['failure-reason-file']);
        return out(
          await L.review(
            need(id, 'EPISODE_ID'),
            input,
            await humanActor(`Mark ${id} reviewed (success: ${input.success ? 'yes' : 'no'}).`),
          ),
        );
      }
      if (action === 'redact')
        return out(
          await L.redact(
            need(id, 'EPISODE_ID'),
            { step: Number(required('step')), field: required('field') },
            await humanActor(`Redact ${values.field} of step ${values.step} in ${id}.`),
          ),
        );
      if (action === 'unredact')
        return out(
          await L.unredact(
            need(id, 'EPISODE_ID'),
            required('redaction'),
            await humanActor(`Undo redaction ${values.redaction} in ${id}.`),
          ),
        );
      if (action === 'import')
        return out(
          await L.importTrainingEpisode(
            need(id, 'TRAINING_EPISODE_ID'),
            { task: values.task },
            await humanActor(
              `Convert Training episode ${id} for the Liquid Agent (masked, unreviewed).`,
            ),
          ),
        );
      if (action === 'accept')
        return out(
          await L.acceptCorrection(
            need(id, 'EPISODE_ID'),
            await humanActor(`Queue correction ${id} for review.`),
          ),
        );
      if (action === 'discard')
        return out(
          await L.discardEpisode(need(id, 'EPISODE_ID'), await humanActor(`Discard ${id}.`)),
        );
      return fail(
        'UNKNOWN_COMMAND',
        'teach episodes [show|review|redact|unredact|dedupe|import|accept|discard]',
      );
    }
    case 'dataset': {
      const actor = await humanActor(
        'Build a training dataset from every reviewed episode (secret scan first). It stays on this device.',
      );
      return out(
        await L.exportDataset(
          {
            name: values.name,
            retention_days:
              values['retention-days'] === undefined ? 30 : Number(values['retention-days']),
          },
          actor,
        ),
      );
    }
    case 'train': {
      const datasetId = need(rest[0], 'DATASET_ID');
      if (values.worker) {
        const preview = await L.previewRemoteTrain(
          {
            dataset_id: datasetId,
            worker: values.worker,
            max_price_usd: Number(required('max-price')),
            public_url: values['public-url'],
          },
          await humanActor('Prepare an upload preview for a trusted c0mpute worker.'),
        );
        const actor = await humanActor(
          `UPLOAD PREVIEW\n  dataset  ${preview.dataset_hash} (${preview.episodes} episodes)\n  worker   ${preview.worker}\n  cost cap $${preview.max_price_usd}\n  retention ${preview.retention.dataset}\n  privacy  ${preview.privacy}`,
        );
        const job = await L.train(
          {
            dataset_id: datasetId,
            target: 'c0mpute',
            preview_id: preview.preview_id,
            confirm: true,
          },
          actor,
        );
        return out(await L.waitJob(job.job_id));
      }
      const job = await L.train(
        { dataset_id: datasetId, target: 'local', epochs: values.epochs },
        await humanActor(`Train CfC and a GRU baseline on ${datasetId}, on this computer.`),
      );
      process.stderr.write(`training ${job.job_id}…\n`);
      return out(await L.waitJob(job.job_id));
    }
    case 'jobs':
      return out(rest[0] ? await L.getJob(rest[0]) : await L.listJobs());
    case 'models':
      return out(await L.listModels());
    case 'evaluate':
      return out(
        await L.evaluate(
          {
            model_id: need(rest[0], 'MODEL_ID'),
            per_workflow: Number(values['per-workflow'] ?? 5),
          },
          await humanActor(`Evaluate ${rest[0]} on the mock sites (no real site is touched).`),
        ),
      );
    case 'activate':
      return out(
        await L.activate(
          need(rest[0], 'MODEL_ID'),
          { tasks: values.task ? [values.task] : undefined },
          await humanActor(`Activate ${rest[0]}.`),
        ),
      );
    case 'deactivate':
      return out(await L.deactivate(need(rest[0], 'TASK'), plainActor()));
    case 'mode': {
      const [task, mode] = rest;
      need(task, 'TASK');
      need(mode, 'MODE');
      if (mode === 'off') return out(await L.setMode(task, 'off', plainActor()));
      const bounds = mode === 'bounded' ? { max_steps: Number(required('max-steps')) } : undefined;
      return out(
        await L.setMode(
          task,
          mode,
          await humanActor(
            `Set ${task} to ${MODE_LABELS[mode] ?? mode}. Every external write (send, post, publish, delete) still asks you first.`,
          ),
          { bounds },
        ),
      );
    }
    case 'run': {
      const task = need(rest[0], 'TASK');
      const actor = await humanActor(
        `Run the agent on the mock ${task} site. You will be asked before actions.`,
      );
      const rl = createInterface({ input: process.stdin, output: process.stderr });
      try {
        const run = await L.startRun(
          { task, seed: Number(values.seed ?? 1), variant: Number(values.variant ?? 0) },
          actor,
          {
            approver: async (q) => {
              const a = (
                await rl.question(
                  `${q.external ? 'EXTERNAL WRITE ' : ''}${q.action.type} ${q.action.target?.name ?? ''}? approve [y/N] `,
                )
              )
                .trim()
                .toLowerCase();
              return { approved: a === 'y', by: actor.id };
            },
            person: async (_obs, proposal) => {
              const a = (
                await rl.question(
                  `suggested: ${proposal?.action ? `${proposal.action.type} ${proposal.action.target?.name ?? ''}` : `nothing (${proposal?.abstain_reason ?? 'abstained'})`}; do it [a] or stop [s]? `,
                )
              )
                .trim()
                .toLowerCase();
              return a === 'a' && proposal?.action ? proposal.action : null;
            },
          },
        );
        return out(await L.waitRun(run.run_id));
      } finally {
        rl.close();
      }
    }
    default:
      return fail('UNKNOWN_COMMAND', `Unknown teach command ${sub}. Run w3bs training --help.`);
  }
}
