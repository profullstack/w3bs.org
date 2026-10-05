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
    ['/specs', 'Standards'],
    ['/browse', 'Browse'],
    ['/prompts', 'Prompts'],
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
  return `<footer><div><a class="wordmark" href="/mission">w3bs<span class="wordmark-dot">.</span></a><p>Human-friendly. Agent-native.<br>Device-friendly. Open by design.</p></div><div><span class="eyebrow">THE INITIATIVE</span><a href="/governance">Governance & participation</a><a href="/docs/founding-prd.md">Founding PRD · v2.0</a><a href="/docs/roadmap.md">Implementation status</a></div><div><span class="eyebrow">FOR IMPLEMENTERS</span><a href="/developers">Developer guide</a><a href="/.well-known/w3bs.json">Machine discovery ↗</a><a href="/api">API reference ↗</a></div><div class="footer-bottom"><span>Open standards for the Agentic Era.</span><span>Community drafts · No Recommendations issued</span></div></footer>`;
}
async function home(store) {
  const count = (await store.search()).length;
  return `<section class="hero"><div class="hero-copy"><div class="eyebrow"><span class="status-dot"></span> AN OPEN STANDARDS INITIATIVE</div><h1>The open Web.<br>For <em>all</em> of us.</h1><p class="hero-sub">Like Nostr but for everything</p><p class="hero-body">A Web where every participant can discover, communicate and collaborate. Open protocols. Portable resources. Authority you can inspect.</p><div class="actions"><a class="button primary" href="/browse?uri=${encodeURIComponent(canonicalExample)}">Explore the open Web <span aria-hidden="true">↗</span></a><a class="text-link" href="/specs">Read the standards <span aria-hidden="true">→</span></a></div><div class="hero-note"><span class="status-dot"></span> Founding stage. Built in the open.</div></div><div class="hero-visual" aria-label="One W3BS resource connects humans, agents and devices through multiple interfaces"><div class="visual-heading"><span>ONE IDENTITY. EVERY INTERFACE.</span><span class="visual-cross">+</span></div><div class="orbit"><svg class="orbit-lines" viewBox="0 0 440 340" aria-hidden="true"><ellipse cx="220" cy="170" rx="184" ry="118"/><ellipse cx="220" cy="170" rx="118" ry="155" transform="rotate(55 220 170)"/><path d="M50 170H390M220 40V300"/></svg><span class="actor human"><span aria-hidden="true">◉</span> Humans</span><span class="actor agent"><span aria-hidden="true">✳</span> Agents</span><span class="actor device"><span aria-hidden="true">▣</span> Devices</span><span class="center-mark">w3bs<span>://</span></span><span class="orbit-dot dot-one"></span><span class="orbit-dot dot-two"></span></div><div class="uri-window"><div><span class="status-dot"></span> CANONICAL RESOURCE <span class="window-dots">···</span></div><code>w3bs://<span>prompt</span>/w3bs/research@1</code><p>Same identity. Same manifest. Same meaning.</p></div><div class="surface-strip"><span>CLI</span><span>API</span><span>MCP</span><span>WEB</span><span>NATIVE</span></div></div></section>
  <div class="principles-strip"><span>OPEN BY DEFAULT</span><span>VENDOR NEUTRAL</span><span>VERIFIABLE BY DESIGN</span><span>BUILT TO INTEROPERATE</span></div>
  <section class="section"><div class="section-heading"><div><div class="eyebrow">A SHARED FOUNDATION</div><h2>The Web is getting<br>new participants.</h2></div><p>Pages became applications. Applications are becoming collaborators. The next chapter needs common ground that belongs to everyone.</p></div><div class="principle-grid"><article><span class="card-index">01 / DISCOVER</span><h3>A common address.</h3><p>A <code>w3bs://</code> URI identifies the resource. The interface, registry and transport can change without changing what it means.</p><a href="/specs/W3BS-URI-1">Explore identifiers <span aria-hidden="true">↗</span></a></article><article><span class="card-index">02 / UNDERSTAND</span><h3>Nothing hidden.</h3><p>Inspect who published it, what it contains, which permissions it needs and whether its signature checks out.</p><a href="/specs/W3BS-MANIFEST-1">Inside a manifest <span aria-hidden="true">↗</span></a></article><article><span class="card-index">03 / COLLABORATE</span><h3>Authority, on purpose.</h3><p>Content is data by default. A signature proves provenance; running instructions still requires explicit authorization.</p><a href="/specs/W3BS-TRUST-1">Understand trust <span aria-hidden="true">↗</span></a></article></div></section>
  <section class="section resource-feature"><div><div class="eyebrow">TRY THE REFERENCE STACK</div><h2>One resource.<br>Every way in.</h2><p>Resolve a signed research prompt through the web, CLI, API, MCP or experimental native client. Compare the canonical manifest across surfaces.</p><a class="text-link" href="${resourceLink(canonicalExample)}">Inspect the example <span aria-hidden="true">→</span></a><div class="resource-count"><strong>${count.toString().padStart(2, '0')}</strong><span>signed, versioned prompts<br>ready to inspect and reuse</span></div></div><div class="terminal"><div class="terminal-tabs" role="tablist" aria-label="Access examples"><button type="button" role="tab" aria-selected="true" id="tab-cli" aria-controls="code-example" data-example="cli">CLI</button><button type="button" role="tab" aria-selected="false" tabindex="-1" id="tab-api" aria-controls="code-example" data-example="api">API</button><button type="button" role="tab" aria-selected="false" tabindex="-1" id="tab-mcp" aria-controls="code-example" data-example="mcp">MCP</button><span class="terminal-title">w3bs / reference</span></div><pre id="code-example" role="tabpanel" aria-labelledby="tab-cli"><code>node src/cli.mjs resolve \\\n  w3bs://prompt/w3bs/research@1</code></pre><div class="terminal-result"><span class="terminal-label">RESOLVES TO</span><code>prompt/w3bs/research@1.0.0</code><span><span class="status-dot"></span> Signature verified</span><span class="terminal-muted">Authority: DATA · Permissions: none</span></div></div></section>
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
function specIndex() {
  return `<section class="page-head"><div class="eyebrow">THE INITIAL STANDARDS SET</div><h1>Common ground.<br><em>Open questions.</em></h1><p>Eight specifications for a full-surface Web. Start with the implemented protocol drafts, then help shape what comes next.</p></section><div class="draft-note">Founding stage · All specifications are proposals or community drafts. No W3BS Recommendations have been issued.</div><div class="spec-list">${specs.map(([id, title, description, status], index) => `<a class="spec-row" href="/specs/${id}"><span class="spec-number">${String(index + 1).padStart(2, '0')}</span><div><span class="eyebrow">${id}</span><h2>${title}</h2><p>${description}</p></div>${pill(status)}<span class="row-arrow" aria-hidden="true">↗</span></a>`).join('')}</div><aside class="callout"><strong>Implementation is part of the standard.</strong><p>A Recommendation requires a conformance suite and multiple independent implementations. The reference stack is an experiment that makes these drafts testable.</p><a href="/developers#conformance">Run the conformance suite →</a></aside>`;
}
function document(file, title) {
  const filename = resolve(ROOT, 'docs', `${file}.md`);
  if (!existsSync(filename)) throw new W3bsError('NOT_FOUND', 'Specification not found.', 404);
  return `<div class="document-top"><a href="/specs">← Standards & documentation</a><a href="/docs/${e(file)}.md">Raw Markdown ↗</a></div><article class="prose">${marked.parse(readFileSync(filename, 'utf8'))}</article>`;
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
    active = path;
  if (path === '/mission') {
    title = 'Like Nostr but for everything';
    content = await home(store);
  } else if (path === '/prompts') {
    title = 'Open Prompt Registry';
    content = await promptCards(store, query);
  } else if (path === '/browse') {
    title = 'Resource Browser';
    content = await browse(store, query);
  } else if (path === '/specs') {
    title = 'Open Standards';
    content = specIndex();
  } else if (path === '/governance') {
    title = 'Governance & Participation';
    content = document('governance', title);
  } else if (path === '/developers') {
    title = 'Build with W3BS';
    content = document('developers', title);
  } else if (/^\/specs\/W3BS-[A-Z]+-1$/.test(path)) {
    title = path.split('/').at(-1);
    content = document(title, title);
    active = '/specs';
  } else throw new W3bsError('NOT_FOUND', 'Page not found.', 404);
  return `<!doctype html><html lang="en"><head><meta charset="utf-8"><meta name="viewport" content="width=device-width, initial-scale=1"><title>${e(title)} — W3BS</title><meta name="description" content="Open standards for a Web where humans, agents and devices can discover, communicate and collaborate. Explore the W3BS community drafts and reference stack."><meta name="theme-color" content="#f6f4ed"><link rel="icon" type="image/svg+xml" href="/assets/mark.svg"><link rel="manifest" href="/manifest.webmanifest"><link rel="stylesheet" href="/assets/style.css"><script type="module" src="/assets/app.js"></script></head><body>${header(active)}<main id="main">${content}</main>${footer()}<div id="toast" class="toast" role="status" aria-live="polite" hidden></div></body></html>`;
}
