import { test, expect } from '@playwright/test';
test('homepage navigation, code tabs and narrow layouts work', async ({ page }, testInfo) => {
  const errors = [];
  page.on('pageerror', (error) => errors.push(error.message));
  await page.goto('/');
  await expect(page.getByRole('heading', { level: 1 })).toContainText('The open Web.');
  await page.getByRole('tab', { name: 'API', exact: true }).click();
  await expect(page.getByRole('tabpanel')).toContainText('curl');
  await page.getByRole('tab', { name: 'API', exact: true }).press('ArrowRight');
  await expect(page.getByRole('tab', { name: 'MCP', exact: true })).toHaveAttribute(
    'aria-selected',
    'true',
  );
  expect(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth)).toBe(true);
  await page.screenshot({ path: `artifacts/home-${testInfo.project.name}.png`, fullPage: true });
  await page.getByRole('navigation').getByRole('link', { name: 'Standards' }).click();
  await expect(page.getByRole('heading', { level: 1 })).toContainText('Common ground.');
  await page.getByRole('link', { name: /W3BS-URI-1/ }).click();
  await expect(page.getByRole('heading', { level: 1 })).toContainText('W3BS-URI-1');
  expect(errors).toEqual([]);
});
test('search a prompt, resolve the canonical artifact and render with consent', async ({
  page,
}, testInfo) => {
  await page.goto('/prompts');
  await page.getByRole('searchbox').fill('research');
  await page.getByRole('button', { name: /Search/ }).click();
  await page.getByRole('link', { name: /Research with receipts/ }).click();
  await expect(
    page.getByRole('heading', { name: 'Research with receipts', exact: true }),
  ).toBeVisible();
  const web = await page.locator('#w3bs-resource').textContent();
  const api = await (
    await page.request.get('/api/resolve?uri=w3bs://prompt/w3bs/research@1.0.0')
  ).json();
  expect(JSON.parse(web)).toEqual(api);
  await page.getByRole('textbox', { name: /topic required/ }).fill('Interoperable devices');
  await page.getByRole('checkbox').check();
  await page.getByRole('button', { name: /Render instructions/ }).click();
  await expect(page.locator('#run-output')).toContainText('Interoperable devices');
  await expect(page.locator('#run-status')).toHaveText(
    'Instructions rendered. No model or tool was called.',
  );
  expect(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth)).toBe(true);
  await page.screenshot({
    path: `artifacts/inspector-${testInfo.project.name}.png`,
    fullPage: true,
  });
});
test('empty and invalid URI states stay useful', async ({ page }) => {
  await page.goto('/prompts?q=does-not-exist-82751');
  await expect(page.getByRole('heading', { name: 'No matching resources.' })).toBeVisible();
  await page.goto('/browse?uri=not-a-w3bs-uri');
  await expect(page.getByRole('alert')).toContainText('Could not resolve');
});
test('PWA provides an honest offline state instead of cached verification', async ({
  page,
  context,
}) => {
  await page.goto('/browse');
  await page.evaluate(async () => {
    const registration = await navigator.serviceWorker.ready;
    if (!registration.active) throw new Error('worker inactive');
  });
  await page.reload();
  await page.waitForFunction(() => navigator.serviceWorker.controller !== null);
  await context.setOffline(true);
  await page.goto('/browse?uri=w3bs://prompt/w3bs/research@1');
  await expect(page.getByRole('heading', { level: 1 })).toContainText('A connection');
  await expect(page.locator('body')).toContainText('revocation checks need a live connection');
});
