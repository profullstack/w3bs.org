// Writes training/fixtures/{valid,invalid}/*.json: one { record, value, why }
// per file. Tests assert each valid example parses and each invalid one fails.
import { mkdirSync, writeFileSync } from 'node:fs';
import { fileURLToPath } from 'node:url';
import { sampleRecords } from './sample.mjs';

const dir = fileURLToPath(new URL('./', import.meta.url));
const r = sampleRecords();
const valid = {
  'session.json': ['session', r.sessions[0], 'A KDE session with one display and a narrow scope.'],
  'episode.json': [
    'episode',
    r.episodes[0],
    'A proposed goal and unknown outcome, awaiting review.',
  ],
  'observation.json': [
    'observation',
    r.observations[0],
    'A keyframe taken from the pre-action buffer.',
  ],
  'action.json': ['action', r.actions[0], 'A device-observed human click with verified alignment.'],
  'action-text-placeholder.json': [
    'action',
    {
      ...r.actions[2],
      action: { ...r.actions[2].action, arguments: { text_placeholder: '{{version_line}}' } },
    },
    'A text edit that kept only a parameter placeholder.',
  ],
  'artifact.json': ['artifact', r.artifacts[0], 'A PNG keyframe in the bundle.'],
  'manifest.json': ['manifest', r.manifest, 'A bundle manifest.'],
  'rights.json': ['rights', r.rights, "The uploader's rights declaration."],
};
const invalid = {
  'ns-as-number.json': [
    'action',
    { ...r.actions[0], action: { ...r.actions[0].action, monotonic_ns: 1000000000 } },
    'Nanosecond counters must be strings.',
  ],
  'unknown-origin.json': [
    'action',
    { ...r.actions[0], action: { ...r.actions[0].action, origin: 'robot' } },
    'Actor origin is a closed set.',
  ],
  'extra-field.json': [
    'episode',
    { ...r.episodes[0], productivity_score: 7 },
    'Records are strict; unknown fields are refused.',
  ],
  'absolute-artifact-path.json': [
    'artifact',
    { ...r.artifacts[0], path: '/home/me/screen.png' },
    'No absolute private paths.',
  ],
  'traversal-artifact-path.json': [
    'artifact',
    { ...r.artifacts[0], path: 'artifacts/../../.ssh/id_rsa' },
    'No parent segments.',
  ],
  'html-artifact.json': [
    'artifact',
    { ...r.artifacts[0], path: 'artifacts/x.html', media_type: 'text/html' },
    'Only image and video media types.',
  ],
  'reversed-segment.json': [
    'episode',
    { ...r.episodes[0], segments: [{ session_id: 'ses_a1', from_ns: '9000', to_ns: '10' }] },
    'A segment cannot end before it starts.',
  ],
  'generated-explanation.json': [
    'episode',
    { ...r.episodes[0], explanation: { text: 'I clicked because…', author: 'model' } },
    'Explanations are human-authored only.',
  ],
  'wrong-schema-version.json': [
    'manifest',
    { ...r.manifest, schema_version: '1.0' },
    'Only schema 0.1 is defined.',
  ],
};
for (const [name, set] of [
  ['valid', valid],
  ['invalid', invalid],
]) {
  mkdirSync(`${dir}${name}`, { recursive: true });
  for (const [file, [record, value, why]] of Object.entries(set))
    writeFileSync(`${dir}${name}/${file}`, `${JSON.stringify({ record, why, value }, null, 2)}\n`);
}
process.stdout.write('Wrote fixture examples.\n');
