// w3bs.org/packages/publish and /api/publish: signed releases with SSH keys (W3BS-SIGN-1,
// src/publishing.mjs). Writes take the session or a `w3bs login` bearer; reads are public.
import { W3bsError } from './core.mjs';
import { escapeHtml as e } from './site.mjs';
import { KEY_NAMESPACE, RELEASE_NAMESPACE } from './publishing.mjs';

const KEY_STEPS = (namespace = 'example.com') =>
  [
    'w3bs login',
    `w3bs publish key add ${namespace}          # signs a challenge with ~/.ssh/id_ed25519`,
    `# then publish the line it prints, either at`,
    `#   https://${namespace}/.well-known/w3bs-publisher.txt`,
    `# or as a DNS TXT record on _w3bs.${namespace}, and run`,
    'w3bs publish key verify',
  ].join('\n');
const RELEASE_STEPS = [
  'w3bs publish release --package example.com/tool --version 1.2.0 \\',
  '  --install "curl -fsSL https://example.com/install.sh | sh" \\',
  '  --description "What the tool does, in one sentence."',
].join('\n');
const PLAIN_STEPS = [
  '# The same with only OpenSSH and curl (TOKEN is a w3bs.org session or w3bs login bearer)',
  'curl -s -H "Authorization: Bearer $TOKEN" https://w3bs.org/api/publish/keys/challenge',
  `printf %s "w3bs-key:<you>:<challenge>" | ssh-keygen -Y sign -n ${KEY_NAMESPACE} -f ~/.ssh/id_ed25519`,
  '# POST /api/publish/keys {public_key, namespace, challenge, signature}',
  `ssh-keygen -Y sign -n ${RELEASE_NAMESPACE} -f ~/.ssh/id_ed25519 release.jcs.json`,
  '# POST /api/publish/releases {statement, signature}',
].join('\n');

export function mountPublishing(app, { store, helpers }) {
  const { accountUser, currentUser, page, onMainHost } = helpers;
  const publishing = store.publishing;
  const signedIn = async (req) => {
    const user = await accountUser(req);
    if (!user) throw new W3bsError('SIGNED_OUT', 'Sign in first (w3bs login).', 401);
    return user;
  };

  app.get('/api/publish/keys/challenge', async (req, res) =>
    res.json(await publishing.challenge(await signedIn(req))),
  );
  app.get('/api/publish/keys', async (req, res) =>
    res.json(await publishing.listKeys((await signedIn(req)).id)),
  );
  app.post('/api/publish/keys', async (req, res) =>
    res.json(await publishing.addKey(await signedIn(req), req.body ?? {})),
  );
  app.post('/api/publish/keys/verify', async (req, res) =>
    res.json(await publishing.verifyKey((await signedIn(req)).id, req.body?.fingerprint)),
  );
  app.delete('/api/publish/keys/:fingerprint', async (req, res) =>
    res.json(await publishing.revokeKey((await signedIn(req)).id, req.params.fingerprint)),
  );
  app.post('/api/publish/releases', async (req, res) =>
    res.status(201).json(await publishing.submitRelease(await signedIn(req), req.body ?? {})),
  );
  app.get('/api/publish/catalog.json', async (_req, res) =>
    res.json(await publishing.catalogEntries()),
  );
  app.get('/api/publish/releases/:namespace/:name', async (req, res) =>
    res.json({
      releases: await publishing.releases(`${req.params.namespace}/${req.params.name}`),
    }),
  );
  app.get('/api/publish/releases/:namespace/:name/:version', async (req, res) => {
    const id = `${req.params.namespace}/${req.params.name}`;
    const release = (await publishing.releases(id)).find(
      (r) => r.statement.version === req.params.version,
    );
    if (!release) throw new W3bsError('NOT_FOUND', `No signed ${id} ${req.params.version}.`, 404);
    res.json(release);
  });

  app.get('/packages/publish', onMainHost, async (req, res) => {
    const user = await currentUser(req);
    const keys = user ? (await publishing.listKeys(user.id)).keys : [];
    const keyRows = keys.length
      ? `<div class="table-wrap"><table class="platform-table"><thead><tr><th>Namespace</th><th>Key</th><th>Domain</th></tr></thead><tbody>${keys
          .map(
            (k) =>
              `<tr><td>${e(k.namespace)}</td><td><code class="break">${e(k.fingerprint)}</code><br><span class="muted">${e(k.type)}</span></td><td>${k.revoked_at ? 'Revoked' : k.domain_verified ? 'Verified' : `Not verified yet: publish <code>${e(k.proof.line)}</code> at <code class="break">${e(k.proof.well_known)}</code> or TXT <code>${e(k.proof.dns)}</code>, then <code>w3bs publish key verify</code>`}</td></tr>`,
          )
          .join('')}</tbody></table></div>`
      : user
        ? '<p class="muted">No keys on your account yet.</p>'
        : '<p class="muted"><a class="inline-link" href="/login?next=%2Fpackages%2Fpublish">Sign in</a> to see your publisher keys.</p>';
    page(res, {
      title: 'Publish a package',
      active: '/packages',
      content: `<section class="page-head compact"><div class="eyebrow">PACKAGES · PUBLISH</div><h1>Sign your releases<br><em>with your SSH key.</em></h1><p>List your tool on <a class="inline-link" href="/packages">w3bs.org/packages</a> with a signature only you can make. You sign with the SSH key you already have; anyone can check it with <code>ssh-keygen -Y verify</code>; <code>w3bs install</code> refuses an installer that does not match what you signed. Ed25519 and ECDSA keys, including hardware (FIDO) keys, are accepted.</p></section><section class="inspector-panel"><h3>1. Register a key for your domain</h3><pre class="instructions">${e(KEY_STEPS())}</pre><p>Your domain is the namespace: packages you sign are <code>your-domain/name</code>. Proving the domain lists your key makes your account its publisher.</p></section><section class="inspector-panel"><h3>2. Publish a release</h3><pre class="instructions">${e(RELEASE_STEPS)}</pre><p>The CLI downloads your installer, records its sha256 in a release statement, signs it with <code>ssh-keygen -Y sign -n ${RELEASE_NAMESPACE}</code> and submits it. Versions are immutable.</p></section><section class="inspector-panel"><h3>Your publisher keys</h3>${keyRows}</section><section class="inspector-panel"><h3>No w3bs CLI? OpenSSH is enough</h3><pre class="instructions">${e(PLAIN_STEPS)}</pre><p>The full format is in the open spec, <a class="inline-link" href="/docs/W3BS-SIGN-1.md">W3BS-SIGN-1</a>.</p></section>`,
    });
  });
}
