import { readFileSync, existsSync } from 'node:fs';
import { resolve } from 'node:path';
import { marked } from 'marked';
import { ROOT } from './store.mjs';
import { W3bsError } from './core.mjs';

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
    ['/governance', 'Participate'],
  ]
    .map(
      ([href, label]) =>
        `<a href="${href}" ${active === href ? 'aria-current="page"' : ''}>${label}</a>`,
    )
    .join(
      '',
    )}</nav><a class="header-action" href="/developers">Start building <span aria-hidden="true">↗</span></a></header>`;
}
function footer() {
  return `<footer><div><a class="wordmark" href="/mission">w3bs<span class="wordmark-dot">.</span></a><p>Human-friendly. Agent-native.<br>Device-friendly. Open by design.</p></div><div><span class="eyebrow">THE INITIATIVE</span><a href="/governance">Governance & participation</a><a href="/docs/founding-prd.md">Founding PRD · v2.0</a><a href="/docs/roadmap.md">Implementation status</a><a href="/desktop">Agentic Desktop Layer</a></div><div><span class="eyebrow">FOR IMPLEMENTERS</span><a href="/developers">Developer guide</a><a href="/.well-known/w3bs.json">Machine discovery ↗</a><a href="/api">API reference ↗</a></div><div class="footer-bottom"><span>Open standards for the Agentic Era.</span><span>Community drafts · No Recommendations issued</span></div></footer>`;
}
async function home(store) {
  const count = (await store.search()).length;
  return `<section class="hero"><div class="hero-copy"><div class="eyebrow"><span class="status-dot"></span> AN OPEN STANDARDS INITIATIVE</div><h1>The open Web.<br>For <em>all</em> of us.</h1><p class="hero-sub">Like Nostr but for everything</p><p class="hero-body">A Web where every participant can discover, communicate and collaborate. Open protocols. Portable resources. Authority you can inspect.</p><div class="actions"><a class="button primary" href="/browse?uri=${encodeURIComponent(canonicalExample)}">Explore the open Web <span aria-hidden="true">↗</span></a><a class="text-link" href="/specs">Read the standards <span aria-hidden="true">→</span></a></div><div class="hero-note"><span class="status-dot"></span> Founding stage. Built in the open.</div></div><div class="hero-visual" aria-label="One W3BS resource connects humans, agents and devices through multiple interfaces"><div class="visual-heading"><span>ONE IDENTITY. EVERY INTERFACE.</span><span class="visual-cross">+</span></div><div class="orbit"><svg class="orbit-lines" viewBox="0 0 440 340" aria-hidden="true"><ellipse cx="220" cy="170" rx="184" ry="118"/><ellipse cx="220" cy="170" rx="118" ry="155" transform="rotate(55 220 170)"/><path d="M50 170H390M220 40V300"/></svg><span class="actor human"><span aria-hidden="true">◉</span> Humans</span><span class="actor agent"><span aria-hidden="true">✳</span> Agents</span><span class="actor device"><span aria-hidden="true">▣</span> Devices</span><span class="center-mark">w3bs<span>://</span></span><span class="orbit-dot dot-one"></span><span class="orbit-dot dot-two"></span></div><div class="uri-window"><div><span class="status-dot"></span> CANONICAL RESOURCE <span class="window-dots">···</span></div><code>w3bs://<span>prompt</span>/w3bs/research@1</code><p>Same identity. Same manifest. Same meaning.</p></div><div class="surface-strip"><span>CLI</span><span>API</span><span>MCP</span><span>WEB</span><span>NATIVE</span></div></div></section>
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
const INSTALL = 'curl -fsSL https://w3bs.org/install.sh | sh';
async function packagesPage(store, query) {
  const filter = query.show === 'agentic' ? { agentic: true } : query.show === 'ours' ? { publisher: 'profullstack' } : {};
  const items = await store.packages(String(query.q || ''), filter);
  const all = await store.packages('');
  const tab = (show, label, count) =>
    `<a class="pill ${(query.show || '') === show ? 'green' : ''}" href="/packages${show ? `?show=${show}` : ''}">${e(label)} · ${count}</a>`;
  const copy = (text) => `<button class="small-button" type="button" data-copy="${e(text)}">Copy</button>`;
  return `<section class="page-head"><div class="eyebrow">THE W3BS CLI · PACKAGES</div><h1>Install from anywhere.<br><em>Update everything</em> at once.</h1><p><code>w3bs</code> is the W3BS command line, and packages are its first app. Every listed tool installs with its publisher's <code>curl | sh</code>; <code>w3bs packages</code> prints this list and <code>w3bs update</code> keeps everything current. The same CLI resolves, verifies and runs signed <code>w3bs://</code> resources.</p></section>
  <section class="section resource-feature"><div><div class="eyebrow">GET STARTED</div><h2>One line.<br>Then one command.</h2><p>The installer downloads the prebuilt <code>w3bs</code> CLI for your machine (Linux or macOS, x86_64 or arm64), checks it, and lets it install itself from the signed W3BS repository. No Node, Python or Rust needed.</p><a class="text-link" href="/docs/package-manager.md">Read the guide <span aria-hidden="true">→</span></a></div><div class="terminal"><div class="terminal-tabs"><span class="terminal-title">install w3bs</span></div><pre><code>${e(INSTALL)}</code></pre><div class="terminal-result">${copy(INSTALL)}<span class="terminal-label">THEN</span><code>w3bs packages</code><code>w3bs install claude-code</code><code>w3bs update</code></div></div></section>
  <section class="section"><div class="section-heading"><div><div class="eyebrow">THE CATALOG</div><h2>${all.length} tools,<br>one update command.</h2></div><p>Agentic coding tools, MCP servers and the CLIs around them. Search, copy the publisher's own installer, or let <code>w3bs install</code> run it and track it for you.</p></div>
  <form class="search-bar" action="/packages" role="search"><label class="sr-only" for="package-search">Search packages</label><input id="package-search" name="q" type="search" value="${e(query.q || '')}" placeholder="Search by name, vendor or command…">${query.show ? `<input type="hidden" name="show" value="${e(query.show)}">` : ''}<button class="button dark" type="submit">Search <span aria-hidden="true">→</span></button></form>
  <div class="catalog-heading"><span>${tab('', 'All', all.length)} ${tab('agentic', 'Agentic', all.filter((x) => x.agentic).length)} ${tab('ours', 'Profullstack', all.filter((x) => x.publisher === 'profullstack').length)}</span><span>${items.length} shown · curl installers only</span></div>
  <div class="prompt-grid">${
    items
      .map(
        (item) =>
          `<article class="prompt-card" id="${e(item.id)}"><div class="prompt-top">${pill(item.agentic ? 'Agentic' : 'CLI', item.agentic ? 'green' : '')}<span class="verified-small">${e(item.vendor || '')}</span></div><h2>${e(item.name)}</h2><p>${e(item.description)}</p><code>${e(item.install)}</code><div class="prompt-bottom"><span>${copy(item.install)} <code>w3bs install ${e(item.id)}</code></span>${item.homepage ? `<a href="${e(item.homepage)}" rel="noopener">Site ↗</a>` : ''}</div></article>`,
      )
      .join('') || '<div class="empty"><h2>No matching tools.</h2><p><a href="/packages">See every package</a>.</p></div>'
  }</div>
  <aside class="callout"><strong>The listing rule: a curl installer, or you're not listed.</strong><p>Every tool here installs with one <code>curl … | sh</code> from its publisher. The script may use npm or pip inside, but <code>w3bs</code> only ever runs that installer. W3BS records what it ran, re-runs it on <code>w3bs update</code>, and is honest that it cannot roll back or verify a publisher's script. Publishers who want signed releases, pins and rollback can host a W3BS repository with <code>w3bs-publish</code>.</p><a href="/docs/package-manager.md#publishers">List your tool →</a></aside></section>
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
  ['First', 'KDE Plasma', 'System-tray item or Plasma widget', 'In development'],
  ['First', 'GNOME', 'Top-panel extension and settings app', 'In development'],
  ['Next', 'Windows', 'Taskbar notification-area icon', 'Planned'],
  ['Next', 'macOS', 'Menu-bar item', 'Planned'],
  ['Next', 'Cinnamon, Xfce, MATE, LXQt', 'Tray or panel integration', 'Planned'],
  [
    'Expansion',
    'Vela, Sway, Hyprland and others',
    'Capability-based compositor adapters',
    'Planned',
  ],
];
const desktopMenuItems = [
  ['agents', 'Agents and permissions…'],
  ['tasks', 'Current tasks…'],
  ['activity', 'Activity and approvals…'],
  ['apps', 'Applications and accounts…'],
  ['settings', 'Settings…'],
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
const desktopCommands = `w3bs desktop status --json
w3bs desktop enable
w3bs desktop disable
w3bs desktop agents add claude-code
w3bs desktop windows list --json
w3bs desktop mouse click --button left
w3bs desktop keyboard shortcut --window WINDOW_ID --keys Ctrl+S
w3bs desktop mcp serve --stdio`;
function desktopPage() {
  const prd = readFileSync(resolve(ROOT, 'docs', 'agentic-desktop-layer-prd.md'), 'utf8');
  const demo = `<div class="demo-frame" data-desktop-demo><p class="demo-label"><span aria-hidden="true">◇</span> Interactive preview - does not control your device</p><div class="demo-screen"><div class="demo-panel"><span class="demo-apps" aria-hidden="true">▤ Files · Editor · Browser</span><span class="demo-clock" aria-hidden="true">09:41</span><button type="button" class="demo-tray" id="demo-tray" aria-haspopup="true" aria-expanded="false" aria-controls="demo-menu"><span class="demo-icon" data-demo-icon aria-hidden="true">○</span><span class="sr-only">Agentic Desktop Layer preview menu. Status: </span><span data-demo-chip>Disabled</span></button></div><div class="demo-menu" id="demo-menu" role="group" aria-labelledby="demo-menu-title" hidden><p class="demo-menu-title" id="demo-menu-title">Agentic Desktop Layer</p><p class="demo-status">Status: <strong data-demo-state>Disabled</strong></p><p class="demo-detail" data-demo-detail hidden></p><button type="button" class="button dark demo-toggle" data-demo-toggle>Enable agent access</button><ul class="demo-items">${desktopMenuItems.map(([key, label]) => `<li><button type="button" data-demo-item="${key}">${label}</button></li>`).join('')}</ul><div class="demo-danger"><button type="button" data-demo-stop>Stop all agent activity</button><button type="button" data-demo-quit>Quit and disable</button></div></div></div><div class="demo-footer"><button type="button" class="button primary" data-demo-emergency>Emergency stop</button><p class="demo-log" role="status" aria-live="polite" data-demo-log>Preview state: Disabled. Nothing on this page talks to your computer.</p></div><noscript><p class="demo-log">The preview needs JavaScript. The menu it shows is listed in section 4 of the PRD below.</p></noscript></div>`;
  return `<section class="page-head desktop-head"><div class="eyebrow">AGENTIC DESKTOP LAYER · W3BS</div><h1>Your desktop.<br>Your agents.<br><em>Your switch.</em></h1><p>An open, local-first layer that lets the agents you authorize use your real desktop: move the mouse, click, drag, type, use shortcuts and read what is on screen. You keep your desktop. A control in your taskbar, panel or menu bar turns it on and off.</p></section><div class="draft-note desktop-status" id="status"><strong>Release status.</strong> M0/M1 done. Real keyboard, pointer and screenshots work on a KDE Plasma 6.7 test rig through the desktop portals. GNOME not yet tested. No certified platform. No downloads yet.</div>
  <section class="section desktop-demo" aria-labelledby="demo-title"><div class="section-heading"><div><div class="eyebrow">THE NATIVE CONTROL</div><h2 id="demo-title">One switch.<br>Always in reach.</h2></div><p>Access starts Disabled after install, login, restart or a crash. Enabling needs a human. Disabling works at any time and stops everything the layer mediates. Try the menu below. It is a simulation.</p></div>${demo}</section>
  <section class="section"><div class="section-heading"><div><div class="eyebrow">WHAT IT DOES</div><h2>Real input.<br>Real limits.</h2></div><p>Agents need an observe, act, observe loop. They also need a human who can see what is happening and stop it.</p></div><div class="principle-grid"><article><span class="card-index">01 / MOUSE AND KEYBOARD</span><h3>The core, not an add-on.</h3><p>OS-level pointer and keyboard input: move, click, drag, scroll, hover, type, shortcuts and explicit key and button holds. Every coordinate names its space, and stale screenshots are rejected.</p></article><article><span class="card-index">02 / YOUR AGENTS</span><h3>The tools you already use.</h3><p>Claude Code, OpenCode and Codex connect through a local stdio MCP server. The same capability model backs the CLI and the local API. No cloud service sits in the middle.</p></article><article><span class="card-index">03 / YOUR SWITCH</span><h3>Stop means stop.</h3><p>Disable closes the authorization gate first, revokes the input lease, cancels queued work, closes capture streams and releases any keys the layer is holding.</p></article></div></section>
  <section class="section"><div class="section-heading"><div><div class="eyebrow">PLATFORMS</div><h2>KDE and GNOME first.<br><em>Then everywhere.</em></h2></div><p>These are targets, not compatibility claims. A platform is certified only when the full mouse, keyboard and stop workflow passes on it.</p></div><div class="table-wrap"><table class="platform-table"><caption class="sr-only">Platform delivery order</caption><thead><tr><th scope="col">Priority</th><th scope="col">Platform</th><th scope="col">Human control</th><th scope="col">Status</th></tr></thead><tbody>${desktopPlatforms.map(([priority, platform, control, status]) => `<tr><td>${priority}</td><th scope="row">${platform}</th><td>${control}</td><td>${pill(status)}</td></tr>`).join('')}</tbody></table></div><p class="table-note">Wayland first on KDE and GNOME, with X11 tested where the desktop still supports it. Vela support is a clean-room adapter built from public interface facts. No Vela code is used.</p></section>
  <section class="section"><div class="section-heading"><div><div class="eyebrow">ARCHITECTURE</div><h2>One broker.<br>Thin adapters.</h2></div><p>The broker is platform-neutral and runs with your user's privileges. Desktop adapters only report what exists and emit events. Authorization, leases and cancellation stay in the broker, so a new platform cannot weaken them.</p></div><figure class="architecture"><pre aria-describedby="architecture-caption">${e(desktopArchitecture)}</pre><figcaption id="architecture-caption">Agents reach the broker over MCP, the CLI or a local API. The human's native control uses a separate channel that agents cannot call.</figcaption></figure></section>
  <section class="section"><div class="section-heading"><div><div class="eyebrow">PERMISSIONS</div><h2>Authority,<br><em>on purpose.</em></h2></div><p>Mouse, keyboard, screen capture, clipboard and account access are separate, visible grants per agent.</p></div><div class="principle-grid"><article><span class="card-index">RESTRICTED</span><h3>Named apps only.</h3><p>The agent may act only on the applications you granted. Input aimed anywhere else is denied. Keyboard input must name its target window, and focus is checked before every chunk of text.</p></article><article><span class="card-index">BROAD DESKTOP</span><h3>Whole desktop, with a warning.</h3><p>Visual and input control of everything on screen. In this mode business rules cannot always be enforced: a draft-only mail grant does not stop sending through a browser.</p></article><article><span class="card-index">APPROVALS</span><h3>Bound to one exact request.</h3><p>An approval covers one agent, task, target and argument digest, and it expires. Agents cannot enable the layer or approve their own access. Input pauses while an approval is on screen.</p></article></div><aside class="callout"><strong>What disabling cannot undo.</strong><p>Turning the switch off stops new work at once. It cannot take back input already delivered, unsend an email, or erase data an agent already received. It also does not control tools or credentials an agent holds outside this layer.</p></aside></section>
  <section class="section"><div class="section-heading"><div><div class="eyebrow">COMMAND LINE</div><h2>Proposed, not yet installable.</h2></div><p>The <code>w3bs desktop</code> namespace as specified. These commands are not install instructions. They will ship through the W3BS Package Manager when a platform passes certification.</p></div><figure class="architecture"><figcaption class="eyebrow">PROPOSED, NOT YET INSTALLABLE</figcaption><pre>${e(desktopCommands)}</pre></figure></section>
  <section class="section prd-share" aria-labelledby="prd-title"><div class="section-heading"><div><div class="eyebrow">THE SPECIFICATION</div><h2 id="prd-title">Read the PRD.</h2></div><p>Version 0.2, October 12, 2026. An implementation specification, not a claim of released software. Source: <a class="inline-link" href="https://github.com/profullstack/w3bs.org/tree/main/desktop">github.com/profullstack/w3bs.org/desktop</a>.</p></div><div class="actions"><button type="button" class="button dark" data-copy-prd="${prdPath}">Copy Markdown</button><a class="button primary" href="${prdPath}" download="agentic-desktop-layer-prd.md">Download PRD <span aria-hidden="true">↓</span></a><a class="text-link" href="${prdPath}">Raw Markdown <span aria-hidden="true">↗</span></a></div><p class="copy-status" role="status" aria-live="polite" data-copy-status></p><div class="copy-fallback" data-copy-fallback hidden><label for="prd-fallback">Copying failed. The Markdown is below: select it all and copy it.</label><textarea id="prd-fallback" readonly rows="14" spellcheck="false"></textarea></div><details class="inspector-panel prd-full"><summary>Read the full PRD on this page</summary><article class="prose">${marked.parse(prd)}</article></details></section>`;
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
    title = 'Like Nostr but for everything';
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
  } else if (path === '/specs') {
    title = 'Open Standards';
    content = specIndex();
  } else if (path === '/governance') {
    title = 'Governance & Participation';
    content = document('governance', title);
  } else if (path === '/developers') {
    title = 'Build with W3BS';
    content = document('developers', title);
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
  return `<!doctype html><html lang="en"><head><meta charset="utf-8"><meta name="viewport" content="width=device-width, initial-scale=1"><title>${e(title)} — W3BS</title><meta name="description" content="${e(description)}"><meta name="theme-color" content="#f6f4ed"><link rel="icon" type="image/svg+xml" href="/assets/mark.svg"><link rel="manifest" href="/manifest.webmanifest"><link rel="stylesheet" href="/assets/style.css"><script type="module" src="/assets/app.js"></script>${scripts}</head><body>${header(active)}<main id="main">${content}</main>${footer()}<div id="toast" class="toast" role="status" aria-live="polite" hidden></div></body></html>`;
}
