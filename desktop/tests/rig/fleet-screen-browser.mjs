// fleet-screen.sh helper: drives the real /fleet and /fleet/<id>/screen pages in headless
// Chromium. node fleet-screen-browser.mjs <phase> <state.json> <shots dir> <text to type>
//   view     connect, screenshot, then force the page's control switch on and type: the
//            agent's view-only gate must drop it
//   control  connect, switch control on, type into the remote terminal
import { readFileSync } from 'node:fs';
import { chromium } from '@playwright/test';

const [phase, stateFile, shots, text] = process.argv.slice(2);
const { origin, cookie } = JSON.parse(readFileSync(stateFile, 'utf8'));
const browser = await chromium.launch();
const context = await browser.newContext({ viewport: { width: 1440, height: 900 } });
await context.addCookies([{ name: 'w3bs_session', value: cookie, url: origin }]);
const page = await context.newPage();
const problems = [];
page.on('pageerror', (e) => problems.push(String(e)));
page.on('console', (m) => m.type() === 'error' && problems.push(m.text()));

await page.goto(`${origin}/fleet`);
const link = page.locator('a.fleet-screen-link');
await link.waitFor({ timeout: 15000 });
await page.screenshot({ path: `${shots}/fleet-${phase}.png`, fullPage: true });
await link.click();
await page.waitForURL(/\/fleet\/m_[^/]+\/screen$/);

const password = page.locator('#screen-message input[type=password]');
await password.waitFor({ timeout: 20000 });
await password.fill(process.env.VNC_PASSWORD);
await page.locator('#screen-message button[type=submit]').click();
await page.locator('#screen-state', { hasText: 'Live' }).waitFor({ timeout: 20000 });
await page.waitForTimeout(2500);
await page.screenshot({ path: `${shots}/screen-${phase}.png` });

// Turn input on in the page. In view mode the switch is hidden, so unhide it: this is the
// "page that skips its own view-only switch" case the agent's gate exists for.
await page.evaluate(() => {
  document.getElementById('screen-control-label').hidden = false;
  const box = document.getElementById('screen-control');
  box.checked = true;
  box.dispatchEvent(new Event('change'));
});
await page.locator('#screen canvas').click({ position: { x: 300, y: 200 } });
await page.keyboard.type(text, { delay: 30 });
await page.keyboard.press('Enter');
await page.waitForTimeout(2500);
await page.screenshot({ path: `${shots}/screen-${phase}-typed.png` });

const state = await page.locator('#screen-state').textContent();
const mode = await page.locator('#screen-mode').textContent();
console.log(JSON.stringify({ phase, state, mode, problems }));
await browser.close();
