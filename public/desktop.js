// Agentic Desktop Layer page. The only network call on this page is the same-origin fetch of
// the PRD Markdown for "Copy Markdown".
const copyButton = document.querySelector('[data-copy-prd]');
if (copyButton) {
  const status = document.querySelector('[data-copy-status]'),
    fallback = document.querySelector('[data-copy-fallback]'),
    area = fallback.querySelector('textarea');
  copyButton.addEventListener('click', async () => {
    let markdown = '';
    status.textContent = 'Copying…';
    try {
      const response = await fetch(copyButton.dataset.copyPrd, {
        headers: { Accept: 'text/markdown' },
      });
      if (!response.ok) throw new Error('load');
      markdown = await response.text();
      await navigator.clipboard.writeText(markdown);
      fallback.hidden = true;
      status.textContent = 'PRD Markdown copied to the clipboard.';
    } catch {
      if (markdown) {
        area.value = markdown;
        fallback.hidden = false;
        area.focus();
        area.select();
        status.textContent =
          'Clipboard unavailable. The Markdown is selected below. Press Ctrl+C or Command+C to copy it.';
      } else {
        status.textContent = 'Could not load the PRD. Use Download PRD instead.';
      }
    }
  });
}
