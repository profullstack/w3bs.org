import { readFileSync, existsSync } from 'node:fs';
import { resolve } from 'node:path';
import { marked } from 'marked';
import { ROOT } from './store.mjs';
import { W3bsError } from './core.mjs';
import { FLEET_LIMITS } from './fleet.mjs';
import { TTL } from './accounts.mjs';

const e = (value) =>
  String(value ?? '').replace(
    /[&<>"']/g,
    (char) => ({ '&': '&amp;', '<': '&lt;', '>': '&gt;', '"': '&quot;', "'": '&#39;' })[char],
  );
const pill = (text, extra = '') => `<span class="pill ${extra}">${e(text)}</span>`;
const canonicalExample = 'w3bs://prompt/w3bs/research@1';
const resourceLink = (id) => `/browse?uri=${encodeURIComponent(id)}`;
const specs = [
  [
    'W3BS-URI-1',
    'One address. Every surface.',
    'Canonical resource identifiers, namespaces and version selection.',
    'Community Draft',
  ],
  [
    'W3BS-MANIFEST-1',
    'A resource you can inspect.',
    'Identity, content, permissions, provenance and signed integrity.',
    'Community Draft',
  ],
  [
    'W3BS-RESOLVE-1',
    'Find it. Resolve it. Verify it.',
    'Discovery, deterministic resolution and transport-independent identity.',
    'Community Draft',
  ],
  [
    'W3BS-SURFACE-1',
    'The same meaning, everywhere.',
    'Shared operations across interfaces and conformance evidence.',
    'Proposal',
  ],
  [
    'W3BS-PROMPT-1',
    'Instructions that travel.',
    'Portable prompts, declared inputs and behavior packages.',
    'Proposal',
  ],
  [
    'W3BS-TRUST-1',
    'Trust is explicit.',
    'Signatures, provenance and the boundary between data and authority.',
    'Proposal',
  ],
  [
    'W3BS-DELEGATION-1',
    'Authority with boundaries.',
    'Consent, scopes, expiration and revocation of delegated permissions.',
    'Proposal',
  ],
  [
    'W3BS-DISCOVERY-1',
    'A front door for machines.',
    'A well-known bootstrap document and future discovery profiles.',
    'Proposal',
  ],
];
function header(active) {
  return `<a class="skip" href="#main">Skip to content</a><header class="header"><a class="wordmark" href="/mission" aria-label="W3BS home"><img src="/assets/mark.svg" alt="" width="28" height="28">w3bs<span class="wordmark-dot">.</span></a><nav aria-label="Main navigation">${[
    ['/packages', 'Packages'],
    ['/specs', 'Standards'],
    ['/browse', 'Browse'],
    ['/prompts', 'Prompts'],
    ['/desktop', 'Desktop'],
    ['/training', 'Training'],
    ['/fleet', 'Fleet'],
    ['/social', 'Social'],
    ['/bbs', 'Community'],
    ['/governance', 'Participate'],
  ]
    .map(
      ([href, label]) =>
        `<a href="${href}" ${active === href ? 'aria-current="page"' : ''}>${label}</a>`,
    )
    .join(
      '',
    )}</nav><a class="header-account" href="/account" ${active === '/account' ? 'aria-current="page"' : ''}>Account</a><a class="header-action" href="/developers">Start building <span aria-hidden="true">↗</span></a></header>`;
}
function footer() {
  return `<footer><div><a class="wordmark" href="/mission">w3bs<span class="wordmark-dot">.</span></a><p>Human-friendly. Agent-native.<br>Device-friendly. Open by design.</p><p class="footer-support"><strong>Need help?</strong> Email <a href="mailto:hello@w3bs.org">hello@w3bs.org</a> or ask on the <a href="/bbs/f/help">Help board</a>.</p></div><div><span class="eyebrow">FLEET AND ACCOUNTS</span><a href="/fleet">Your fleet</a><a href="/pricing">Pricing: Fleet is free</a><a href="/signup?next=%2Ffleet">Create an account</a><a href="/terms">Terms</a><a href="/privacy">Privacy</a><span class="eyebrow footer-gap">THE INITIATIVE</span><a href="/governance">Governance & participation</a><a href="/bbs">Community forums</a><a href="/social">W3BS Social (AT Protocol)</a><a href="/desktop">Agentic Desktop Layer</a><a href="/training">W3BS Training</a></div><div><span class="eyebrow">FOR IMPLEMENTERS</span><a href="/developers">Developer guide</a><a href="/packages">Packages and the w3bs CLI</a><a href="/docs/founding-prd.md">Founding PRD · v2.0</a><a href="/docs/roadmap.md">Implementation status</a><a href="/.well-known/w3bs.json">Machine discovery ↗</a><a href="/api">API reference ↗</a><a href="https://github.com/profullstack/w3bs.org" rel="noopener">Source on GitHub ↗</a></div><div class="footer-bottom"><span>Operated by Profullstack, Inc. Open standards for the Agentic Era.</span><span>Community drafts · No Recommendations issued</span></div></footer>`;
}
const INSTALL = 'curl -fsSL https://w3bs.org/install.sh | sh';
// The campaign path: install the CLI, sign in, join a server, watch it on /fleet.
const FLEET_STEPS = [
  ['Install the w3bs CLI', INSTALL, 'Linux or macOS. Installs to your home directory, no root.'],
  [
    'Sign in',
    'w3bs login',
    'Opens your browser. Over SSH it shows a code to enter at w3bs.org/device instead.',
  ],
  [
    'Join this server',
    'w3bs fleet join <token>',
    'Get the token on w3bs.org/fleet with Make join command. It works once, for an hour.',
  ],
];
function fleetHero() {
  const copy = (text, label) =>
    `<button class="small-button" type="button" data-copy="${e(text)}" aria-label="Copy: ${e(label)}">Copy</button>`;
  return `<section class="hero fleet-hero" aria-labelledby="fleet-title"><div class="hero-copy"><div class="eyebrow"><span class="status-dot"></span> W3BS FLEET · FREE</div><h1 id="fleet-title">Watch your servers <em>from one page.</em></h1><p class="hero-sub">Free. Set up in 2 minutes.</p><p class="hero-body">Install the w3bs CLI, sign in and join a Linux server. It shows up on w3bs.org/fleet with memory, disk, load and uptime, updated every ${FLEET_LIMITS.intervalSeconds} seconds.</p><div class="actions"><a class="button primary" href="/signup?next=%2Ffleet">Create a free account <span aria-hidden="true">↗</span></a><a class="text-link" href="/fleet">Open your fleet <span aria-hidden="true">→</span></a></div><div class="hero-note"><span class="status-dot"></span> No credit card. No paid plan. Up to ${FLEET_LIMITS.machines} machines. <a href="/pricing">What is free</a></div></div><div class="terminal fleet-terminal"><div class="terminal-tabs"><span class="terminal-title">set up w3bs fleet</span></div><ol class="fleet-steps">${FLEET_STEPS.map(
    ([title, command, note], index) =>
      `<li><span class="terminal-label">${index + 1} · ${e(title.toUpperCase())}</span><div class="fleet-step-line"><code>${e(command)}</code>${command.includes('<token>') ? '' : copy(command, title)}</div><span class="terminal-muted">${e(note)}</span></li>`,
  ).join(
    '',
  )}</ol><div class="terminal-result"><span class="terminal-label">THEN OPEN</span><a class="fleet-terminal-link" href="/fleet">w3bs.org/fleet <span aria-hidden="true">→</span></a><span><span class="status-dot"></span> online · memory · disk · load · uptime</span></div></div></section>`;
}
function fleetFree() {
  return `<section class="section fleet-free" aria-labelledby="free-title"><div class="section-heading"><div><div class="eyebrow">WHAT YOU GET, FREE</div><h2 id="free-title">One page.<br>Every server.</h2></div><p>Fleet is free while it is in beta, and there is no paid plan. The <a class="inline-link" href="/pricing">pricing page</a> lists every limit.</p></div><div class="principle-grid"><article><span class="card-index">01 / LIVE</span><h3>Live stats.</h3><p>Each server reports every ${FLEET_LIMITS.intervalSeconds} seconds: memory, disk, load, uptime, OS and kernel. Silent for ${FLEET_LIMITS.offlineAfterSeconds} seconds means offline.</p><a href="/fleet">Open your fleet <span aria-hidden="true">↗</span></a></article><article><span class="card-index">02 / FREE</span><h3>Up to ${FLEET_LIMITS.machines} machines.</h3><p>No card, no trial clock. Join servers with a one-time token. Remove one from the page or with <code>w3bs fleet rm</code>.</p><a href="/pricing">Limits and what is free <span aria-hidden="true">↗</span></a></article><article><span class="card-index">03 / YOUR CALL</span><h3>Read-only by default.</h3><p>A server takes no actions from this page unless you allowed them when it joined. Even then only three: report now, update w3bs, restart the desktop layer. Screen sharing is off unless you turn it on.</p><a href="/privacy">What the agent sends <span aria-hidden="true">↗</span></a></article></div></section>`;
}
async function home(store) {
  const count = (await store.search()).length;
  return `${fleetHero()}
  ${fleetFree()}
  <section class="hero hero-standards" aria-labelledby="standards-title"><div class="hero-copy"><div class="eyebrow"><span class="status-dot"></span> ALSO ON W3BS · AN OPEN STANDARDS INITIATIVE</div><h2 class="hero-title" id="standards-title">The open Web.<br>For <em>all</em> of us.</h2><p class="hero-sub">Like Nostr but for everything</p><p class="hero-body">A Web where every participant can discover, communicate and collaborate. Open protocols. Portable resources. Authority you can inspect.</p><div class="actions"><a class="button primary" href="/browse?uri=${encodeURIComponent(canonicalExample)}">Explore the open Web <span aria-hidden="true">↗</span></a><a class="text-link" href="/specs">Read the standards <span aria-hidden="true">→</span></a></div><div class="hero-note"><span class="status-dot"></span> Founding stage. Built in the open.</div></div><div class="hero-visual" aria-label="One W3BS resource connects humans, agents and devices through multiple interfaces"><div class="visual-heading"><span>ONE IDENTITY. EVERY INTERFACE.</span><span class="visual-cross">+</span></div><div class="orbit"><svg class="orbit-lines" viewBox="0 0 440 340" aria-hidden="true"><ellipse cx="220" cy="170" rx="184" ry="118"/><ellipse cx="220" cy="170" rx="118" ry="155" transform="rotate(55 220 170)"/><path d="M50 170H390M220 40V300"/></svg><span class="actor human"><span aria-hidden="true">◉</span> Humans</span><span class="actor agent"><span aria-hidden="true">✳</span> Agents</span><span class="actor device"><span aria-hidden="true">▣</span> Devices</span><span class="center-mark">w3bs<span>://</span></span><span class="orbit-dot dot-one"></span><span class="orbit-dot dot-two"></span></div><div class="uri-window"><div><span class="status-dot"></span> CANONICAL RESOURCE <span class="window-dots">···</span></div><code>w3bs://<span>prompt</span>/w3bs/research@1</code><p>Same identity. Same manifest. Same meaning.</p></div><div class="surface-strip"><span>CLI</span><span>API</span><span>MCP</span><span>WEB</span><span>NATIVE</span></div></div></section>
  <div class="principles-strip"><span>OPEN BY DEFAULT</span><span>VENDOR NEUTRAL</span><span>VERIFIABLE BY DESIGN</span><span>BUILT TO INTEROPERATE</span></div>
  <section class="section"><div class="section-heading"><div><div class="eyebrow">A SHARED FOUNDATION</div><h2>The Web is getting<br>new participants.</h2></div><p>Pages became applications. Applications are becoming collaborators. The next chapter needs common ground that belongs to everyone.</p></div><div class="principle-grid"><article><span class="card-index">01 / DISCOVER</span><h3>A common address.</h3><p>A <code>w3bs://</code> URI identifies the resource. The interface, registry and transport can change without changing what it means.</p><a href="/specs/W3BS-URI-1">Explore identifiers <span aria-hidden="true">↗</span></a></article><article><span class="card-index">02 / UNDERSTAND</span><h3>Nothing hidden.</h3><p>Inspect who published it, what it contains, which permissions it needs and whether its signature checks out.</p><a href="/specs/W3BS-MANIFEST-1">Inside a manifest <span aria-hidden="true">↗</span></a></article><article><span class="card-index">03 / COLLABORATE</span><h3>Authority, on purpose.</h3><p>Content is data by default. A signature proves provenance; running instructions still requires explicit authorization.</p><a href="/specs/W3BS-TRUST-1">Understand trust <span aria-hidden="true">↗</span></a></article></div></section>
  <section class="section resource-feature"><div><div class="eyebrow">TRY THE REFERENCE STACK</div><h2>One resource.<br>Every way in.</h2><p>Resolve a signed research prompt through the web, CLI, API, MCP or experimental native client. Compare the canonical manifest across surfaces.</p><a class="text-link" href="${resourceLink(canonicalExample)}">Inspect the example <span aria-hidden="true">→</span></a><div class="resource-count"><strong>${count.toString().padStart(2, '0')}</strong><span>signed, versioned prompts<br>ready to inspect and reuse</span></div></div><div class="terminal"><div class="terminal-tabs" role="tablist" aria-label="Access examples"><button type="button" role="tab" aria-selected="true" id="tab-cli" aria-controls="code-example" data-example="cli">CLI</button><button type="button" role="tab" aria-selected="false" tabindex="-1" id="tab-api" aria-controls="code-example" data-example="api">API</button><button type="button" role="tab" aria-selected="false" tabindex="-1" id="tab-mcp" aria-controls="code-example" data-example="mcp">MCP</button><span class="terminal-title">w3bs / reference</span></div><pre id="code-example" role="tabpanel" aria-labelledby="tab-cli"><code>w3bs resolve \\\n  w3bs://prompt/w3bs/research@1</code></pre><div class="terminal-result"><span class="terminal-label">RESOLVES TO</span><code>prompt/w3bs/research@1.0.0</code><span><span class="status-dot"></span> Signature verified</span><span class="terminal-muted">Authority: DATA · Permissions: none</span></div></div></section>
  <section class="section"><div class="section-heading"><div><div class="eyebrow">STANDARDS, IN THE OPEN</div><h2>A starting point.<br>An open invitation.</h2></div><a class="text-link" href="/specs">All eight specifications <span aria-hidden="true">↗</span></a></div><div class="spec-list">${specs
    .slice(0, 3)
    .map(
      ([id, title, description, status], index) =>
        `<a class="spec-row" href="/specs/${id}"><span class="spec-number">0${index + 1}</span><div><span class="eyebrow">${id}</span><h3>${title}</h3><p>${description}</p></div>${pill(status)}<span class="row-arrow" aria-hidden="true">↗</span></a>`,
    )
    .join('')}</div></section>
  <section class="join"><div class="eyebrow">OPEN IS SOMETHING WE BUILD TOGETHER</div><h2>Help shape what comes next.</h2><p>Implement a draft. Question an assumption. Bring a different perspective.<br>The next Web needs more than one voice.</p><a class="button dark" href="/governance">Join the work <span aria-hidden="true">↗</span></a></section>`;
}
async function promptCards(store, query) {
  const resources = await store.search(query.q || '', query.type || '');
  const catalog = JSON.parse(readFileSync(resolve(ROOT, 'fixtures/catalog.json'), 'utf8'));
  const categories = new Map(catalog.map((item) => [item.id, item.category]));
  return `<section class="page-head"><div class="eyebrow">THE OPEN PROMPT REGISTRY</div><h1>Good instructions<br>should <em>travel.</em></h1><p>Portable, versioned, signed artifacts. Inspect their source, understand their permissions and make them your own.</p></section><form class="search-bar" action="/prompts" role="search"><label class="sr-only" for="prompt-search">Search prompts</label><input id="prompt-search" name="q" type="search" value="${e(query.q || '')}" placeholder="Search by name, topic or URI…"><button class="button dark" type="submit">Search <span aria-hidden="true">→</span></button></form><div class="catalog-heading"><span>${resources.length} ${resources.length === 1 ? 'resource' : 'resources'}</span><span>Open examples · MIT licensed · v1.0.0</span></div><div class="prompt-grid">${resources.map((item) => `<a class="prompt-card" href="${resourceLink(item.id)}"><div class="prompt-top">${pill(categories.get(item.id) || item.type)}<span class="verified-small">${item.verification.valid ? '✓ Signed' : 'Verification failed'}</span></div><h2>${e(item.name)}</h2><p>${e(item.description)}</p><code>${e(item.id.replace('w3bs://', ''))}</code><div class="prompt-bottom"><span>${e(item.publisher.namespace)} <span class="muted">/ ${e(item.version)}</span></span><span aria-hidden="true">↗</span></div></a>`).join('') || '<div class="empty"><h2>No matching resources.</h2><p>Try another phrase or <a href="/prompts">view all prompts</a>.</p></div>'}</div><aside class="callout"><strong>A registry is an implementation, not the standard.</strong><p>Host your own compatible registry. Your resource identity and signed manifest remain portable.</p><a href="/developers">Build with W3BS →</a></aside>`;
}
async function browse(store, query) {
  let result, error;
  if (query.uri) {
    try {
      result = await store.inspect(query.uri);
    } catch (failure) {
      error = failure.message;
    }
  }
  return `<section class="page-head compact"><div class="eyebrow">THE REFERENCE RESOLVER</div><h1>A window into<br>the <em>Agentic Web.</em></h1><p>Enter a W3BS URI to inspect its canonical artifact, publisher, provenance and verification state.</p></section><form class="search-bar resolver" action="/browse"><label class="sr-only" for="resolve-uri">W3BS resource URI</label><input id="resolve-uri" name="uri" value="${e(query.uri || canonicalExample)}" required spellcheck="false"><button class="button primary" type="submit">Resolve <span aria-hidden="true">→</span></button></form>${error ? `<div class="error" role="alert"><strong>Could not resolve this resource.</strong><p>${e(error)}</p><a href="/prompts">Browse available resources →</a></div>` : ''}${result ? resourceDetail(result) : `<div class="resolver-empty"><span class="large-mark">w3bs<span>://</span></span><h2>The URI is canonical.<br>The surface is optional.</h2><p>Try <a href="${resourceLink(canonicalExample)}">the research prompt</a>, or <a href="/prompts">explore the registry</a>.</p></div>`}`;
}
function resourceDetail(result) {
  const { manifest: m, verification: v } = result;
  return `<script type="application/json" id="w3bs-resource">${JSON.stringify(result).replaceAll('<', '\\u003c')}</script><section class="resource-detail"><div class="resource-title"><div><div class="eyebrow">${e(m.type)} / ${e(m.publisher.namespace)}</div><h2>${e(m.name)}</h2><p>${e(m.description)}</p></div>${pill(v.valid ? 'Signature verified' : 'Verification failed', v.valid ? 'green' : 'red')}</div><div class="identity-line"><span class="eyebrow">CANONICAL IDENTITY</span><code>${e(result.canonicalUri)}</code><button class="small-button" type="button" data-copy="${e(result.canonicalUri)}">Copy URI</button></div><div class="inspector-grid"><div><section class="inspector-panel"><h3>Instructions</h3><pre class="instructions">${e(m.content.instructions)}</pre><div class="data-notice">Retrieved instructions are data until explicitly authorized. A valid signature does not grant authority.</div></section><section class="inspector-panel"><h3>Try the template</h3><p>Fill the declared inputs to render these instructions. This runtime does not call a model or any tools.</p><form id="run-form" data-uri="${e(result.canonicalUri)}">${Object.entries(
    m.content.inputs,
  )
    .map(
      ([name, field]) =>
        `<label class="input-label" for="input-${e(name)}">${e(name)} ${field.required ? '<span class="required">required</span>' : ''}</label><p class="field-description">${e(field.description)}</p><textarea id="input-${e(name)}" name="${e(name)}" rows="4" ${field.required ? 'required' : ''} maxlength="50000"></textarea>`,
    )
    .join(
      '',
    )}<label class="consent"><input type="checkbox" id="run-consent" required> I authorize rendering this template with the inputs above.</label><button class="button dark" type="submit" ${!v.valid ? 'disabled' : ''}>Render instructions <span aria-hidden="true">→</span></button><p id="run-status" role="status" aria-live="polite"></p><pre class="instructions" id="run-output" hidden></pre></form></section><details class="inspector-panel"><summary>Canonical manifest</summary><pre class="json-source">${e(JSON.stringify(m, null, 2))}</pre></details></div><aside><section class="inspector-panel metadata"><h3>Resource details</h3><dl><dt>Version</dt><dd>${e(m.version)}</dd><dt>Publisher</dt><dd>${e(m.publisher.name)}</dd><dt>License</dt><dd>${e(m.license)}</dd><dt>Created</dt><dd>${e(m.createdAt.slice(0, 10))}</dd><dt>Runtime</dt><dd>${e(m.content.runtime)}</dd><dt>Permissions</dt><dd>${m.permissions.length ? e(m.permissions.join(', ')) : 'None requested'}</dd><dt>Dependencies</dt><dd>${m.dependencies.length ? e(m.dependencies.join(', ')) : 'None'}</dd><dt>Declared class</dt><dd>${e(m.trust.classification)}</dd><dt>Effective authority</dt><dd>${e(v.authority)}</dd></dl></section><section class="inspector-panel metadata"><h3>Verification</h3><dl><dt>Signature</dt><dd>${v.signatureValid ? 'Valid Ed25519 signature' : 'Not verified'}</dd><dt>Publisher key</dt><dd>${v.publisherTrusted ? 'Pinned by registry operator' : 'Untrusted'}</dd><dt>Revocation</dt><dd>${v.revoked ? 'Revoked' : 'Not revoked'}</dd><dt>Key ID</dt><dd>${e(v.keyId)}</dd><dt>Digest</dt><dd class="digest">${e(v.digest)}</dd></dl><a class="text-link" href="/api/manifest?uri=${encodeURIComponent(m.id)}">Download manifest ↗</a><a class="text-link" href="/api/conformance?uri=${encodeURIComponent(m.id)}">View fixture checks ↗</a></section><section class="inspector-panel"><h3>Provenance</h3><a class="break" href="${e(m.provenance.source)}">${e(m.provenance.source)}</a><p>${m.provenance.derivedFrom.length ? `Derived from ${e(m.provenance.derivedFrom.join(', '))}` : 'Original reference example.'}</p></section></aside></div></section>`;
}
// ThreatCrush scan state for a catalog package (src/scan.mjs).
const SCAN_LABELS = {
  passed: ['No malware found', 'green', '#3f7d3a'],
  flagged: ['Malware flagged', 'red', '#b5432a'],
  error: ['Scan failed', '', '#8a8a80'],
  pending: ['Scan pending', '', '#8a8a80'],
};
const scanLabel = (scan) => SCAN_LABELS[scan?.status] || SCAN_LABELS.pending;
const scanPill = (scan) => {
  const [text, tone] = scanLabel(scan);
  return `<span class="pill scan-badge ${tone}" title="ThreatCrush malware scan">ThreatCrush · ${e(text)}</span>`;
};
export function scanBadgeSvg(scan) {
  const [text, , color] = scanLabel(scan);
  const label = 'threatcrush',
    value = text.toLowerCase();
  const width = (value) => Math.round(value.length * 6.6 + 12);
  const left = width(label),
    right = width(value);
  return `<svg xmlns="http://www.w3.org/2000/svg" width="${left + right}" height="20" role="img" aria-label="${label}: ${e(value)}"><title>${label}: ${e(value)}</title><rect width="${left}" height="20" fill="#2b2b27"/><rect x="${left}" width="${right}" height="20" fill="${color}"/><g fill="#fff" font-family="Verdana,DejaVu Sans,sans-serif" font-size="11" text-anchor="middle"><text x="${left / 2}" y="14">${label}</text><text x="${left + right / 2}" y="14">${e(value)}</text></g></svg>`;
}
const packageLink = (id) => `/packages/${id.split('/').map(encodeURIComponent).join('/')}`;
async function packagesPage(store, query) {
  const filter =
    query.show === 'agentic'
      ? { agentic: true }
      : query.show === 'ours'
        ? { publisher: 'profullstack' }
        : {};
  const items = await store.packages(String(query.q || ''), filter);
  const all = await store.packages('');
  const tab = (show, label, count) =>
    `<a class="pill ${(query.show || '') === show ? 'green' : ''}" href="/packages${show ? `?show=${show}` : ''}">${e(label)} · ${count}</a>`;
  const copy = (text) =>
    `<button class="small-button" type="button" data-copy="${e(text)}">Copy</button>`;
  return `<section class="page-head"><div class="eyebrow">THE W3BS CLI · PACKAGES</div><h1>Install from anywhere.<br><em>Update everything</em> at once.</h1><p><code>w3bs</code> is the W3BS command line, and packages are its first app. Every listed tool installs with its publisher's <code>curl | sh</code>; <code>w3bs packages</code> prints this list and <code>w3bs update</code> keeps everything current. The same CLI resolves, verifies and runs signed <code>w3bs://</code> resources.</p></section>
  <section class="section resource-feature"><div><div class="eyebrow">GET STARTED</div><h2>One line.<br>Then one command.</h2><p>The installer downloads the prebuilt <code>w3bs</code> CLI for your machine (Linux or macOS, x86_64 or arm64), checks it, and lets it install itself from the signed W3BS repository. No Node, Python or Rust needed.</p><a class="text-link" href="/docs/package-manager.md">Read the guide <span aria-hidden="true">→</span></a></div><div class="terminal"><div class="terminal-tabs"><span class="terminal-title">install w3bs</span></div><pre><code>${e(INSTALL)}</code></pre><div class="terminal-result">${copy(INSTALL)}<span class="terminal-label">THEN</span><code>w3bs packages</code><code>w3bs install claude-code</code><code>w3bs update</code></div></div></section>
  <section class="section"><div class="section-heading"><div><div class="eyebrow">THE CATALOG</div><h2>${all.length} tools,<br>one update command.</h2></div><p>Agentic coding tools, MCP servers and the CLIs around them. Search, copy the publisher's own installer, or let <code>w3bs install</code> run it and track it for you.</p></div>
  <form class="search-bar" action="/packages" role="search"><label class="sr-only" for="package-search">Search packages</label><input id="package-search" name="q" type="search" value="${e(query.q || '')}" placeholder="Search by name, vendor or command…">${query.show ? `<input type="hidden" name="show" value="${e(query.show)}">` : ''}<button class="button dark" type="submit">Search <span aria-hidden="true">→</span></button></form>
  <div class="catalog-heading"><span>${tab('', 'All', all.length)} ${tab('agentic', 'Agentic', all.filter((x) => x.agentic).length)} ${tab('ours', 'Profullstack', all.filter((x) => x.publisher === 'profullstack').length)}</span><span>${items.length} shown · curl installers only</span></div>
  <div class="prompt-grid">${
    items
      .map(
        (item) =>
          `<article class="prompt-card" id="${e(item.id)}"><div class="prompt-top">${pill(item.agentic ? 'Agentic' : 'CLI', item.agentic ? 'green' : '')}<span class="verified-small">${e(item.vendor || '')}</span></div><h2><a href="${e(packageLink(item.id))}">${e(item.name)}</a></h2>${scanPill(item.scan)}<p>${e(item.description)}</p><code>${e(item.w3bs)}</code><div class="prompt-bottom"><span>${copy(item.w3bs)}</span>${item.homepage ? `<a href="${e(item.homepage)}" rel="noopener">Site ↗</a>` : ''}</div></article>`,
      )
      .join('') ||
    '<div class="empty"><h2>No matching tools.</h2><p><a href="/packages">See every package</a>.</p></div>'
  }</div>
  <aside class="callout"><strong>Every package is scanned for malware.</strong><p>W3BS runs <a href="https://threatcrush.com" rel="noopener">ThreatCrush</a> over each tool's installer and source when it is submitted, and again whenever it changes: a new version, a different installer or a new commit. Each package page shows the result.</p></aside><aside class="callout"><strong>The listing rule: a curl installer, or you're not listed.</strong><p>Every tool here installs with one <code>curl … | sh</code> from its publisher. The script may use npm or pip inside, but <code>w3bs</code> only ever runs that installer. W3BS records what it ran, re-runs it on <code>w3bs update</code>, and is honest that it cannot roll back or verify a publisher's script. Publishers who want signed releases, pins and rollback can host a W3BS repository with <code>w3bs-publish</code>.</p><a href="/docs/package-manager.md#publishers">List your tool →</a></aside></section>
  <section class="section"><div class="section-heading"><div><div class="eyebrow">HOW IT WORKS</div><h2>Your inventory.<br>Their release hosting.</h2></div><p>Publishers keep their domains, hosting and signing keys. You keep one inventory, one update command, pins, rollback and removal.</p></div><div class="principle-grid"><article><span class="card-index">01 / INSTALL</span><h3>From the website or by name.</h3><p>A publisher's <code>curl | sh</code> and <code>w3bs install</code> run the same engine and write the same receipt.</p></article><article><span class="card-index">02 / VERIFY</span><h3>Signed by the publisher.</h3><p>Native packages come from TUF-signed repositories: every artifact's length and SHA-256 is checked before anything is activated.</p></article><article><span class="card-index">03 / MAINTAIN</span><h3>Update all. Undo one.</h3><p><code>w3bs update</code> respects pins and major versions; <code>w3bs rollback</code> restores the previous files. Machine output with <code>--json</code>.</p></article></div></section>`;
}
function specIndex() {
  return `<section class="page-head"><div class="eyebrow">THE INITIAL STANDARDS SET</div><h1>Common ground.<br><em>Open questions.</em></h1><p>Eight specifications for a full-surface Web. Start with the implemented protocol drafts, then help shape what comes next.</p></section><div class="draft-note">Founding stage · All specifications are proposals or community drafts. No W3BS Recommendations have been issued.</div><div class="spec-list">${specs.map(([id, title, description, status], index) => `<a class="spec-row" href="/specs/${id}"><span class="spec-number">${String(index + 1).padStart(2, '0')}</span><div><span class="eyebrow">${id}</span><h2>${title}</h2><p>${description}</p></div>${pill(status)}<span class="row-arrow" aria-hidden="true">↗</span></a>`).join('')}</div><aside class="callout"><strong>Implementation is part of the standard.</strong><p>A Recommendation requires a conformance suite and multiple independent implementations. The reference stack is an experiment that makes these drafts testable.</p><a href="/developers#conformance">Run the conformance suite →</a></aside>`;
}
function document(file, title) {
  const filename = resolve(ROOT, 'docs', `${file}.md`);
  if (!existsSync(filename)) throw new W3bsError('NOT_FOUND', 'Specification not found.', 404);
  return `<div class="document-top"><a href="/specs">← Standards & documentation</a><a href="/docs/${e(file)}.md">Raw Markdown ↗</a></div><article class="prose">${marked.parse(readFileSync(filename, 'utf8'))}</article>`;
}
const desktopDescription =
  'Agentic Desktop Layer: an open, local-first layer that lets authorized agents use your real desktop with mouse and keyboard, behind a switch you control. KDE Plasma and GNOME first.';
const prdPath = '/docs/agentic-desktop-layer-prd.md';
const desktopPlatforms = [
  ['First', 'KDE Plasma', 'System-tray item or Plasma widget', 'Installable, testing'],
  ['First', 'GNOME', 'Top-panel extension and settings app', 'Installable, testing'],
  ['Next', 'Windows', 'Taskbar notification-area icon', 'Installable, testing'],
  ['Next', 'macOS', 'Menu-bar item', 'Installable, testing'],
  ['Next', 'Cinnamon, Xfce, MATE, LXQt', 'Tray or panel integration', 'Planned'],
  [
    'Expansion',
    'Vela, Sway, Hyprland and others',
    'Capability-based compositor adapters',
    'Planned',
  ],
];
const desktopArchitecture = `Claude Code / OpenCode / Codex / compatible clients
                       |
               MCP / CLI / local API
                       |
       Local authorization and task broker
                       |
      Shared input engine, leases, audit records
                       |
       Desktop and application/service adapters
                       |
                Existing OS and apps

Native tray/panel/menu-bar control
                       |
          Separate human-management channel`;
const DESKTOP_INSTALL = 'curl -fsSL https://w3bs.org/desktop/install.sh | sh';
const DESKTOP_INSTALL_WIN = 'irm https://w3bs.org/desktop/install.ps1 | iex';
const DESKTOP_W3BS = 'w3bs install agentic-desktop-layer';
const DESKTOP_W3BS_BOTH = `${INSTALL}\n${DESKTOP_W3BS}`;
const desktopSteps = [
  [
    'Install',
    DESKTOP_W3BS_BOTH,
    'The first line installs the <a class="inline-link" href="/packages">w3bs CLI</a> (skip it if you have it); the second installs the desktop layer through it, and <code>w3bs update</code> keeps both current. On Windows, in PowerShell: <code>irm https://w3bs.org/desktop/install.ps1 | iex</code>. The installer checks the download against the release checksums, installs <code>w3bs-desktop</code> for your user only, starts the broker in the background with its local MCP endpoint (agent access stays Disabled) and adds the control to your session: the tray icon on KDE Plasma, Windows and other desktops, the panel extension on GNOME (log out and back in once), the menu-bar item on macOS.',
  ],
  [
    'Register your agent',
    'w3bs desktop agents add claude-code --preset broad',
    'Presets: <code>observe</code> (windows and screenshots, no input), <code>restricted --app &lt;app id&gt;</code> (input only into the apps you name) or <code>broad</code> (the whole desktop). Add <code>--approve-for type</code> to approve each typing request yourself. The token is saved privately for the next step.',
  ],
  [
    'Connect it to your agent',
    'w3bs desktop setup claude-code --agent claude-code --http',
    'Prints the line to run: <code>claude mcp add --transport http w3bs-desktop http://127.0.0.1:7801/mcp --header "Authorization: Bearer …"</code>. That connects to the MCP endpoint the running broker serves on 127.0.0.1:7801, so nothing extra starts per session. It answers only on loopback, only with your agent token, and refuses browser requests. Leave off <code>--http</code> to get the stdio form instead (Claude Code starts <code>w3bs desktop mcp serve --stdio</code> itself). Use <code>setup opencode</code> or <code>setup codex</code> for those clients. Nothing goes through the cloud.',
  ],
  [
    'Turn it on',
    'w3bs desktop control',
    'Only you can do this, from the tray icon, the GNOME panel, the macOS menu bar, or <code>w3bs desktop control</code> in a terminal (type <code>enable</code>). The first time, the system asks for permission: allow Remote Control on KDE; on GNOME pick your screen and switch on Allow Remote Interaction; on macOS allow <code>w3bs-desktop</code> under Privacy &amp; Security in Accessibility and Screen Recording (<code>w3bs desktop doctor --prompt</code> opens the prompts). Windows needs no extra permission.<br>In the control, type <code>enable</code> and press Enter. Or skip the terminal and click Enable agent access in the tray, the GNOME panel or the macOS menu bar.<br>For the full picture, choose <strong>Open control panel…</strong> at the top of that menu (or run <code>w3bs desktop control --open</code>): a local app in your browser at <code>127.0.0.1:7801/control</code> with the same switch plus approvals, agents and their permissions, tasks and recent activity. It signs in with a one-time link, never accepts an agent token, and closing it does not turn access off.',
  ],
  [
    'Ask your agent to work',
    'claude "open the text editor and write a short note"',
    'The agent checks the status, starts a task, looks at the windows and screen, then clicks and types into the window it named. Requests that need approval appear in the tray or panel menu and in the control app, with Approve and Deny. The tray&rsquo;s Agents and Recent activity submenus show who has access and what just happened.',
  ],
  [
    'Turn it off',
    'w3bs desktop disable',
    'Or press the emergency shortcut: Super+Alt+Escape on Linux (KDE shows Meta+Alt+Esc), Win+Alt+Esc on Windows, Control+Option+Command+Escape on macOS. You can also run <code>w3bs desktop disable</code> or lock the screen. Everything stops at once, held keys are released, and nothing turns back on by itself.',
  ],
];
const desktopCommands = `w3bs desktop status                 # Disabled / Enabled, who holds the input lease
w3bs desktop doctor                 # session, backend, permissions, extension checks
w3bs desktop control                # terminal control: enable, disable, approve, deny
w3bs desktop control --open         # control app in your browser (one-time sign-in link)
w3bs desktop tray                   # tray control (started for you on login)
w3bs desktop disable                # stop everything now
w3bs desktop agents add|list|update|revoke|remove
w3bs desktop approvals list|approve|deny
w3bs desktop tasks list|inspect|cancel
w3bs desktop audit export|delete    # metadata only, kept 7 days
w3bs desktop layout|windows list|inspect --window ID      --agent NAME
w3bs desktop screen capture --display ID --output shot.png --agent NAME
w3bs desktop mouse move|click|drag|scroll ...              --agent NAME
w3bs desktop keyboard type --stdin|press|shortcut ...      --agent NAME
w3bs desktop mcp serve --stdio --agent NAME
w3bs desktop daemon --mcp-http 127.0.0.1:7801   # broker + MCP over HTTP (installers do this)`;
const desktopUninstall = `# Linux
systemctl --user disable --now w3bs-desktop
rm ~/.local/bin/w3bs-desktop ~/.config/systemd/user/w3bs-desktop.service \\
   ~/.config/autostart/w3bs-desktop-tray.desktop
gnome-extensions uninstall agentic-desktop@w3bs.org   # GNOME only

# macOS
curl -fsSL https://w3bs.org/desktop/install.sh | sh -s -- --uninstall

# Windows (PowerShell)
& ([scriptblock]::Create((irm https://w3bs.org/desktop/install.ps1))) -Uninstall`;
function desktopPage() {
  const prd = readFileSync(resolve(ROOT, 'docs', 'agentic-desktop-layer-prd.md'), 'utf8');
  const shots = `<div class="control-shots"><figure class="control-shot control-shot-main"><img src="/assets/desktop-control-app.webp" width="1280" height="1110" loading="lazy" alt="The control app with agent access Enabled: the active agent and its task, Disable agent access, Stop all agent activity and Quit and disable buttons, the emergency stop shortcut, a pending approval to type into the Kate editor, and three agents with their permission presets."><figcaption>The control app, opened from the tray. Status, the big switches, approvals and every agent's access on one page.</figcaption></figure><figure class="control-shot"><img src="/assets/desktop-control-app-access.webp" width="1280" height="900" loading="lazy" alt="Changing an agent's access in the control app: Observe, Restricted, Broad desktop or Nothing, and the list of apps a restricted agent may use."><figcaption>Change what an agent may do: observe only, restricted to named apps, the whole desktop, or nothing.</figcaption></figure><figure class="control-shot control-shot-phone"><img src="/assets/desktop-control-app-phone.webp" width="780" height="1420" loading="lazy" alt="The control app on a narrow screen: Enabled status and full-width Disable, Stop and Quit buttons."><figcaption>It stays usable in a narrow window.</figcaption></figure><p class="control-shots-note">Real screenshots of the control app running against the layer's built-in demo desktop, so the agents and tasks are sample data. The app only answers on 127.0.0.1 and signs in with a one-time link from the tray or <code>w3bs desktop control --open</code>.</p></div>`;
  return `<section class="page-head desktop-head"><div class="eyebrow">AGENTIC DESKTOP LAYER · W3BS</div><h1>Your desktop.<br>Your agents.<br><em>Your switch.</em></h1><p>An open, local-first layer that lets the agents you authorize use your real desktop: move the mouse, click, drag, type, use shortcuts and read what is on screen. You keep your desktop. A control in your taskbar, panel or menu bar turns it on and off.</p><figure class="desktop-hero-video"><video controls playsinline preload="metadata" poster="/assets/desktop-demo-poster.webp" width="1920" height="1080" aria-describedby="desktop-video-caption"><source src="/assets/desktop-demo.mp4" type="video/mp4"><a class="inline-link" href="/assets/desktop-demo.mp4">Download the demo video (MP4)</a></video><figcaption id="desktop-video-caption" class="muted">Bring agents to your desktop: a 48-second demo.</figcaption></figure><div class="desktop-install-box"><span class="eyebrow">MACOS AND LINUX</span><div class="identity-line"><code>${e(INSTALL)}</code><button class="small-button" type="button" data-copy="${e(INSTALL)}">Copy</button></div><div class="identity-line"><code>${e(DESKTOP_W3BS)}</code><button class="small-button" type="button" data-copy="${e(DESKTOP_W3BS)}">Copy</button></div><span class="eyebrow">WINDOWS (POWERSHELL)</span><div class="identity-line"><code>${e(DESKTOP_INSTALL_WIN)}</code><button class="small-button" type="button" data-copy="${e(DESKTOP_INSTALL_WIN)}">Copy</button></div><p class="muted">The first line installs the <a class="inline-link" href="/packages">w3bs CLI</a> (skip it if you have it), the second the desktop layer; <code>w3bs update</code> keeps both current. Without the w3bs CLI: <code>${e(DESKTOP_INSTALL)}</code></p><div class="actions"><a class="button primary" href="#steps">How to use it <span aria-hidden="true">↓</span></a><a class="text-link" href="#install">What the installer does <span aria-hidden="true">→</span></a></div></div></section><div class="draft-note desktop-status" id="status"><strong>Release status.</strong> M0/M1 done. On KDE Plasma 6.7 and GNOME Shell 50 test rigs, agents type, point, click, take screenshots and target named windows, and the tray, GNOME panel control and emergency stop switch access off. On Windows and macOS the same workflow passes end to end on real GitHub-hosted desktops (Windows 56 of 56 checks, macOS 23 of 23). On GNOME, text with characters the keyboard layout lacks is refused rather than typed partially. Install it through the w3bs CLI, or with PowerShell on Windows (above). No platform is certified yet.</div>
  <section class="section resource-feature" id="install" aria-labelledby="install-title"><div><div class="eyebrow">INSTALL AND USE</div><h2 id="install-title">Install with w3bs.<br>One switch to control.</h2><p>Linux (KDE Plasma, GNOME; x86_64 and arm64), macOS 11 or later (Apple silicon and Intel) and Windows 10 or 11 (x64). Per-user install, no administrator rights. Builds are not code-signed: Windows SmartScreen may warn the first time, and macOS may ask for Accessibility and Screen Recording again after an upgrade.</p><p class="muted">The w3bs CLI is the same one that installs every <a class="inline-link" href="/packages">W3BS package</a>; it runs the desktop layer's own installer and remembers it for <code>w3bs update</code>. Read the installers first if you like: <a class="inline-link" href="/install.sh">install.sh</a> (w3bs CLI), <a class="inline-link" href="/desktop/install.sh">desktop/install.sh</a>, <a class="inline-link" href="/desktop/install.ps1">install.ps1</a>.</p></div><div class="terminal"><div class="terminal-tabs"><span class="terminal-title">install the agentic desktop layer</span></div><pre><code># macOS and Linux
${e(INSTALL)}
${e(DESKTOP_W3BS)}

# Windows (PowerShell)
${e(DESKTOP_INSTALL_WIN)}

# Without the w3bs CLI (macOS and Linux)
${e(DESKTOP_INSTALL)}</code></pre><div class="terminal-result"><button class="small-button" type="button" data-copy="${e(DESKTOP_W3BS_BOTH)}">Copy macOS/Linux</button><button class="small-button" type="button" data-copy="${e(DESKTOP_INSTALL_WIN)}">Copy Windows</button><span class="terminal-label">AGENT ACCESS AFTER INSTALL</span><code>Disabled</code><span class="terminal-muted">You turn it on from the tray, panel or menu bar.</span></div></div></section>
  <section class="section" id="steps" aria-labelledby="steps-title"><div class="section-heading"><div><div class="eyebrow">HOW TO USE IT</div><h2 id="steps-title">Six steps.</h2></div><p>Commands are shown as <code>w3bs desktop …</code>. On Windows, or without the w3bs CLI, type <code>w3bs-desktop …</code> instead; it is the same program. If <code>w3bs</code> says <em>unrecognized subcommand 'desktop'</em>, your CLI is older than 0.1.2: run <code>w3bs self update</code>.</p></div><ol class="desktop-steps">${desktopSteps
    .map(
      ([title, cmd, text]) =>
        `<li><h3>${title}</h3><div class="identity-line"><code>${e(cmd)}</code><button class="small-button" type="button" data-copy="${e(cmd)}">Copy</button></div><p>${text}</p></li>`,
    )
    .join(
      '',
    )}</ol><aside class="callout"><strong>Screenshots on Linux.</strong><p>Desktops ask before an app may take screenshots. Allow it in your privacy settings, or run <code>flatpak permission-set screenshot screenshot "" yes</code>. Without it, screenshot requests are refused instead of popping up a dialog in the middle of agent work.</p></aside></section>
  <section class="section desktop-demo" aria-labelledby="demo-title"><div class="section-heading"><div><div class="eyebrow">THE NATIVE CONTROL</div><h2 id="demo-title">One switch.<br>Always in reach.</h2></div><p>Access starts Disabled after install, login, restart or a crash. Enabling needs a human. Disabling works at any time and stops everything the layer mediates. The tray menu starts with Open control panel…, a local app with the same switch plus approvals, agents and permissions, tasks and activity. This is what it looks like.</p></div>${shots}</section>
  <section class="section"><div class="section-heading"><div><div class="eyebrow">WHAT IT DOES</div><h2>Real input.<br>Real limits.</h2></div><p>Agents need an observe, act, observe loop. They also need a human who can see what is happening and stop it.</p></div><div class="principle-grid"><article><span class="card-index">01 / MOUSE AND KEYBOARD</span><h3>The core, not an add-on.</h3><p>OS-level pointer and keyboard input: move, click, drag, scroll, hover, type, shortcuts and explicit key and button holds. Every coordinate names its space, and stale screenshots are rejected.</p></article><article><span class="card-index">02 / YOUR AGENTS</span><h3>The tools you already use.</h3><p>Claude Code, OpenCode and Codex connect through a local stdio MCP server. The same capability model backs the CLI and the local API. No cloud service sits in the middle.</p></article><article><span class="card-index">03 / YOUR SWITCH</span><h3>Stop means stop.</h3><p>Disable closes the authorization gate first, revokes the input lease, cancels queued work, closes capture streams and releases any keys the layer is holding.</p></article></div></section>
  <section class="section"><div class="section-heading"><div><div class="eyebrow">PLATFORMS</div><h2>KDE and GNOME first.<br><em>Then everywhere.</em></h2></div><p>These are targets, not compatibility claims. A platform is certified only when the full mouse, keyboard and stop workflow passes on it.</p></div><div class="table-wrap"><table class="platform-table"><caption class="sr-only">Platform delivery order</caption><thead><tr><th scope="col">Priority</th><th scope="col">Platform</th><th scope="col">Human control</th><th scope="col">Status</th></tr></thead><tbody>${desktopPlatforms.map(([priority, platform, control, status]) => `<tr><td>${priority}</td><th scope="row">${platform}</th><td>${control}</td><td>${pill(status)}</td></tr>`).join('')}</tbody></table></div><p class="table-note">Wayland first on KDE and GNOME, with X11 tested where the desktop still supports it. Vela support is a clean-room adapter built from public interface facts. No Vela code is used.</p></section>
  <section class="section"><div class="section-heading"><div><div class="eyebrow">ARCHITECTURE</div><h2>One broker.<br>Thin adapters.</h2></div><p>The broker is platform-neutral and runs with your user's privileges. Desktop adapters only report what exists and emit events. Authorization, leases and cancellation stay in the broker, so a new platform cannot weaken them.</p></div><figure class="architecture"><pre aria-describedby="architecture-caption">${e(desktopArchitecture)}</pre><figcaption id="architecture-caption">Agents reach the broker over MCP, the CLI or a local API. The human's native control uses a separate channel that agents cannot call.</figcaption></figure></section>
  <section class="section"><div class="section-heading"><div><div class="eyebrow">PERMISSIONS</div><h2>Authority,<br><em>on purpose.</em></h2></div><p>Mouse, keyboard, screen capture, clipboard and account access are separate, visible grants per agent.</p></div><div class="principle-grid"><article><span class="card-index">RESTRICTED</span><h3>Named apps only.</h3><p>The agent may act only on the applications you granted. Input aimed anywhere else is denied. Keyboard input must name its target window, and focus is checked before every chunk of text.</p></article><article><span class="card-index">BROAD DESKTOP</span><h3>Whole desktop, with a warning.</h3><p>Visual and input control of everything on screen. In this mode business rules cannot always be enforced: a draft-only mail grant does not stop sending through a browser.</p></article><article><span class="card-index">APPROVALS</span><h3>Bound to one exact request.</h3><p>An approval covers one agent, task, target and argument digest, and it expires. Agents cannot enable the layer or approve their own access. Input pauses while an approval is on screen.</p></article></div><aside class="callout"><strong>What disabling cannot undo.</strong><p>Turning the switch off stops new work at once. It cannot take back input already delivered, unsend an email, or erase data an agent already received. It also does not control tools or credentials an agent holds outside this layer.</p></aside></section>
  <section class="section"><div class="section-heading"><div><div class="eyebrow">COMMAND REFERENCE</div><h2>Everything else.</h2></div><p>Run <code>w3bs desktop --help</code> for every option. Text to type always comes from a file or standard input, never from the command line, so it stays out of shell history.</p></div><figure class="architecture"><pre>${e(desktopCommands)}</pre></figure><details class="inspector-panel"><summary>Uninstall</summary><pre class="instructions">${e(desktopUninstall)}</pre></details></section>
  <section class="section prd-share" aria-labelledby="prd-title"><div class="section-heading"><div><div class="eyebrow">THE SPECIFICATION</div><h2 id="prd-title">Read the PRD.</h2></div><p>Version 0.2, October 12, 2026. An implementation specification, not a claim of released software. Source: <a class="inline-link" href="https://github.com/profullstack/w3bs.org/tree/main/desktop">github.com/profullstack/w3bs.org/desktop</a>.</p></div><div class="actions"><button type="button" class="button dark" data-copy-prd="${prdPath}">Copy Markdown</button><a class="button primary" href="${prdPath}" download="agentic-desktop-layer-prd.md">Download PRD <span aria-hidden="true">↓</span></a><a class="text-link" href="${prdPath}">Raw Markdown <span aria-hidden="true">↗</span></a></div><p class="copy-status" role="status" aria-live="polite" data-copy-status></p><div class="copy-fallback" data-copy-fallback hidden><label for="prd-fallback">Copying failed. The Markdown is below: select it all and copy it.</label><textarea id="prd-fallback" readonly rows="14" spellcheck="false"></textarea></div><details class="inspector-panel prd-full"><summary>Read the full PRD on this page</summary><article class="prose">${marked.parse(prd)}</article></details></section>`;
}
// "What do I type?": install with w3bs, then the tool's own first commands from the
// catalog's `examples` (taken from each tool's README or help text). A tool without
// examples still gets its commands and their --help.
function usagePanel(item, copy) {
  const steps = [
    { run: item.w3bs, does: `Install ${item.name} and track it for updates` },
    ...(item.examples?.length
      ? item.examples
      : (item.commands || []).map((command) => ({ run: command, does: `Start ${command}` }))),
  ];
  const main = (item.commands || [])[0];
  return `<section class="inspector-panel usage" id="usage"><h3>Get started</h3><p>What to type once it is installed.</p><ol class="usage-steps">${steps
    .map(
      (step) =>
        `<li><div><code>${e(step.run)}</code><span class="muted">${e(step.does)}</span></div>${copy(step.run)}</li>`,
    )
    .join(
      '',
    )}</ol>${main ? `<p class="muted">Every option: <code>${e(main)} --help</code>${item.homepage ? ` · <a href="${e(item.homepage)}" rel="noopener">docs ↗</a>` : ''}</p>` : ''}</section>`;
}
async function packageDetail(store, id, origin) {
  const item = await store.package(id);
  const scan = item.scan;
  const copy = (text) =>
    `<button class="small-button" type="button" data-copy="${e(text)}">Copy</button>`;
  const badgeUrl = `${origin}/api/packages/${id}/badge.svg`;
  const badgeMarkdown = `[![ThreatCrush scan](${badgeUrl})](${origin}${packageLink(id)})`;
  const counts = scan.summary
    ? ['critical', 'high', 'medium', 'low', 'info']
        .filter((level) => scan.summary[level])
        .map((level) => `${scan.summary[level]} ${level}`)
        .join(' · ') || 'none'
    : '';
  const listFindings = (list) =>
    list
      .map(
        (finding) =>
          `<li class="${finding.malware ? 'malware' : ''}">${pill(finding.malware ? 'Malware indicator' : finding.severity, finding.malware ? 'red' : '')}<div><strong>${e(finding.title)}</strong><code>${e(finding.file)}${finding.line ? `:${e(finding.line)}` : ''}</code><span class="muted">${e(finding.ruleId)}${finding.cwe ? ` · ${e(finding.cwe)}` : ''}</span></div></li>`,
      )
      .join('');
  const malwareFindings = listFindings((scan.findings || []).filter((item) => item.malware));
  const otherCount = (scan.findings || []).filter((item) => !item.malware).length;
  const otherFindings = otherCount
    ? `<details class="scan-other"><summary>${otherCount}${scan.truncated ? '+' : ''} other finding${otherCount === 1 ? '' : 's'} (not malware)</summary><ul class="scan-findings">${listFindings((scan.findings || []).filter((item) => !item.malware))}</ul>${scan.truncated ? '<p class="muted">Showing the first 100 findings.</p>' : ''}</details>`
    : '';
  const explanation = {
    passed: `ThreatCrush found no malware indicators in ${e(item.name)} ${e(scan.version || '')}.${scan.findings?.length ? ' The other findings below are ordinary code-review notes, not malware.' : ''}`,
    flagged:
      'ThreatCrush found code shaped like malware: a hidden payload executed at runtime or secrets sent off the machine. Read the findings before installing. W3BS reviews flagged packages and delists confirmed ones.',
    error: `The last scan could not finish: ${e(scan.error || 'unknown error')}. It is retried on the next pass.`,
    pending:
      'This version has not been scanned yet. Scans run when a package is submitted and whenever it changes.',
  }[scan.status];
  return `<section class="resource-detail"><div class="resource-title"><div><div class="eyebrow"><a href="/packages">PACKAGES</a> / ${e(item.vendor || item.publisher)}</div><h2>${e(item.name)}</h2><p>${e(item.description)}</p></div>${scanPill(scan)}</div><div class="identity-line"><span class="eyebrow">INSTALL</span><code>${e(item.w3bs)}</code>${copy(item.w3bs)}</div><p class="muted install-hint">No <code>w3bs</code> yet? <code>${e(INSTALL)}</code></p><div class="inspector-grid"><div>${usagePanel(item, copy)}<section class="inspector-panel" id="scan"><h3>Malware scan</h3><p>${explanation}</p>${scan.status === 'passed' || scan.status === 'flagged' ? `<dl class="scan-facts"><dt>Scanned</dt><dd>${e(scan.scannedAt?.slice(0, 16).replace('T', ' '))} UTC · ${e(scan.scanner)}</dd><dt>Files</dt><dd>${e(scan.filesScanned)} (installer${scan.sources?.repository ? ` + ${e(scan.sources.repository)}${scan.sources.commit ? `@${e(scan.sources.commit.slice(0, 12))}` : ''}` : ''})</dd><dt>Findings</dt><dd>${e(counts)}${scan.malware ? ` · <strong>${e(scan.malware)} malware indicator${scan.malware === 1 ? '' : 's'}</strong>` : ''}</dd></dl>` : ''}${(scan.notes || []).map((note) => `<p class="muted">${e(note)}</p>`).join('')}${malwareFindings ? `<ul class="scan-findings">${malwareFindings}</ul>` : ''}${otherFindings}</section><section class="inspector-panel"><h3>Badge for your README</h3><p><img src="/api/packages/${e(id)}/badge.svg" alt="ThreatCrush: ${e(scanLabel(scan)[0])}" height="20"></p><pre class="instructions">${e(badgeMarkdown)}</pre>${copy(badgeMarkdown)}</section></div><aside><section class="inspector-panel metadata"><h3>Package details</h3><dl><dt>ID</dt><dd>${e(item.id)}</dd><dt>Version</dt><dd>${e(item.latest_version || 'Publisher managed')}</dd><dt>Vendor</dt><dd>${e(item.vendor || '')}</dd><dt>License</dt><dd>${e(item.license || 'Not stated')}</dd><dt>Commands</dt><dd>${e((item.commands || []).join(', '))}</dd><dt>Kind</dt><dd>${item.agentic ? 'Agentic' : 'CLI'}</dd><dt>Publisher's installer</dt><dd><code class="break">${e(item.install)}</code></dd></dl>${item.homepage ? `<a class="text-link" href="${e(item.homepage)}" rel="noopener">Website ↗</a>` : ''}${item.repository ? `<a class="text-link" href="${e(item.repository)}" rel="noopener">Source ↗</a>` : ''}<a class="text-link" href="/api/packages/${e(id)}">Package JSON ↗</a></section></aside></div></section>`;
}
const DAYS = (ms) => Math.round(ms / (24 * 60 * 60 * 1000));
const UPDATED = '2026-10-15';
const factTable = (caption, rows) =>
  `<div class="table-wrap"><table class="platform-table fact-table"><caption class="sr-only">${e(caption)}</caption><tbody>${rows.map(([label, value]) => `<tr><th scope="row">${label}</th><td>${value}</td></tr>`).join('')}</tbody></table></div>`;
const signupActions = `<div class="actions"><a class="button primary" href="/signup?next=%2Ffleet">Create a free account <span aria-hidden="true">↗</span></a><a class="text-link" href="/fleet">Open your fleet <span aria-hidden="true">→</span></a></div>`;
function pricingPage() {
  const L = FLEET_LIMITS;
  return `<section class="page-head"><div class="eyebrow">PRICING · W3BS FLEET</div><h1>Fleet is <em>free.</em></h1><p>Watch your servers from one page at w3bs.org/fleet. It costs nothing. There is no paid plan and no card to enter, just a username and an email address. Fleet is a beta run by Profullstack, Inc.</p>${signupActions}</section>
  <section class="section" aria-labelledby="included-title"><div class="section-heading"><div><div class="eyebrow">INCLUDED</div><h2 id="included-title">Everything Fleet does.</h2></div><p>There is one plan, and this is all of it.</p></div><div class="principle-grid"><article><span class="card-index">LIVE STATS</span><h3>Every ${L.intervalSeconds} seconds.</h3><p>Memory, disk on the root mount, load, uptime, CPU count, OS, kernel and the w3bs version. Online or offline at a glance.</p></article><article><span class="card-index">ACTIONS</span><h3>Three, if you allow them.</h3><p>Report now, update w3bs packages, restart the desktop layer. Only on machines that joined with actions allowed.</p></article><article><span class="card-index">TERMINAL AND SCREEN</span><h3>Your choice of surface.</h3><p><code>w3bs fleet ls</code>, <code>run</code> and <code>rm</code> from any computer you signed in with <code>w3bs login</code>. Optional screen sharing through a VNC server on the machine.</p></article></div></section>
  <section class="section" aria-labelledby="limits-title"><div class="section-heading"><div><div class="eyebrow">LIMITS</div><h2 id="limits-title">The numbers.</h2></div><p>These come straight from the code that runs Fleet. If one changes, this page changes with it.</p></div>${factTable(
    'Fleet limits',
    [
      ['Machines per account', `${L.machines}`],
      ['Report interval', `Every ${L.intervalSeconds} seconds`],
      ['Marked offline', `After ${L.offlineAfterSeconds} seconds with no report`],
      [
        'Stats history',
        'Latest report only. Each report replaces the one before; Fleet keeps no graphs or history.',
      ],
      ['Join tokens', `One machine each, valid for ${L.tokenMinutes} minutes`],
      ['Actions waiting per machine', `${L.pendingActions}`],
      ['Action timeout', `${L.actionTimeoutMinutes} minutes, then marked failed`],
      [
        'Action history',
        `The newest ${L.actionHistory} per machine, with the last ${L.actionOutputKB} KB of each one's output`,
      ],
      ['Alerts', 'None yet. Fleet does not email or page you when a machine goes offline.'],
      [
        'Platforms',
        'Linux. The agent installs as a systemd service; without systemd, run <code>w3bs fleet agent</code> yourself.',
      ],
      [
        'Sign-in',
        `Web sessions last ${DAYS(TTL.session)} days. <code>w3bs login</code> stays signed in while you use it at least every ${DAYS(TTL.refresh)} days. The agent on each machine uses its own key, which lasts until you remove the machine.`,
      ],
      [
        'Sign-up and sign-in limits',
        'From one IP address: 5 sign-up forms and 10 email-link requests per hour. Per email address: 5 links per hour. After 10 wrong passwords in 15 minutes, the password stops working for 15 minutes or until you sign in with an emailed link.',
      ],
    ],
  )}</section>
  <section class="section" aria-labelledby="promise-title"><div class="section-heading"><div><div class="eyebrow">WHAT WE DO NOT PROMISE</div><h2 id="promise-title">A beta, honestly.</h2></div><p>Read this before you rely on it.</p></div><div class="principle-grid"><article><span class="card-index">UPTIME</span><h3>No uptime guarantee.</h3><p>There is no SLA. Fleet can be down, change or lose data. Keep your own monitoring for anything that matters.</p></article><article><span class="card-index">WHAT IT SEES</span><h3>Only what machines report.</h3><p>Fleet shows what each agent sends. It does not check your servers from outside, so a machine that cannot reach w3bs.org just looks offline.</p></article><article><span class="card-index">PRICE</span><h3>Free today.</h3><p>No paid plan exists. Questions go to <a href="mailto:hello@w3bs.org">hello@w3bs.org</a>. See the <a href="/terms">terms</a> and <a href="/privacy">privacy</a> pages.</p></article></div></section>`;
}
const legalHead = (eyebrow, title, intro) =>
  `<section class="page-head compact"><div class="eyebrow">${eyebrow}</div><h1>${title}</h1><p>${intro}</p></section><div class="draft-note"><strong>Plain-language beta version.</strong> Last updated ${UPDATED}. W3BS is a free beta. This page says what we actually do in plain words; a fuller version may replace it, and changes are posted here with a new date.</div>`;
function termsPage() {
  return `${legalHead('TERMS · BETA', 'Terms of <em>use.</em>', 'The rules for using w3bs.org, the w3bs CLI and W3BS Fleet. Short on purpose.')}<article class="prose legal">
<h2>Who runs this</h2><p>w3bs.org and its services are operated by Profullstack, Inc. Questions: <a href="mailto:hello@w3bs.org">hello@w3bs.org</a>.</p>
<h2>A free beta</h2><p>Accounts, the w3bs CLI, Fleet, the package catalog and W3BS Social are free and in beta. They are provided as they are, with no warranty and no uptime guarantee. We can change, limit or stop any of them. Do not make Fleet your only monitoring.</p>
<h2>Your account</h2><p>Keep your password, passkeys and the machine keys on your servers safe. You are responsible for what is done with your account and on the machines you join to it. Tell us at <a href="mailto:hello@w3bs.org">hello@w3bs.org</a> if you think someone else is using it.</p>
<h2>Machines you join</h2><p>Only join machines you own or are allowed to manage. If you allow actions or screen control when a machine joins, they run on that machine as the user the agent runs as. Turning them on is your choice and your responsibility.</p>
<h2>Fair use</h2><p>Do not use W3BS to attack, probe or break into other people's accounts or machines, send spam, host illegal content, impersonate others, or overload the service. We may suspend or remove accounts and machines that do, and limit use that strains the service.</p>
<h2>Software and packages</h2><p>The code for this site and the w3bs CLI is open source under the MIT license at <a href="https://github.com/profullstack/w3bs.org" rel="noopener">github.com/profullstack/w3bs.org</a>. Tools in the <a href="/packages">package catalog</a> come from their own publishers under their own licenses. W3BS runs their installers and scans them for malware, but does not write or guarantee them.</p>
<h2>Liability</h2><p>As far as the law allows, Profullstack, Inc. is not liable for losses that come from using or not being able to use W3BS, including downtime, lost data or something an action did on your machine.</p>
<h2>Ending</h2><p>You can stop at any time: remove your machines on <a href="/fleet">/fleet</a> and ask us to delete your account. How your data is handled is on the <a href="/privacy">privacy page</a>.</p>
</article>`;
}
function privacyPage() {
  return `${legalHead('PRIVACY · BETA', 'Your <em>data.</em>', 'What W3BS collects, why, how long it stays and how to remove it.')}<article class="prose legal">
<h2>Who we are</h2><p>w3bs.org is operated by Profullstack, Inc. Questions and deletion requests: <a href="mailto:hello@w3bs.org">hello@w3bs.org</a>. We do not sell your data.</p>
<h2>Your account</h2><ul><li>Your username, which is public.</li><li>Your email address, to confirm it and to send sign-in links. It is never shown publicly.</li><li>A hash of your password, if you set one, and the public keys and names of your passkeys.</li><li>Sign-in sessions, a record of sign-in attempts (time and whether it worked), and the apps and CLI sign-ins you approved. Tokens are stored only as hashes.</li></ul>
<h2>What a Fleet machine sends</h2><p>Every ${FLEET_LIMITS.intervalSeconds} seconds, the agent on each machine you join sends: its hostname, OS name and version, kernel, CPU architecture and count, uptime, load averages, total and available memory, total and available disk for the root mount, the w3bs version, whether the desktop layer is installed and running, the user name the agent runs as, and its screen-sharing setting (mode, local VNC address, whether it is reachable). Nothing else in the report is stored.</p><ul><li>Only the latest report is kept. Each one replaces the last.</li><li>Actions you queue are kept with their output (the newest ${FLEET_LIMITS.actionHistory} per machine, last ${FLEET_LIMITS.actionOutputKB} KB of output).</li><li>Screen sharing passes through our server live and is not recorded. A line with its time and result is kept in the machine's history.</li></ul>
<h2>Counting what works</h2><p>The server records three events: an account was created, the w3bs CLI signed in, and an account's first machine reported in. Each row holds the event name, your account id and the time. No IP address, email or hostname.</p>
<h2>Page analytics</h2><p>Public pages load <a href="https://crawlproof.com" rel="noopener">CrawlProof</a> stats, also run by Profullstack. It records the page address, the referring page, screen and window size, language, time zone and a random visitor id kept in your browser's local storage. It sets no cookie. It is not loaded on your account, Fleet, device sign-in or email link pages.</p>
<h2>Email and logs</h2><p>We send email only for sign-up and sign-in links, through our email provider Resend. Mail to hello@w3bs.org is forwarded to us by Forward Email. Sign-up and sign-in forms use your IP address to limit repeated attempts, and our web server may log IP addresses and requested pages for security and debugging.</p>
<h2>How long it stays</h2><ul><li>Account data stays until you ask us to delete the account.</li><li>Web sessions end after ${DAYS(TTL.session)} days; CLI sign-ins after ${DAYS(TTL.refresh)} days unused. Expired ones are deleted.</li><li>Email links expire in ${Math.round(TTL.emailLink / 60000)} minutes and are deleted a day later.</li><li>A machine's stats and history are deleted when you remove it.</li><li>Event rows are kept; deleting your account removes your account id from them.</li></ul>
<h2>Removing your data</h2><p>Remove a machine on <a href="/fleet">/fleet</a> or with <code>w3bs fleet rm</code>. Sign the CLI out on <a href="/account">your account page</a> or with <code>w3bs logout</code>. To delete your account, email <a href="mailto:hello@w3bs.org">hello@w3bs.org</a> from the address on it. That removes your profile, passkeys, sessions, sign-ins, machines and their history. W3BS Social has its own notes on <a href="/social#policy">its page</a>, because posts there are public on the AT Protocol network.</p>
</article>`;
}
export async function renderPage({ path, host, query, store, origin }) {
  if (path === '/')
    path = host.startsWith('browse.')
      ? '/browse'
      : host.startsWith('prompt.')
        ? '/prompts'
        : host.startsWith('specs.')
          ? '/specs'
          : '/mission';
  let content,
    title,
    active = path,
    description =
      'Open standards for a Web where humans, agents and devices can discover, communicate and collaborate. Explore the W3BS community drafts and reference stack.',
    scripts = '';
  if (path === '/mission') {
    title = 'Watch your servers from one page';
    description = `W3BS Fleet: watch your servers from one page. Free, up to ${FLEET_LIMITS.machines} machines. Install the w3bs CLI, sign in, join a server. Also home of the W3BS open standards.`;
    content = await home(store);
  } else if (path === '/prompts') {
    title = 'Open Prompt Registry';
    content = await promptCards(store, query);
  } else if (path === '/browse') {
    title = 'Resource Browser';
    content = await browse(store, query);
  } else if (path === '/packages') {
    title = 'Packages · the w3bs CLI';
    content = await packagesPage(store, query);
  } else if (/^\/packages\/[^/]+\/[^/]+$/.test(path)) {
    let id;
    try {
      id = path.slice('/packages/'.length).split('/').map(decodeURIComponent).join('/');
    } catch {
      throw new W3bsError('NOT_FOUND', 'Page not found.', 404);
    }
    content = await packageDetail(store, id, origin);
    title = `${id} · Packages`;
    active = '/packages';
  } else if (path === '/specs') {
    title = 'Open Standards';
    content = specIndex();
  } else if (path === '/pricing' || path === '/free') {
    title = 'Pricing: Fleet is free';
    description = `W3BS Fleet is free: watch up to ${FLEET_LIMITS.machines} servers from one page, live stats every ${FLEET_LIMITS.intervalSeconds} seconds. No paid plan, no uptime guarantee.`;
    active = '/pricing';
    content = pricingPage();
  } else if (path === '/terms') {
    title = 'Terms of use';
    content = termsPage();
  } else if (path === '/privacy') {
    title = 'Privacy';
    content = privacyPage();
  } else if (path === '/governance') {
    title = 'Governance & Participation';
    content = document('governance', title);
  } else if (path === '/developers') {
    title = 'Build with W3BS';
    content = document('developers', title);
  } else if (path === '/training') {
    title = 'W3BS Training';
    description =
      'Teach agents by doing the task yourself: record it from the Agentic Desktop Layer, review it, and let only the agents you choose retrieve the approved examples over MCP.';
    content = trainingPage();
  } else if (path === '/desktop') {
    title = 'Agentic Desktop Layer';
    description = desktopDescription;
    scripts = '<script type="module" src="/assets/desktop.js"></script>';
    content = desktopPage();
  } else if (/^\/specs\/W3BS-[A-Z]+-1$/.test(path)) {
    title = path.split('/').at(-1);
    content = document(title, title);
    active = '/specs';
  } else throw new W3bsError('NOT_FOUND', 'Page not found.', 404);
  return pageShell({ title, description, active, content, scripts });
}
const TRAINING_INSTALL = 'curl -fsSL https://w3bs.org/training/install.sh | sh';
const TRAINING_INSTALL_WIN = 'irm https://w3bs.org/training/install.ps1 | iex';
const TRAINING_QUICK_DESKTOP = [
  'w3bs update                      # or, first time: curl -fsSL https://w3bs.org/install.sh | sh',
  '                                 #   then: w3bs install agentic-desktop-layer w3bs-training',
  'sudo apt install libsecret-tools # Linux, once: the keyring client',
].join('\n');
const TRAINING_QUICK_AGENT = [
  'w3bs update',
  'claude mcp add --scope user w3bs-training -- \\',
  "  ssh -o BatchMode=yes you@desktop '~/.local/bin/w3bs-training' mcp serve --stdio --consumer claude-code",
  'claude mcp list                  # w3bs-training: Connected',
].join('\n');
function trainingPage() {
  const steps = [
    [
      'Record',
      "In the Agentic Desktop Layer tray, choose <strong>Start training…</strong>, say what you are about to do and tick the apps it may see. It keeps pictures of those apps' focused window only. No keystrokes, typed text, clipboard or audio.",
    ],
    [
      'Review',
      'Each recording becomes a task card: confirm the goal, say how it went (complete, partial, failed), clear privacy warnings, split or merge tasks. Nothing is used until you review it.',
    ],
    [
      'Share with an agent',
      'On the Consumers page, <strong>Share all reviewed tasks</strong> with one named agent (or <code>w3bs training share</code>). It retrieves examples over MCP, as evidence, never as instructions. Private retrieval does not approve model training.',
    ],
    [
      'See if it helped',
      'Agents report each attempt with and without the examples. Failures come back to you, and the Runs page compares the two with confidence intervals instead of claims.',
    ],
  ];
  return `<section class="page-head"><div class="eyebrow">W3BS TRAINING · COMMUNITY DRAFT</div><h1>Teach agents<br><em>by doing it yourself.</em></h1><p>Record a task on your own desktop, review what was captured, and let only the agents you choose learn from the approved examples. Local first: your recordings stay encrypted on your computer unless you push them to your w3bs.org workspace.</p><div class="desktop-install-box"><span class="eyebrow">MACOS AND LINUX</span><div class="identity-line"><code>${e(TRAINING_INSTALL)}</code><button class="small-button" type="button" data-copy="${e(TRAINING_INSTALL)}">Copy</button></div><span class="eyebrow">WINDOWS (POWERSHELL)</span><div class="identity-line"><code>${e(TRAINING_INSTALL_WIN)}</code><button class="small-button" type="button" data-copy="${e(TRAINING_INSTALL_WIN)}">Copy</button></div><p class="muted">One self-contained binary, checked against the release checksums. With the <a class="inline-link" href="/packages">w3bs CLI</a>: <code>w3bs install w3bs-training</code>. On Linux, also install libsecret's <code>secret-tool</code> (<code>sudo apt install libsecret-tools</code>) so the encryption key lives in your desktop keyring. Recording needs the <a class="inline-link" href="/desktop">Agentic Desktop Layer</a>.</p><div class="actions"><a class="button primary" href="/training/app/">Open your workspace <span aria-hidden="true">→</span></a><a class="text-link" href="#how">How it works <span aria-hidden="true">↓</span></a></div></div></section><section id="how" class="section"><div class="section-heading"><div><div class="eyebrow">HOW IT WORKS</div><h2>Four steps.</h2></div></div><ol class="desktop-steps">${steps.map(([t, d]) => `<li><h3>${t}</h3><p>${d}</p></li>`).join('')}</ol></section><section id="claude" class="section training-quick"><div class="section-heading"><div><div class="eyebrow">QUICK START</div><h2>Use it from Claude Code.</h2></div></div><p>Record on your desktop; Claude Code can run on another machine and reads the shared tasks over ssh. Recordings never leave the desktop.</p><h3>On the desktop</h3><pre><code>${e(TRAINING_QUICK_DESKTOP)}</code></pre><p>Then record (tray → <strong>Start training…</strong> → <strong>Finish task</strong>), review in <code>w3bs training open</code>, and click <strong>Consumers → Share all reviewed tasks</strong>.</p><h3>On the agent machine</h3><pre><code>${e(TRAINING_QUICK_AGENT)}</code></pre><p>Then ask Claude to use w3bs-training to find how you did the task. <a class="inline-link" href="https://github.com/profullstack/w3bs.org/blob/main/docs/training/README.md#quick-start-record-on-your-desktop-use-it-from-claude-code" rel="noopener">Full quick start</a>.</p></section><section class="section"><div class="section-heading"><div><div class="eyebrow">WHERE YOUR DATA LIVES</div><h2>Two places, one format.</h2></div></div><p><strong>On your computer:</strong> <code>w3bs training open</code> runs the review app locally, from a one-time link; frames are AES-256-GCM encrypted with a key from your keyring. <strong>On w3bs.org:</strong> your <a class="inline-link" href="/training/app/">workspace</a> is the same app, signed in with your W3BS account, in its own database schema. Send reviewed tasks there with <code>w3bs training push</code> to review on another device or to give a hosted agent access. The server can decrypt what you push to it; keep anything you would not hand to a server on your own machine.</p><p>Both speak the open <a class="inline-link" href="https://github.com/profullstack/w3bs.org/blob/main/docs/training/format-0.1.md" rel="noopener">W3BS Demonstration Dataset Format 0.1</a>, so you can export a bundle, a JSONL dataset or next-action training examples (with no future state in the input) and take them anywhere.</p></section><div class="draft-note"><strong>Status.</strong> Recording is verified on KDE Plasma; GNOME, Windows and macOS use the same code but have not been run through the rig. Recordings are screenshots of approved windows (the <code>visual-review</code> profile), so they help an agent see how a task is done but do not qualify as action-level training data. No model-training integration is offered yet. Read the <a class="inline-link" href="https://github.com/profullstack/w3bs.org/tree/main/docs/training" rel="noopener">docs and privacy guide</a>.</div>`;
}
export const escapeHtml = e;
// CrawlProof page analytics (crawlproof.com/stats.js). The id is the CrawlProof project
// for w3bs.org. It is rendered only when the app runs as https://w3bs.org, so local runs
// and CI do not count as traffic; W3BS_CRAWLPROOF_SITE overrides it ('' turns it off).
export const CRAWLPROOF_SITE_ID = '2400d7b5-128e-4142-b9aa-a4331085d07a';
export function trackerSite(env = process.env) {
  if (env.W3BS_CRAWLPROOF_SITE !== undefined) return env.W3BS_CRAWLPROOF_SITE.trim();
  return (env.PUBLIC_ORIGIN || '').replace(/\/+$/, '') === 'https://w3bs.org'
    ? CRAWLPROOF_SITE_ID
    : '';
}
const tracker = (site) =>
  /^[0-9a-f-]{36}$/i.test(site)
    ? `<script async src="https://crawlproof.com/stats.js" data-site="${site}" data-careers="off"></script>`
    : '';
export function pageShell({
  title,
  description = 'Open standards for a Web where humans, agents and devices can discover, communicate and collaborate.',
  active = '',
  content,
  scripts = '',
  robots = '',
  // Off for pages whose address or buttons can carry account data (email links, device
  // codes, machine names): stats.js reports the full URL and the text of clicked buttons.
  track = true,
}) {
  return `<!doctype html><html lang="en"><head><meta charset="utf-8"><meta name="viewport" content="width=device-width, initial-scale=1"><title>${e(title)} — W3BS</title><meta name="description" content="${e(description)}">${robots ? `<meta name="robots" content="${e(robots)}">` : ''}<meta name="theme-color" content="#f6f4ed"><link rel="icon" type="image/svg+xml" href="/assets/mark.svg"><link rel="manifest" href="/manifest.webmanifest"><link rel="stylesheet" href="/assets/style.css"><script type="module" src="/assets/app.js"></script>${scripts}${track ? tracker(trackerSite()) : ''}</head><body>${header(active)}<main id="main">${content}</main>${footer()}<div id="toast" class="toast" role="status" aria-live="polite" hidden></div></body></html>`;
}
