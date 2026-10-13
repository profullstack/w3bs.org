import { createHash } from 'node:crypto';
import { spawn } from 'node:child_process';
import { mkdir, mkdtemp, rm, writeFile } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { Readable, Transform } from 'node:stream';
import { pipeline } from 'node:stream/promises';
import { createGunzip } from 'node:zlib';
import { join, resolve } from 'node:path';
import { ROOT } from './store.mjs';

// Package malware scanning. Every catalog package is scanned with ThreatCrush when it
// is submitted (CI on catalog pull requests, scripts/scan-packages.mjs) and again
// whenever it changes: a new catalog entry, a new latest_version, different installer
// bytes or a new commit on its GitHub repository. Nothing downloaded here is ever
// executed; ThreatCrush reads it as text.

// Rules whose only innocent explanation is rare: a decoded blob run as code, or the
// whole environment serialised into a payload. CWE-506 is "embedded malicious code",
// so any rule ThreatCrush adds under it counts too. Everything else (an installer that
// pipes another script to sh, a weak hash) is reported as a note, not as malware.
export const MALWARE_RULES = new Set([
  'js-encoded-payload-execution',
  'js-environment-exfiltration',
]);
const MAX_INSTALLER = 2 * 1024 * 1024;
const MAX_SOURCE = 64 * 1024 * 1024;
const MAX_UNPACKED = 512 * 1024 * 1024;
const MAX_FINDINGS = 100;
const USER_AGENT = 'w3bs.org package scanner (+https://w3bs.org/packages)';

export const installerUrl = (entry) => /https:\/\/[^\s|'"]+/.exec(entry.install || '')?.[0];
export function githubRepo(entry) {
  const match = /^https:\/\/github\.com\/([\w.-]+)\/([\w.-]+?)(?:\.git)?\/?$/.exec(
    entry.repository || '',
  );
  return match ? { owner: match[1], repo: match[2] } : undefined;
}
const sha256 = (data) => createHash('sha256').update(data).digest('hex');

async function download(fetchImpl, url, limit, headers = {}) {
  const res = await fetchImpl(url, {
    redirect: 'follow',
    signal: AbortSignal.timeout(120000),
    headers: { 'user-agent': USER_AGENT, ...headers },
  });
  if (!res.ok) throw new Error(`${url} answered HTTP ${res.status}`);
  const chunks = [];
  let size = 0;
  for await (const chunk of res.body) {
    size += chunk.length;
    if (size > limit) throw new Error(`${url} is larger than ${limit} bytes`);
    chunks.push(chunk);
  }
  return Buffer.concat(chunks);
}

function run(command, args, { timeout = 300000, maxOutput = 32 * 1024 * 1024 } = {}) {
  return new Promise((resolvePromise, reject) => {
    const child = spawn(command, args, { stdio: ['ignore', 'pipe', 'pipe'] });
    const out = [],
      err = [];
    let size = 0;
    const timer = setTimeout(() => child.kill('SIGKILL'), timeout);
    child.stdout.on('data', (chunk) => {
      size += chunk.length;
      if (size > maxOutput) child.kill('SIGKILL');
      else out.push(chunk);
    });
    child.stderr.on('data', (chunk) => err.push(chunk));
    child.on('error', reject);
    child.on('close', (code, signal) => {
      clearTimeout(timer);
      if (code === 0) resolvePromise(Buffer.concat(out).toString('utf8'));
      else
        reject(
          new Error(
            `${command} ${args[0]} failed (${signal || `exit ${code}`}): ${Buffer.concat(err).toString('utf8').slice(0, 500)}`,
          ),
        );
    });
  });
}

// Unpack a .tar.gz. The container image has tar but no gzip, so decompress here and
// stream plain tar in, stopping at MAX_UNPACKED so a small archive cannot fill the disk.
async function untarGz(archive, target) {
  const child = spawn(
    'tar',
    ['-xf', '-', '-C', target, '--no-same-owner', '--no-same-permissions'],
    {
      stdio: ['pipe', 'ignore', 'pipe'],
    },
  );
  const err = [];
  child.stderr.on('data', (chunk) => err.push(chunk));
  const exited = new Promise((resolvePromise, reject) => {
    child.on('error', reject);
    child.on('close', (code) =>
      code === 0
        ? resolvePromise()
        : reject(
            new Error(
              `tar failed (exit ${code}): ${Buffer.concat(err).toString('utf8').slice(0, 300)}`,
            ),
          ),
    );
  });
  let size = 0;
  const limit = new Transform({
    transform(chunk, _encoding, callback) {
      size += chunk.length;
      callback(
        size > MAX_UNPACKED ? new Error(`source unpacks to more than ${MAX_UNPACKED} bytes`) : null,
        chunk,
      );
    },
  });
  try {
    await pipeline(Readable.from([archive]), createGunzip(), limit, child.stdin);
  } catch (error) {
    child.kill('SIGKILL');
    await exited.catch(() => {});
    throw error;
  }
  await exited;
}

// Runs the pinned ThreatCrush CLI from node_modules under the current runtime (Bun in
// the container, Node or Bun locally). W3BS_THREATCRUSH points at another build.
export async function runThreatcrush(dir) {
  const cli =
    process.env.W3BS_THREATCRUSH ||
    resolve(ROOT, 'node_modules/@profullstack/threatcrush/dist/index.js');
  return JSON.parse(await run(process.execPath, [cli, 'scan', '--format', 'json', dir]));
}

export function verdict(report) {
  const findings = (report.findings || []).map((finding) => ({
    ruleId: finding.ruleId,
    title: finding.title,
    severity: finding.severity,
    category: finding.category,
    cwe: finding.cwe,
    // Paths are relative to the scan root: installer/… or source/<repo-dir>/…
    file: String(finding.file || '').replace(/^source\/[^/]+\//, 'source/'),
    line: finding.line,
    malware: MALWARE_RULES.has(finding.ruleId) || finding.cwe === 'CWE-506',
  }));
  const malware = findings.filter((finding) => finding.malware).length;
  return {
    status: malware ? 'flagged' : 'passed',
    scanner: `threatcrush ${report.version}`,
    filesScanned: report.filesScanned,
    summary: report.summary,
    malware,
    findings: findings.sort((a, b) => Number(b.malware) - Number(a.malware)).slice(0, MAX_FINDINGS),
    truncated: findings.length > MAX_FINDINGS,
  };
}

// What identifies the thing users would install right now. Cheap: one installer
// download and, for GitHub repositories, one API call for the default branch head.
export async function probe(entry, { fetch: fetchImpl = globalThis.fetch, githubToken } = {}) {
  const url = installerUrl(entry);
  if (!url) throw new Error(`${entry.id} has no https installer URL`);
  const installer = await download(fetchImpl, url, MAX_INSTALLER);
  const repo = githubRepo(entry);
  let commit;
  if (repo) {
    try {
      commit = (
        await download(
          fetchImpl,
          `https://api.github.com/repos/${repo.owner}/${repo.repo}/commits/HEAD`,
          4096,
          {
            accept: 'application/vnd.github.sha',
            ...(githubToken ? { authorization: `Bearer ${githubToken}` } : {}),
          },
        )
      )
        .toString('utf8')
        .trim();
      if (!/^[0-9a-f]{40}$/.test(commit)) commit = undefined;
    } catch {
      // Rate limited or private: scan the default branch and fingerprint without it.
      commit = undefined;
    }
  }
  const sources = {
    installer: url,
    installerSha256: sha256(installer),
    ...(repo ? { repository: `github.com/${repo.owner}/${repo.repo}`, commit } : {}),
  };
  const fingerprint = sha256(
    JSON.stringify([entry.latest_version, entry.install, sources.installerSha256, commit]),
  );
  return { installer, sources, fingerprint, repo };
}

// Download the installer and (for GitHub repositories) the source at the probed
// commit, then scan both in one ThreatCrush run.
export async function scanPackage(
  entry,
  {
    fetch: fetchImpl = globalThis.fetch,
    threatcrush = runThreatcrush,
    githubToken = process.env.GITHUB_TOKEN,
    probed,
  } = {},
) {
  const found = probed ?? (await probe(entry, { fetch: fetchImpl, githubToken }));
  const base = process.env.W3BS_SCAN_DIR || tmpdir();
  await mkdir(base, { recursive: true });
  const dir = await mkdtemp(join(base, 'w3bs-scan-'));
  try {
    await mkdir(join(dir, 'installer'));
    await writeFile(join(dir, 'installer', 'install.sh'), found.installer);
    const notes = [];
    if (found.repo) {
      try {
        const archive = await download(
          fetchImpl,
          `https://codeload.github.com/${found.repo.owner}/${found.repo.repo}/tar.gz/${found.sources.commit || 'HEAD'}`,
          MAX_SOURCE,
        );
        await mkdir(join(dir, 'source'));
        await untarGz(archive, join(dir, 'source'));
      } catch (error) {
        notes.push(`Source not scanned: ${error.message}`);
      }
    } else notes.push('No public GitHub repository listed; only the installer was scanned.');
    const result = verdict(await threatcrush(dir));
    return {
      ...result,
      version: entry.latest_version ?? null,
      fingerprint: found.fingerprint,
      sources: found.sources,
      notes,
      scannedAt: new Date().toISOString(),
    };
  } finally {
    await rm(dir, { recursive: true, force: true });
  }
}

// The background re-scanner. One package at a time; a package is re-scanned only when
// its fingerprint moved or its last scan errored.
export function createScanner(
  store,
  {
    intervalMs = Number(process.env.W3BS_SCAN_INTERVAL_HOURS || 6) * 3600000,
    startDelayMs = 30000,
    log = (message) => process.stderr.write(`w3bs scan: ${message}\n`),
    ...options
  } = {},
) {
  let timer,
    running = false;
  async function check(entry, { force = false } = {}) {
    const previous = await store.latestScan(entry.id);
    let probed;
    try {
      probed = await probe(entry, {
        fetch: options.fetch,
        githubToken: options.githubToken ?? process.env.GITHUB_TOKEN,
      });
    } catch (error) {
      // Installer unreachable. Keep a good result; record the failure if there is none.
      if (!previous) await store.recordScan(entry.id, failure(entry, error));
      log(`${entry.id}: ${error.message}`);
      return previous;
    }
    if (!force && previous?.fingerprint === probed.fingerprint && previous.status !== 'error')
      return previous;
    let result;
    try {
      result = await scanPackage(entry, { ...options, probed });
    } catch (error) {
      result = {
        ...failure(entry, error),
        fingerprint: probed.fingerprint,
        sources: probed.sources,
      };
    }
    await store.recordScan(entry.id, result);
    log(
      `${entry.id}@${entry.latest_version ?? '?'}: ${result.status}${result.malware ? ` (${result.malware} malware indicators)` : ''}`,
    );
    return result;
  }
  async function checkAll() {
    if (running) return;
    running = true;
    try {
      for (const entry of await store.packages('')) {
        try {
          await check(entry);
        } catch (error) {
          log(`${entry.id}: ${error.message}`);
        }
      }
    } finally {
      running = false;
    }
  }
  return {
    check,
    checkAll,
    start() {
      const tick = () => {
        checkAll().finally(() => {
          timer = setTimeout(tick, intervalMs);
          timer.unref?.();
        });
      };
      timer = setTimeout(tick, startDelayMs);
      timer.unref?.();
    },
    stop: () => clearTimeout(timer),
  };
}
const failure = (entry, error) => ({
  status: 'error',
  error: error.message.slice(0, 500),
  version: entry.latest_version ?? null,
  fingerprint: '',
  scannedAt: new Date().toISOString(),
});
