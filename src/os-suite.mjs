// W3BS OS M0: resolve the Profullstack catalog entries into the draft suite manifest
// (docs/os-prd.md §5, §13 M0). scripts/os-suite.mjs writes the snapshot to os/suite-manifest.json.
// §5 named inclusions and the surfaces each must install.
export const NAMED = {
  'moshcode.sh/moshcode': ['cli'],
  'pairux.com/pairux': ['desktop', 'cli'],
  'diskpush.com/diskpush': ['desktop', 'cli'],
  'w3bs.org/agentic-desktop-layer': ['desktop', 'cli'],
  'w3bs.org/w3bs-training': ['cli'],
  'threatcrush.com/threatcrush': ['desktop', 'cli'],
  'nixamp.com/nixamp': ['desktop', 'cli'],
  'hdtilt.com/hdtilt': ['desktop', 'cli'],
  'tronbrowser.dev/tronbrowser': ['desktop', 'cli'],
  'nightcell7.com/nightcell7': ['desktop'],
  'mynaposter.com/myna': ['desktop', 'cli'],
  'r4ck.dev/r4ck': ['desktop', 'cli'],
};

// §5 collisions: one canonical owner per command; the other package keeps an explicit alternate.
export const COMMAND_OWNERS = {
  openmcp: 'logicsrc.com/openmcp',
  proxy: 'profullstack.com/proxy',
};

// Desktop products in the Profullstack inventory without a catalog entry (§5 reconcile).
export const UNCATALOGED = ['media2markdown'];

export function buildSuite(catalog, { catalogRevision = null } = {}) {
  const ours = catalog
    .filter((p) => p.vendor === 'Profullstack' && p.publisher === 'profullstack')
    .sort((a, b) => a.id.localeCompare(b.id));
  const owners = {};
  for (const p of ours) for (const c of p.commands ?? []) (owners[c] ??= []).push(p.id);
  const blocking = [];
  const entries = ours.map((p) => {
    const required = NAMED[p.id] ?? ['cli'];
    const curl = /^curl\b/.test(p.install ?? '');
    const commands = (p.commands ?? []).map((name) => {
      const claimants = owners[name];
      if (claimants.length < 2) return { name, owner: p.id };
      const owner = COMMAND_OWNERS[name];
      return owner === p.id
        ? { name, owner: p.id, alternates: claimants.filter((id) => id !== p.id) }
        : { name, owner, shadowed: true, invoke_as: `${p.name} ${name}` };
    });
    const tasks = [];
    if (!p.latest_version) tasks.push('pin a release version');
    if (curl)
      tasks.push(
        'publish a signed artifact (deb or W3BS native) for linux-x86_64 and linux-aarch64',
      );
    if (required.includes('desktop'))
      tasks.push('qualify the Linux desktop artifact and .desktop entry');
    tasks.push('record checksum, license, executables and receipt owner');
    tasks.push('pass CLI discovery + launch check in a W3BS OS image');
    for (const t of tasks) blocking.push({ id: p.id, task: t });
    return {
      id: p.id,
      name: p.name,
      named: Boolean(NAMED[p.id]),
      required_surfaces: required,
      commands,
      version: p.latest_version ?? null,
      repository: p.repository ?? null,
      license: p.license ?? null,
      install_today: curl ? 'publisher-script' : 'other',
      update_owner: null,
      targets: { 'linux-x86_64': 'unqualified', 'linux-aarch64': 'unqualified' },
      status: 'unresolved',
    };
  });
  for (const name of UNCATALOGED)
    blocking.push({
      id: name,
      task: 'add a catalog entry (desktop product missing from the catalog)',
    });
  const missingNamed = Object.keys(NAMED).filter((id) => !ours.some((p) => p.id === id));
  for (const id of missingNamed)
    blocking.push({ id, task: 'named inclusion missing from the catalog' });
  return {
    format: 'w3bs-os-suite/0.1',
    milestone: 'M0',
    status: 'draft',
    catalog_revision: catalogRevision,
    summary: {
      entries: entries.length,
      named: entries.filter((e) => e.named).length,
      publisher_scripts: entries.filter((e) => e.install_today === 'publisher-script').length,
      unpinned: entries.filter((e) => !e.version).length,
      command_collisions: Object.entries(owners)
        .filter(([, ids]) => ids.length > 1)
        .map(([name, ids]) => ({ name, claimants: ids, owner: COMMAND_OWNERS[name] ?? null })),
      missing_named: missingNamed,
      uncataloged: UNCATALOGED,
      blocking_tasks: blocking.length,
    },
    entries,
    blocking,
  };
}
