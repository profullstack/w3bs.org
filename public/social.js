// w3bs.org/social: asks for the signed-in person's one PDS invite code and shows it.
const form = document.querySelector('form[data-form="social-invite"]');
const shown = document.querySelector('.social-invite');
const code = document.getElementById('social-code');

form?.addEventListener('submit', async (event) => {
  event.preventDefault();
  const status = form.querySelector('.form-status');
  const button = form.querySelector('button');
  status.textContent = '';
  button.disabled = true;
  try {
    const response = await fetch('/api/social/invite', {
      method: 'POST',
      credentials: 'same-origin',
    });
    const result = await response.json().catch(() => ({}));
    if (!response.ok)
      throw new Error(result.error?.message || `Request failed (${response.status}).`);
    code.textContent = result.code;
    form.hidden = true;
    shown.hidden = false;
  } catch (error) {
    status.textContent = error.message;
    button.disabled = false;
  }
});

const copy = document.querySelector('[data-action="copy-code"]');
copy?.addEventListener('click', async () => {
  await navigator.clipboard?.writeText(code.textContent).catch(() => {});
  copy.textContent = 'Copied';
});
