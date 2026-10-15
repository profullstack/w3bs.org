#!/usr/bin/env node
// w3bs-training: the W3BS Training CLI. The w3bs CLI runs it for
// `w3bs training ...`. Output is JSON unless a command is interactive.
import { randomBytes } from 'node:crypto';
import { readFileSync } from 'node:fs';
import { userInfo } from 'node:os';
import { createInterface } from 'node:readline/promises';
import { parseArgs } from 'node:util';
import { openTraining, VERSION } from '../core/service.mjs';
import { defaultHome } from '../core/store.mjs';

const help = `W3BS Training ${VERSION}: review, curate and consume demonstrations

  w3bs training status
  w3bs training open [--port N] [--no-browser]       local review app (human session)
  w3bs training import FILE [--preview] [--mapping mapping.json] [--annotations notes.jsonl]
                            [--rights-file rights.json] [--allow-text] [--duration-ms N]
  w3bs training commit IMPORT_ID | imports [IMPORT_ID]
  w3bs training inbox [--all] | review                 review cards (review is interactive)
  w3bs training episodes list [--state S] | show ID | delete ID... | split ID --at STEP
                         | merge ID ID... | label ID --decision D [--goal-file F] [--explanation-file F]
  w3bs training datasets create --name N | list | inspect ID | update ID --metadata-file F
                         | add ID EP... | remove ID EP... | delete ID --dry-run|--confirm
  w3bs training versions create DATASET_ID [--manifest-file selection.json] | show ID V
                         | approve ID V --purpose a,b | release ID V | revoke ID V
  w3bs training consumers add --config-file consumer.json | list | revoke GRANT_ID
  w3bs training access on|off                          every agent's retrieval at once
  w3bs training retrieve --consumer NAME --query-file task.txt [--dataset ID]
  w3bs training export DATASET_ID --version V --format bundle|jsonl|next-action --out PATH [--purpose P]
  w3bs training runs [--consumer NAME] | evaluate --plan-file evaluation.json
  w3bs training workflows derive EPISODE_ID | list
  w3bs training approvals list | approve ID | deny ID
  w3bs training jobs adapters | submit --plan-file plan.json | cancel JOB_ID
  w3bs training forget --last 10m | --from ISO [--to ISO]
  w3bs training deletions REQUEST_ID | audit
  w3bs training mcp serve --stdio --consumer NAME
  w3bs training setup claude-code|codex|opencode --consumer NAME

Human-only commands (review, label, approve, release, grant, export, delete,
forget) need an interactive terminal and a typed confirmation code. Shell access
alone is not consent. Sensitive text is read from files, never from arguments.
Data lives in ${defaultHome()} (W3BS_TRAINING_HOME). Recording itself is
\`w3bs desktop training\`.
`;

const fail = (code, message, details) => {
  const error = new Error(message);
  error.code = code;
  if (details) error.details = details;
  throw error;
};
const out = (value) => process.stdout.write(`${JSON.stringify(value, null, 2)}\n`);
const readJson = (path) => JSON.parse(readFileSync(path, 'utf8'));
const readText = (path) => readFileSync(path === '-' ? 0 : path, 'utf8').trim();
const username = () => {
  try {
    return userInfo().username;
  } catch {
    return 'local-user';
  }
};
const interactive = () => process.stdin.isTTY && process.stdout.isTTY;

// A person at a terminal types a fresh code. Without a TTY this refuses, so an
// agent cannot pipe "yes" into a human-only command.
async function humanActor(summary) {
  if (!interactive())
    fail(
      'HUMAN_APPROVAL_REQUIRED',
      'This command needs a person at an interactive terminal (or use `w3bs training open`). Agents can file a request with training_request_approval.',
    );
  const code = randomBytes(3).toString('hex').slice(0, 4).toUpperCase();
  const rl = createInterface({ input: process.stdin, output: process.stderr });
  process.stderr.write(`\n${summary}\n`);
  const answer = (await rl.question(`Type ${code} to confirm: `)).trim().toUpperCase();
  rl.close();
  if (answer !== code)
    fail('HUMAN_APPROVAL_REQUIRED', 'Confirmation code did not match. Nothing was changed.');
  return { kind: 'human', id: username() };
}
const plainActor = () => ({ kind: interactive() ? 'human' : 'agent', id: username() });

function parseDuration(value) {
  const match = /^(\d+)\s*(s|m|h|d)$/.exec(String(value ?? ''));
  if (!match) fail('INVALID_INPUT', '--last takes a duration like 30s, 10m, 2h or 1d.');
  return Number(match[1]) * { s: 1e3, m: 6e4, h: 36e5, d: 864e5 }[match[2]];
}

const { values, positionals } = (() => {
  try {
    return parseArgs({
      allowPositionals: true,
      options: {
        help: { type: 'boolean', short: 'h' },
        json: { type: 'boolean' },
        all: { type: 'boolean' },
        preview: { type: 'boolean' },
        stdio: { type: 'boolean' },
        'no-browser': { type: 'boolean' },
        'dry-run': { type: 'boolean' },
        confirm: { type: 'boolean' },
        'allow-text': { type: 'boolean' },
        port: { type: 'string' },
        mapping: { type: 'string' },
        annotations: { type: 'string' },
        'rights-file': { type: 'string' },
        'duration-ms': { type: 'string' },
        state: { type: 'string' },
        at: { type: 'string' },
        decision: { type: 'string' },
        'goal-file': { type: 'string' },
        'explanation-file': { type: 'string' },
        name: { type: 'string' },
        'metadata-file': { type: 'string' },
        'manifest-file': { type: 'string' },
        purpose: { type: 'string' },
        'config-file': { type: 'string' },
        consumer: { type: 'string' },
        'query-file': { type: 'string' },
        dataset: { type: 'string' },
        version: { type: 'string' },
        format: { type: 'string' },
        out: { type: 'string' },
        'plan-file': { type: 'string' },
        last: { type: 'string' },
        from: { type: 'string' },
        to: { type: 'string' },
        'goal-family': { type: 'string' },
      },
    });
  } catch (error) {
    process.stderr.write(
      `${JSON.stringify({ error: { code: 'INVALID_ARGUMENTS', message: error.message } })}\n`,
    );
    process.exit(2);
  }
})();

const [command, sub, ...rest] = positionals;
if (values.help || !command) {
  process.stdout.write(help);
  process.exit(0);
}
if (command === 'version' || command === '--version') {
  out({ version: VERSION });
  process.exit(0);
}

let service;
try {
  service = await openTraining();
  const required = (name) => values[name] ?? fail('MISSING_OPTION', `--${name} is required.`);
  const need = (value, what) => value ?? fail('MISSING_ARGUMENT', `${what} is required.`);
  switch (command) {
    case 'status':
      out(await service.status());
      break;
    case 'open': {
      const { serveLocalApp } = await import('../ui/server.mjs');
      await serveLocalApp(service, {
        port: Number(values.port ?? 0),
        openBrowser: !values['no-browser'],
        user: username(),
      });
      service = null; // the server owns the store until Ctrl-C
      break;
    }
    case 'import': {
      const path = need(sub, 'FILE');
      const result = await service.importFile(
        {
          path,
          mapping: values.mapping && readJson(values.mapping),
          annotations:
            values.annotations &&
            readText(values.annotations)
              .split('\n')
              .filter(Boolean)
              .map((l) => JSON.parse(l)),
          rights: values['rights-file'] && readJson(values['rights-file']),
          allowText: !!values['allow-text'],
          durationMs: values['duration-ms'] && Number(values['duration-ms']),
        },
        plainActor(),
      );
      if (values.preview) out(result);
      else
        out({
          ...(await service.commitImport(result.import_id, plainActor())),
          preview: result.preview,
        });
      break;
    }
    case 'commit':
      out(await service.commitImport(need(sub, 'IMPORT_ID'), plainActor()));
      break;
    case 'imports':
      out(sub ? await service.getImport(sub) : await service.listImports());
      break;
    case 'inbox':
      out(await service.inbox({ all: !!values.all }));
      break;
    case 'review':
    case 'tui': {
      const { reviewLoop } = await import('../ui/terminal.mjs');
      await reviewLoop(service, await humanActor('Start a review session in this terminal.'));
      break;
    }
    case 'episodes': {
      if (sub === 'list') out(await service.listEpisodes({ state: values.state }));
      else if (sub === 'show') out(await service.getEpisode(need(rest[0], 'EPISODE_ID')));
      else if (sub === 'delete') {
        need(rest[0], 'EPISODE_ID');
        out(
          await service.deleteEpisodes(
            rest,
            await humanActor(
              `Permanently delete ${rest.length} episode(s) and the frames only they use.`,
            ),
          ),
        );
      } else if (sub === 'split')
        out(
          await service.split(
            need(rest[0], 'EPISODE_ID'),
            { at_step: required('at') },
            await humanActor(`Split ${rest[0]} before ${values.at}.`),
          ),
        );
      else if (sub === 'merge')
        out(await service.merge(rest, await humanActor(`Merge ${rest.length} episodes.`)));
      else if (sub === 'label') {
        const id = need(rest[0], 'EPISODE_ID');
        const input = { decision: required('decision') };
        if (values['goal-file']) input.goal = readText(values['goal-file']);
        if (values['explanation-file']) input.explanation = readText(values['explanation-file']);
        if (values['goal-family']) input.family = values['goal-family'];
        // An outcome decision also confirms the goal shown here, as on a review card.
        const outcome = ['complete', 'partial', 'failed', 'interrupted', 'unknown'].includes(
          input.decision,
        );
        if (outcome && input.goal === undefined) input.confirm_goal = true;
        const { episode } = await service.getEpisode(id);
        const goal = input.goal ?? episode.goal.value;
        out(
          await service.review(
            id,
            input,
            await humanActor(`Label ${id}: ${input.decision}${outcome ? `\nGoal: ${goal}` : ''}`),
          ),
        );
      } else fail('UNKNOWN_COMMAND', 'episodes list|show|delete|split|merge|label');
      break;
    }
    case 'datasets': {
      if (sub === 'create')
        out(await service.createDataset({ name: required('name') }, plainActor()));
      else if (sub === 'list') out(await service.listDatasets());
      else if (sub === 'inspect') out(await service.inspectDataset(need(rest[0], 'DATASET_ID')));
      else if (sub === 'update')
        out(
          await service.updateDataset(
            need(rest[0], 'DATASET_ID'),
            {
              name: values.name,
              metadata: values['metadata-file'] && readJson(values['metadata-file']),
            },
            plainActor(),
          ),
        );
      else if (sub === 'add')
        out(await service.addEpisodes(need(rest[0], 'DATASET_ID'), rest.slice(1), plainActor()));
      else if (sub === 'remove')
        out(await service.removeEpisodes(need(rest[0], 'DATASET_ID'), rest.slice(1), plainActor()));
      else if (sub === 'delete') {
        const id = need(rest[0], 'DATASET_ID');
        if (values.confirm)
          out(
            await service.deleteDataset(
              id,
              { confirm: true },
              await humanActor(`Delete dataset ${id}, its versions and grants.`),
            ),
          );
        else out(await service.deleteDataset(id, { confirm: false }, plainActor()));
      } else fail('UNKNOWN_COMMAND', 'datasets create|list|inspect|update|add|remove|delete');
      break;
    }
    case 'versions': {
      const [id, version] = rest;
      if (sub === 'create') {
        const selection = values['manifest-file'] && readJson(values['manifest-file']);
        out(
          await service.createVersion(
            need(id, 'DATASET_ID'),
            { episodes: selection?.episodes, seed: selection?.seed },
            plainActor(),
          ),
        );
      } else if (sub === 'show')
        out(await service.getVersion(need(id, 'DATASET_ID'), need(version, 'VERSION')));
      else if (sub === 'approve') {
        const purposes = required('purpose')
          .split(',')
          .map((p) => p.trim())
          .filter(Boolean);
        out(
          await service.approve(
            need(id, 'DATASET_ID'),
            need(version, 'VERSION'),
            purposes,
            await humanActor(`Approve ${id}@${version} for: ${purposes.join(', ')}.`),
          ),
        );
      } else if (sub === 'release')
        out(
          await service.release(
            need(id, 'DATASET_ID'),
            need(version, 'VERSION'),
            await humanActor(`Release ${id}@${version}.`),
          ),
        );
      else if (sub === 'revoke')
        out(
          await service.revoke(
            need(id, 'DATASET_ID'),
            need(version, 'VERSION'),
            await humanActor(`Revoke ${id}@${version} and every grant to it.`),
          ),
        );
      else fail('UNKNOWN_COMMAND', 'versions create|show|approve|release|revoke');
      break;
    }
    case 'consumers': {
      if (sub === 'add') {
        const config = readJson(required('config-file'));
        out(
          await service.grant(
            config,
            await humanActor(
              `Let agent "${config.consumer}" retrieve ${config.dataset_id}@${config.version} for ${config.purpose ?? 'private_retrieval'} (${config.days ?? 30} days).`,
            ),
          ),
        );
      } else if (sub === 'list') out(await service.listGrants());
      else if (sub === 'revoke')
        out(
          await service.revokeGrant(
            need(rest[0], 'GRANT_ID'),
            await humanActor(`Revoke grant ${rest[0]}.`),
          ),
        );
      else fail('UNKNOWN_COMMAND', 'consumers add|list|revoke');
      break;
    }
    case 'access':
      if (sub !== 'on' && sub !== 'off') fail('UNKNOWN_COMMAND', 'access on|off');
      out(
        await service.setAgentAccess(
          sub === 'on',
          await humanActor(`Turn agent access to demonstrations ${sub}.`),
        ),
      );
      break;
    case 'retrieve':
      out(
        await service.retrieve(
          {
            consumer: required('consumer'),
            query: values['query-file'] ? readText(values['query-file']) : '',
            dataset_id: values.dataset,
          },
          plainActor().kind === 'agent'
            ? { kind: 'agent', id: required('consumer') }
            : plainActor(),
        ),
      );
      break;
    case 'export': {
      const id = need(sub, 'DATASET_ID');
      const version = required('version');
      const format = values.format ?? 'bundle';
      out(
        await service.export(
          id,
          version,
          { format, purpose: values.purpose, out: required('out') },
          await humanActor(
            `Export ${id}@${version} as ${format} to ${values.out}.\nThe copy will live outside W3BS: disabling the desktop layer or revoking the version cannot recall it.`,
          ),
        ),
      );
      break;
    }
    case 'runs':
      out(await service.listRuns({ consumer: values.consumer }));
      break;
    case 'evaluate':
      out(await service.evaluate(values['plan-file'] ? readJson(values['plan-file']) : {}));
      break;
    case 'workflows':
      if (sub === 'derive')
        out(
          await service.deriveWorkflow(
            need(rest[0], 'EPISODE_ID'),
            await humanActor(`Create an untested workflow from ${rest[0]}.`),
          ),
        );
      else if (sub === 'list') out(await service.listWorkflows());
      else fail('UNKNOWN_COMMAND', 'workflows derive|list');
      break;
    case 'approvals':
      if (sub === 'list') out(await service.listApprovals());
      else if (sub === 'approve' || sub === 'deny')
        out(
          await service.decideApproval(
            need(rest[0], 'REQUEST_ID'),
            sub === 'approve',
            await humanActor(`${sub} request ${rest[0]}.`),
          ),
        );
      else fail('UNKNOWN_COMMAND', 'approvals list|approve|deny');
      break;
    case 'jobs':
      if (sub === 'adapters') out(await service.trainingAdapters());
      else if (sub === 'submit')
        out(
          await service.submitTrainingJob(
            readJson(required('plan-file')),
            await humanActor('Submit a training job.'),
          ),
        );
      else if (sub === 'cancel')
        fail(
          'NOT_FOUND',
          `No training job ${rest[0] ?? ''}: no training adapter has run a job here.`,
        );
      else fail('UNKNOWN_COMMAND', 'jobs adapters|submit|cancel');
      break;
    case 'forget': {
      const to = values.to ? new Date(values.to) : new Date();
      const from = values.last
        ? new Date(to.getTime() - parseDuration(values.last))
        : values.from
          ? new Date(values.from)
          : fail('MISSING_OPTION', '--last or --from is required.');
      out(
        await service.forget(
          { from, to },
          await humanActor(
            `Delete everything captured between ${from.toISOString()} and ${to.toISOString()}.`,
          ),
        ),
      );
      break;
    }
    case 'deletions':
      out(await service.getDeletion(need(sub, 'REQUEST_ID')));
      break;
    case 'audit':
      out(await service.auditLog());
      break;
    case 'mcp': {
      if (sub !== 'serve' || !values.stdio)
        fail('UNKNOWN_COMMAND', 'mcp serve --stdio --consumer NAME');
      const consumer = required('consumer');
      if (!/^[A-Za-z0-9._-]{1,80}$/.test(consumer))
        fail('INVALID_INPUT', 'consumer must be 1-80 letters, digits, dot, dash or underscore.');
      const { createTrainingMcp } = await import('../mcp.mjs');
      const { StdioServerTransport } = await import('@modelcontextprotocol/sdk/server/stdio.js');
      await createTrainingMcp(service, consumer).connect(new StdioServerTransport());
      service = null;
      break;
    }
    case 'setup': {
      const consumer = values.consumer ?? sub;
      const args = ['training', 'mcp', 'serve', '--stdio', '--consumer', consumer];
      const configs = {
        'claude-code': { run: `claude mcp add w3bs-training -- w3bs ${args.join(' ')}` },
        codex: {
          file: '~/.codex/config.toml',
          add: `[mcp_servers.w3bs-training]\ncommand = "w3bs"\nargs = ${JSON.stringify(args)}\n`,
        },
        opencode: {
          file: 'opencode.json',
          add: { mcp: { 'w3bs-training': { type: 'local', command: ['w3bs', ...args] } } },
        },
      };
      if (!configs[sub]) fail('UNKNOWN_COMMAND', 'setup claude-code|codex|opencode');
      out({
        client: sub,
        consumer,
        ...configs[sub],
        then: `Grant it access as a person: w3bs training consumers add --config-file consumer.json with {"consumer":"${consumer}","dataset_id":"...","version":1}`,
        note: 'MCP access lets the agent read approved demonstrations. It does not mean the model behind the client can be fine-tuned, and it grants no desktop actions.',
      });
      break;
    }
    default:
      fail('UNKNOWN_COMMAND', `Unknown command ${command}. Run w3bs training --help.`);
  }
} catch (error) {
  process.stderr.write(
    `${JSON.stringify({ error: { code: error.code || 'FAILED', message: error.message, ...(error.details ? { details: error.details } : {}) } }, null, 2)}\n`,
  );
  process.exitCode = error.code === 'HUMAN_APPROVAL_REQUIRED' ? 3 : 1;
} finally {
  service?.close();
}
