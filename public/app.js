const codeExamples = {
  cli: 'node src/cli.mjs resolve \\\n  w3bs://prompt/w3bs/research@1',
  api: 'curl --get https://w3bs.org/api/resolve \\\n  --data-urlencode "uri=w3bs://prompt/w3bs/research@1"',
  mcp: 'Tool: w3bs_resolve\n\n{ "uri": "w3bs://prompt/w3bs/research@1" }',
};
const tabs = [...document.querySelectorAll('[data-example]')];
function selectTab(tab) {
  for (const button of tabs) {
    button.setAttribute('aria-selected', String(button === tab));
    button.tabIndex = button === tab ? 0 : -1;
  }
  const panel = document.querySelector('#code-example');
  panel.textContent = codeExamples[tab.dataset.example];
  panel.setAttribute('aria-labelledby', tab.id);
}
for (const tab of tabs) {
  tab.addEventListener('click', () => selectTab(tab));
  tab.addEventListener('keydown', (event) => {
    if (!['ArrowLeft', 'ArrowRight', 'Home', 'End'].includes(event.key)) return;
    event.preventDefault();
    const index =
      event.key === 'Home'
        ? 0
        : event.key === 'End'
          ? tabs.length - 1
          : (tabs.indexOf(tab) + (event.key === 'ArrowRight' ? 1 : -1) + tabs.length) % tabs.length;
    selectTab(tabs[index]);
    tabs[index].focus();
  });
}
let toastTimer;
function toast(message) {
  const element = document.querySelector('#toast');
  element.textContent = message;
  element.hidden = false;
  clearTimeout(toastTimer);
  toastTimer = setTimeout(() => {
    element.hidden = true;
  }, 3000);
}
for (const button of document.querySelectorAll('[data-copy]'))
  button.addEventListener('click', async () => {
    try {
      await navigator.clipboard.writeText(button.dataset.copy);
      toast('URI copied.');
    } catch {
      toast('Clipboard unavailable. Select and copy the URI above.');
    }
  });
const runForm = document.querySelector('#run-form');
runForm?.addEventListener('submit', async (event) => {
  event.preventDefault();
  const button = runForm.querySelector('button[type=submit]'),
    status = document.querySelector('#run-status'),
    output = document.querySelector('#run-output');
  button.disabled = true;
  status.textContent = 'Rendering…';
  output.hidden = true;
  try {
    const inputs = Object.fromEntries(
      [...runForm.querySelectorAll('textarea')].map((input) => [input.name, input.value]),
    );
    const response = await fetch('/api/run', {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({
        uri: runForm.dataset.uri,
        inputs,
        consent: document.querySelector('#run-consent').checked,
      }),
    });
    const result = await response.json();
    if (!response.ok) throw new Error(result.error?.message || 'Rendering failed.');
    output.textContent = result.output;
    output.hidden = false;
    status.textContent = 'Instructions rendered. No model or tool was called.';
  } catch (error) {
    status.textContent = error.message;
  } finally {
    button.disabled = false;
  }
});
if ('serviceWorker' in navigator) navigator.serviceWorker.register('/sw.js').catch(() => {});
