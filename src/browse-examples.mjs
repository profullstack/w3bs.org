// One-click examples on /browse. Add a site here and it shows up in the grid.
// Every Moshpit name below resolved over dns.moshcode.sh and answered on its pit.world gateway
// when it was added (2026-10-19); drop one that stops answering rather than list a dead link.

// Moshpit names: <label>.<ending>, not in the ICANN root. Each also answers at
// https://<name>.pit.world for browsers without Moshpit DNS.
export const MOSHPIT_EXAMPLES = [
  { name: 'chovy.hacker', blurb: 'A personal Moshpit name, served from a Profullstack box.' },
  { name: 'pay.agent', blurb: 'Get paid in crypto with one link, on CoinPay.' },
  { name: 'built.indie', blurb: 'Launch an indie product with one URL.' },
  { name: 'seo.rank', blurb: 'A free on-page SEO audit.' },
  { name: 'live.streaming', blurb: 'Live stations, streaming now.' },
  { name: 'family.ancestry', blurb: 'Public family trees of people, animals and companies.' },
  { name: 'search.peptides', blurb: 'Every research peptide vendor, compared.' },
  { name: 'profullstack.agent', blurb: "Profullstack's agent namespace on Moshpit." },
  { name: 'demo.indie', blurb: 'A site served from a Moshpit server.' },
];

// ICANN sites in the Profullstack network, shown as they are.
export const WEB_EXAMPLES = [
  { host: 'w3bs.org', blurb: 'This site: Fleet, packages and the W3BS drafts.' },
  { host: 'profullstack.com', blurb: 'Agentic engineering: the studio behind the network.' },
  { host: 'hqtui.com', blurb: 'Terminal UIs for TypeScript, Rust, Go, Python and Zig.' },
  { host: 'threatcrush.com', blurb: 'A security agent for Linux servers; it scans W3BS packages.' },
  { host: 'crawlproof.com', blurb: 'See your site the way AI crawlers do.' },
  { host: 'coinpayportal.com', blurb: 'Crypto payments, escrow and wallets.' },
  { host: 'moshcode.sh', blurb: 'moshcode, and the Moshpit registry and resolver.' },
  { host: 'pit.world', blurb: 'Private search, and the gateway to every Moshpit name.' },
  { host: 'ad.network', blurb: 'Run ads or earn from your site, in crypto.' },
];
