// w3bs.org/fleet/<id>/screen: a machine's desktop in noVNC, relayed through w3bs.org (see
// src/fleet-screen.mjs). Starts view-only; machines shared with `--control` can be switched
// to control here, and the agent on the machine enforces whichever mode it was given.
import RFB from '/assets/novnc.js';

const data = JSON.parse(document.getElementById('screen-data').textContent);
const frame = document.getElementById('screen');
const state = document.getElementById('screen-state');
const message = document.getElementById('screen-message');
const modeNote = document.getElementById('screen-mode');
const control = document.getElementById('screen-control');
const fit = document.getElementById('screen-fit');
const reconnect = document.getElementById('screen-reconnect');

const setState = (text, tone = '') => {
  state.textContent = text;
  state.className = `pill ${tone}`;
};
const say = (text) => {
  message.replaceChildren(text);
};

const canControl = data.screen?.mode === 'control';
document.getElementById('screen-control-label').hidden = !canControl;
modeNote.textContent = data.screen
  ? `${canControl ? 'View and control' : 'View only'} · VNC ${data.screen.vnc || '127.0.0.1:5900'}`
  : '';

let rfb;
let scale = true;

function passwordForm() {
  const form = document.createElement('form');
  form.className = 'screen-password';
  const input = Object.assign(document.createElement('input'), {
    type: 'password',
    autocomplete: 'off',
    placeholder: 'VNC password',
    required: true,
  });
  const button = Object.assign(document.createElement('button'), {
    className: 'small-button',
    type: 'submit',
    textContent: 'Connect',
  });
  form.append('The VNC server on the machine asks for its password. ', input, ' ', button);
  form.addEventListener('submit', (event) => {
    event.preventDefault();
    rfb?.sendCredentials({ password: input.value });
    say('');
  });
  return form;
}

function connect() {
  reconnect.hidden = true;
  say('');
  if (!data.screen) {
    setState('Not shared');
    say('This machine does not share its screen. On the machine, run: w3bs fleet screen on');
    return;
  }
  setState('Connecting…');
  const scheme = location.protocol === 'https:' ? 'wss' : 'ws';
  const ws = new WebSocket(
    `${scheme}://${location.host}/api/fleet/machines/${encodeURIComponent(data.id)}/screen`,
  );
  ws.binaryType = 'arraybuffer';
  let reason = '';
  // Registered before noVNC's own handler, so the relay's explanation is known on disconnect.
  ws.addEventListener('close', (event) => {
    if (event.code >= 4000) reason = event.reason;
  });
  rfb = new RFB(frame, ws, { shared: true });
  rfb.viewOnly = !(canControl && control.checked);
  rfb.scaleViewport = scale;
  rfb.showDotCursor = true;
  rfb.background = 'transparent';
  rfb.addEventListener('connect', () => {
    setState('Live', 'green');
    rfb.focus();
  });
  rfb.addEventListener('desktopname', (event) => {
    if (event.detail.name) modeNote.textContent = `${event.detail.name} · ${modeNote.textContent}`;
  });
  rfb.addEventListener('credentialsrequired', () => {
    setState('Password');
    say(passwordForm());
    message.querySelector('input')?.focus();
  });
  rfb.addEventListener('securityfailure', (event) => {
    reason = event.detail.reason || 'The VNC server refused the password.';
  });
  rfb.addEventListener('disconnect', (event) => {
    rfb = undefined;
    setState(event.detail.clean && !reason ? 'Closed' : 'Disconnected', reason ? 'red' : '');
    say(reason || (event.detail.clean ? 'The session ended.' : 'The connection dropped.'));
    reconnect.hidden = false;
  });
}

control.addEventListener('change', () => {
  if (rfb) rfb.viewOnly = !control.checked;
});
fit.addEventListener('click', () => {
  scale = !scale;
  fit.textContent = scale ? 'Fit' : 'Actual size';
  if (rfb) rfb.scaleViewport = scale;
});
document.getElementById('screen-full').addEventListener('click', () => {
  frame.requestFullscreen?.().catch(() => {});
});
reconnect.addEventListener('click', connect);
connect();
