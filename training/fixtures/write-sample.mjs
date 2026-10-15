// bun training/fixtures/write-sample.mjs [out.w3bs-training.zip]
// Writes the synthetic tutorial bundle (no real capture inside).
import { writeFileSync } from 'node:fs';
import { sampleBundle } from './sample.mjs';

const out = process.argv[2] ?? 'sample.w3bs-training.zip';
writeFileSync(out, sampleBundle());
process.stdout.write(`${out}\n`);
