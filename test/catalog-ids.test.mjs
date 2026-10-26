import test from 'node:test';
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';

// Mirrors package-manager/crates/w3bs-schema/src/identity.rs: the signer rejects the whole
// catalog on one bad id, so catch it here instead of in the nightly re-sign.
const catalog = JSON.parse(readFileSync(new URL('../catalog/catalog.json', import.meta.url)));
const label = /^[a-z0-9]([a-z0-9-]{0,61}[a-z0-9])?$/;
const name = /^[a-z0-9][a-z0-9_-]{0,63}$/;
const command = /^[A-Za-z0-9_][A-Za-z0-9_.-]{0,63}$/;

test('every catalog id is publisher.example/name and every command is valid', () => {
  const seen = new Set();
  for (const entry of catalog) {
    const parts = entry.id.split('/');
    assert.equal(parts.length, 2, `${entry.id}: must look like publisher.example/name`);
    const [publisher, pkg] = parts;
    assert.ok(publisher.includes('.') && publisher.length <= 253, `${entry.id}: publisher must be a hostname`);
    for (const part of publisher.split('.')) assert.match(part, label, `${entry.id}: publisher label "${part}"`);
    assert.match(pkg, name, `${entry.id}: package name`);
    assert.ok(!seen.has(entry.id), `${entry.id}: duplicate id`);
    seen.add(entry.id);
    for (const cmd of entry.commands ?? []) {
      assert.match(cmd, command, `${entry.id}: command "${cmd}"`);
      assert.ok(!['w3bs', 'w3bs-publish'].includes(cmd), `${entry.id}: command "${cmd}" is reserved`);
    }
  }
});
