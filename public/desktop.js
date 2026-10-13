// Agentic Desktop Layer page. The tray demo is a pure simulation: it never fetches,
// never probes local services and never asks for permissions. The only network call on this
// page is the same-origin fetch of the PRD Markdown for "Copy Markdown".
const demo = document.querySelector('[data-desktop-demo]');
if (demo) {
  const $ = (selector) => demo.querySelector(selector);
  const tray = $('#demo-tray'),
    menu = $('#demo-menu'),
    chip = $('[data-demo-chip]'),
    icon = $('[data-demo-icon]'),
    stateText = $('[data-demo-state]'),
    detail = $('[data-demo-detail]'),
    toggle = $('[data-demo-toggle]'),
    log = $('[data-demo-log]');
  // Icons pair with text so state never depends on colour alone.
  const icons = { Disabled: '○', Enabling: '◔', Enabled: '●', Stopping: '■' };
  const items = {
    agents:
      'Agents and permissions: claude-code (demo) has pointer and keyboard access to the text editor only. Restricted mode.',
    tasks: 'Current tasks: none running in this preview.',
    activity:
      'Activity and approvals: no pending approvals. The real control lists metadata only, never typed text.',
    apps: 'Applications and accounts: the editor is granted. Mail and calendar are not connected.',
    settings:
      'Settings: emergency shortcut, audit retention (7 days) and autostart. Autostart never enables access.',
  };
  let state = 'Disabled',
    timer;
  function render(message) {
    demo.dataset.state = state.toLowerCase();
    chip.textContent = state;
    icon.textContent = icons[state];
    stateText.textContent = state;
    const enabled = state === 'Enabled';
    toggle.textContent = enabled ? 'Disable agent access' : 'Enable agent access';
    // aria-disabled, not disabled: a disabled button would drop keyboard focus mid-transition.
    toggle.setAttribute('aria-disabled', String(state === 'Enabling' || state === 'Stopping'));
    detail.hidden = !enabled;
    detail.textContent = enabled
      ? 'Agent: claude-code (demo) · Task: Tidy notes.txt · Access: Restricted, text editor only'
      : '';
    if (message) log.textContent = `Preview state: ${state}. ${message}`;
  }
  function go(next, message, then, delay) {
    clearTimeout(timer);
    state = next;
    render(message);
    if (then) timer = setTimeout(then, delay);
  }
  function stopTo(finalState, message) {
    go(
      'Stopping',
      'Rejecting new work, revoking the input lease, releasing held keys.',
      () => go(finalState, message),
      700,
    );
  }
  function openMenu() {
    menu.hidden = false;
    tray.setAttribute('aria-expanded', 'true');
    toggle.focus();
  }
  function closeMenu(returnFocus = true) {
    menu.hidden = true;
    tray.setAttribute('aria-expanded', 'false');
    if (returnFocus) tray.focus();
  }
  tray.addEventListener('click', () => (menu.hidden ? openMenu() : closeMenu()));
  toggle.addEventListener('click', () => {
    if (state === 'Enabled') stopTo('Disabled', 'Access is off. Enabling again needs you.');
    else if (state === 'Disabled')
      go(
        'Enabling',
        'Checking human authorization and backend readiness.',
        () => go('Enabled', 'claude-code (demo) may now work on the granted editor.'),
        900,
      );
  });
  for (const button of demo.querySelectorAll('[data-demo-item]'))
    button.addEventListener('click', () => {
      log.textContent = `Preview: ${items[button.dataset.demoItem]}`;
    });
  $('[data-demo-stop]').addEventListener('click', () => {
    if (state === 'Enabled')
      stopTo(
        'Enabled',
        'All agent tasks were cancelled. Access stays on, and an agent must ask for a new lease.',
      );
    else log.textContent = `Preview state: ${state}. No agent activity to stop.`;
  });
  $('[data-demo-quit]').addEventListener('click', () => {
    closeMenu();
    if (state === 'Disabled')
      log.textContent = 'Preview state: Disabled. The control quit. Access was already off.';
    else stopTo('Disabled', 'The control quit, so access was disabled with it.');
  });
  $('[data-demo-emergency]').addEventListener('click', () => {
    if (state === 'Disabled') {
      log.textContent =
        'Preview state: Disabled. Emergency stop works at any time. Nothing was running.';
      return;
    }
    stopTo(
      'Disabled',
      'Human takeover. Access stays off until you enable it again. Nothing resumes on its own.',
    );
  });
  menu.addEventListener('keydown', (event) => {
    const buttons = [...menu.querySelectorAll('button')];
    const index = buttons.indexOf(document.activeElement);
    if (event.key === 'Escape') {
      event.preventDefault();
      closeMenu();
    } else if (event.key === 'ArrowDown' || event.key === 'ArrowUp') {
      event.preventDefault();
      const step = event.key === 'ArrowDown' ? 1 : -1;
      buttons[(index + step + buttons.length) % buttons.length]?.focus();
    } else if (event.key === 'Home' || event.key === 'End') {
      event.preventDefault();
      buttons[event.key === 'Home' ? 0 : buttons.length - 1]?.focus();
    }
  });
  tray.addEventListener('keydown', (event) => {
    if (event.key === 'Escape' && !menu.hidden) closeMenu();
    if (event.key === 'ArrowDown' && menu.hidden) {
      event.preventDefault();
      openMenu();
    }
  });
  document.addEventListener('click', (event) => {
    if (!menu.hidden && !menu.contains(event.target) && !tray.contains(event.target))
      closeMenu(false);
  });
  render();
}

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
