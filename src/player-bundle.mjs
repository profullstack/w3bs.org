/**
 * @profullstack/player for the browser, served from w3bs.org's own origin (script-src 'self').
 *
 * w3bs.org has no client build step: public/ is plain JS. So the player is bundled here, once, the
 * first time it is asked for, into one ES module (/assets/pf-player.js) plus its stylesheet
 * (/assets/pf-player.css). public/pf-player-init.js imports it only on a page with a
 * [data-pf-player] host. Same pattern as pit.world's src/player-bundle.ts.
 */
import { readFileSync } from 'node:fs';
import { createRequire } from 'node:module';

let js = null;
let css = null;

export function playerJs() {
  js ??= (async () => {
    const out = await Bun.build({
      entrypoints: [new URL('./player-entry.mjs', import.meta.url).pathname],
      target: 'browser',
      format: 'esm',
      splitting: false,
      minify: true,
    });
    if (!out.success)
      throw new Error(`player bundle failed: ${out.logs.map((l) => l.message).join('; ')}`);
    return await out.outputs[0].text();
  })().catch((error) => {
    js = null;
    throw error;
  });
  return js;
}

export function playerCss() {
  css ??= readFileSync(
    createRequire(import.meta.url).resolve('@profullstack/player/player.css'),
    'utf8',
  );
  return css;
}
