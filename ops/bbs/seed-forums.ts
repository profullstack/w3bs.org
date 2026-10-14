/**
 * The w3bs.org/bbs forum tree and branding: the W3BS community board (tsbb).
 *
 * The board lives in ~/www/w3bs.org-bbs on dev2 (container w3bsorg-bbs-app-1,
 * 127.0.0.1:3341, vault w3bs-org-bbs--prod), mounted under /bbs by the w3bs.org
 * vhost (cli-tools dev2/sites.d/w3bs.org.json). It runs inside the tsbb
 * container, against the board's own database:
 *
 *   docker cp ops/bbs/seed-forums.ts w3bsorg-bbs-app-1:/app/data/seed-forums.ts
 *   docker exec w3bsorg-bbs-app-1 node /app/data/seed-forums.ts
 *   docker restart w3bsorg-bbs-app-1      # settings are cached per process
 *
 * Idempotent. Forums are matched by slug, so re-running adds what is missing
 * and leaves admin edits (names, order) alone. Settings are overwritten.
 */
const CHECKOUT = process.env.TSBB_CHECKOUT_DIR ?? '/app';
const { migrate } = await import(`${CHECKOUT}/packages/db/src/migrate.ts`);
const { one, run, now } = await import(`${CHECKOUT}/packages/db/src/client.ts`);
const { seed } = await import(`${CHECKOUT}/packages/db/src/seed.ts`);

type Forum = { slug: string; name: string; description: string };
type Category = { slug: string; name: string; forums: Forum[] };

const TREE: Category[] = [
  {
    slug: 'w3bs',
    name: 'W3BS',
    forums: [
      { slug: 'announcements', name: 'Announcements', description: 'Releases, spec drafts and news from the W3BS initiative.' },
      { slug: 'introductions', name: 'Introductions', description: 'New here? Say who you are, what you build, and which agents you run.' },
      { slug: 'feedback', name: 'Feedback & Bugs', description: 'Something broken on w3bs.org, the w3bs CLI or the desktop layer? Report it here.' },
    ],
  },
  {
    slug: 'build',
    name: 'Build',
    forums: [
      { slug: 'desktop-agents', name: 'Desktop Agents', description: 'The Agentic Desktop Layer: letting Claude Code, OpenCode and Codex use a real desktop, safely. Setups, rigs, wins and failures.' },
      { slug: 'cli-packages', name: 'w3bs CLI & Packages', description: 'Installing, publishing and signing tools with the w3bs CLI and the curl-installable package catalog.' },
      { slug: 'mcp-tooling', name: 'MCP & Agent Tooling', description: 'MCP servers, agent permissions, sandboxing and the plumbing between agents and the things they touch.' },
      { slug: 'prompts', name: 'Prompt Registry', description: 'Signed prompts, the w3bs:// resolver and sharing prompts that agents can verify.' },
    ],
  },
  {
    slug: 'standards',
    name: 'Standards',
    forums: [
      { slug: 'spec-drafts', name: 'Spec Drafts', description: 'Discuss the open W3BS drafts for humans, agents and devices. Propose changes, poke holes.' },
      { slug: 'proposals', name: 'Proposals', description: 'Pitch a new standard or a missing piece of the agentic web.' },
    ],
  },
  {
    slug: 'community',
    name: 'Community',
    forums: [
      { slug: 'show-and-tell', name: 'Show & Tell', description: 'Share what you built with agents: demos, repos, screenshots, war stories.' },
      { slug: 'help', name: 'Help & Questions', description: 'Stuck? Ask. Someone has probably hit the same wall.' },
      { slug: 'off-topic', name: 'Off Topic', description: 'Anything else.' },
    ],
  },
];

const SETTINGS: Record<string, unknown> = {
  'board.name': 'W3BS Community',
  'board.tagline': 'Open standards for the agentic web, discussed by the people building it.',
  'board.description':
    'The W3BS community forums: desktop agents, the w3bs CLI and packages, MCP and agent tooling, and the open W3BS spec drafts for humans, agents and devices.',
  // `paper` is the w3bs.org look (paper/ink, Georgia headlines). Until the board
  // runs a tsbb release that has it, tsbb falls back to its default skin.
  'board.skin': 'paper',
  'board.accent': '#d95231',
  'board.logoUrl': 'https://w3bs.org/assets/mark.svg',
  'board.logoHref': 'https://w3bs.org/mission',
  'board.faviconUrl': 'https://w3bs.org/assets/mark.svg',
  // The tsbb platform pitch on the index is for boards that are about tsbb.
  'board.showPlatform': false,
  // w3bs.org has no dark mode, so neither does its forum by default.
  'board.theme': 'light',
  // The site's own header navigation, with the board's items where Community sits.
  'board.navLinks': [
    'Packages | https://w3bs.org/packages',
    'Standards | https://w3bs.org/specs',
    'Browse | https://w3bs.org/browse',
    'Prompts | https://w3bs.org/prompts',
    'Desktop | https://w3bs.org/desktop',
    '{forums}',
    'Participate | https://w3bs.org/governance',
  ].join('\n'),
};

await migrate(undefined, { quiet: true });

async function upsertForum(parentId: number | null, kind: string, slug: string, name: string, description: string | null, position: number) {
  const existing = await one<{ id: number }>('SELECT id FROM forums WHERE slug = ?', [slug]);
  if (existing) return Number(existing.id);
  const result = await run(
    `INSERT INTO forums (parent_id, kind, slug, name, description, position, created_at)
     VALUES (?, ?, ?, ?, ?, ?, ?) RETURNING id`,
    [parentId, kind, slug, name, description, position, now()],
  );
  console.log(`  + ${kind} ${slug}`);
  return Number((result.rows[0] as unknown as { id: number }).id);
}

for (const [c, category] of TREE.entries()) {
  const parent = await upsertForum(null, 'category', category.slug, category.name, null, c);
  for (const [f, forum] of category.forums.entries()) {
    await upsertForum(parent, 'forum', forum.slug, forum.name, forum.description, f);
  }
}

for (const [key, value] of Object.entries(SETTINGS)) {
  await run(
    `INSERT INTO settings (key, value, updated_at) VALUES (?, ?, ?)
     ON CONFLICT (key) DO UPDATE SET value = excluded.value, updated_at = excluded.updated_at`,
    [key, JSON.stringify(value), now()],
  );
}
console.log('  settings');

await seed({ quiet: true });
console.log('Done. Restart the container so the running board reloads its settings.');
process.exit(0);
