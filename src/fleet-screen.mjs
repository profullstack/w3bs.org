// Fleet screens: the owner watches a machine's desktop in noVNC on /fleet/<id>/screen. Nothing
// on the machine listens to the internet. Its agent (`w3bs fleet agent`, with `w3bs fleet
// screen on`) keeps one outbound WebSocket here (/api/fleet/listen); when the owner opens the
// page, the relay tells the agent a session id, the agent dials /api/fleet/screen/<session>
// and connects it to the machine's local VNC server, and the relay pipes the two sockets.
// RFB passes through unchanged; the agent, not the relay, keeps a view-only machine view-only.
import { randomBytes } from 'node:crypto';
import { WebSocketServer } from 'ws';
import { bearerOf } from './auth-routes.mjs';

const PAIR_TIMEOUT = 20_000;
const PING_EVERY = 25_000; // dev2's nginx drops a proxied socket after 300 s of silence
const MAX_SESSIONS = 3; // per machine
const MAX_BUFFERED = 8 * 1024 * 1024;
const OPEN = 1; // WebSocket readyState
// Close codes the page explains (4000-4999 are the application's).
export const CLOSE = {
  NOT_SHARED: 4403,
  NOT_LISTENING: 4404,
  BUSY: 4429,
  AGENT_ERROR: 4502,
  NO_ANSWER: 4504,
};

const duration = (ms) => {
  const s = Math.round(ms / 1000);
  return s < 60
    ? `${s}s`
    : s < 3600
      ? `${Math.floor(s / 60)}m ${s % 60}s`
      : `${Math.floor(s / 3600)}h ${Math.floor((s % 3600) / 60)}m`;
};
// A close reason is at most 123 bytes of UTF-8.
const reasonOf = (text) => Buffer.from(String(text)).subarray(0, 123).toString().replace(/�$/, '');

export function createScreenRelay({ fleet, origin, currentUser }) {
  const host = new URL(origin).hostname;
  const wss = new WebSocketServer({ noServer: true, maxPayload: 16 * 1024 * 1024 });
  const listeners = new Map(); // machine id -> the agent's /api/fleet/listen socket
  const sessions = new Map(); // session id -> { machine, userId, viewer, agent, queue, ... }

  const refuse = (socket, status, text) => {
    socket.end(`HTTP/1.1 ${status} ${text}\r\nConnection: close\r\nContent-Length: 0\r\n\r\n`);
  };
  const accept = (req, socket, head) =>
    new Promise((resolve) => wss.handleUpgrade(req, socket, head, resolve));
  const keepAlive = (ws) => {
    const timer = setInterval(() => ws.ping(), PING_EVERY);
    ws.on('close', () => clearInterval(timer));
  };
  const send = (ws, data) => {
    if (ws.readyState !== OPEN) return;
    if (ws.bufferedAmount > MAX_BUFFERED) return ws.close(1013, 'The connection is too slow.');
    ws.send(data, { binary: true });
  };
  const liveFor = (machineId) =>
    [...sessions.values()].filter((s) => s.machine.id === machineId).length;

  function end(session, { status = 'done', note } = {}) {
    if (session.ended) return;
    session.ended = true;
    clearTimeout(session.timer);
    sessions.delete(session.id);
    for (const ws of [session.viewer, session.agent])
      if (ws && ws.readyState <= OPEN) ws.close(1000);
    const watched = session.pairedAt
      ? `Watched for ${duration(Date.now() - session.pairedAt)}`
      : '';
    const output = [watched, note].filter(Boolean).join('. ') || 'Ended before it started.';
    fleet
      .logScreen(session.machine.id, session.userId, {
        status: session.pairedAt && status === 'done' ? 'done' : 'failed',
        output: `${output} (${session.mode}, from ${session.ip})`,
        startedAt: session.startedAt,
      })
      .catch(() => {});
  }

  // Owner's browser: /api/fleet/machines/<id>/screen, by session cookie on the main host only.
  async function viewer(req, socket, head, machineId) {
    if (req.headers.origin !== origin) return refuse(socket, 403, 'Forbidden');
    const user = await currentUser(req);
    if (!user) return refuse(socket, 401, 'Unauthorized');
    const machine = await fleet.ownedMachine(user.id, machineId).catch(() => null);
    if (!machine) return refuse(socket, 404, 'Not Found');
    const ws = await accept(req, socket, head);
    const mode = machine.info.screen?.mode;
    if (!mode)
      return ws.close(
        CLOSE.NOT_SHARED,
        'This machine does not share its screen. On it, run: w3bs fleet screen on',
      );
    if (!machine.permissions?.['desktop.view'])
      return ws.close(CLOSE.NOT_SHARED, 'Desktop viewing is disabled in Fleet permissions.');
    // The relay forwards RFB unchanged, so a control-mode desktop cannot safely be
    // downgraded to view-only here. Require both grants before opening that stream.
    if (mode === 'control' && !machine.permissions?.['desktop.control'])
      return ws.close(CLOSE.NOT_SHARED, 'Desktop control is disabled in Fleet permissions.');
    const listener = listeners.get(machine.id);
    if (!listener)
      return ws.close(
        CLOSE.NOT_LISTENING,
        'The machine’s agent is not connected. Check it with: w3bs fleet status',
      );
    if (liveFor(machine.id) >= MAX_SESSIONS)
      return ws.close(
        CLOSE.BUSY,
        `${MAX_SESSIONS} screen sessions are already open on this machine.`,
      );
    const session = {
      id: `s_${randomBytes(18).toString('base64url')}`,
      machine,
      mode,
      userId: user.id,
      ip: String(req.headers['x-real-ip'] || req.socket.remoteAddress || '?'),
      viewer: ws,
      agent: null,
      queue: [],
      queued: 0,
      startedAt: new Date().toISOString(),
    };
    sessions.set(session.id, session);
    keepAlive(ws);
    ws.on('message', (data, binary) => {
      if (!binary) return;
      if (session.agent) return send(session.agent, data);
      session.queued += data.length;
      if (session.queued > 1024 * 1024)
        return end(session, {
          status: 'failed',
          note: 'Viewer sent too much before the machine answered',
        });
      session.queue.push(data);
    });
    ws.on('close', () => end(session));
    ws.on('error', () => end(session, { status: 'failed' }));
    session.timer = setTimeout(() => {
      ws.close(
        CLOSE.NO_ANSWER,
        'The machine did not answer. Is its agent running? (w3bs fleet status)',
      );
      end(session, { status: 'failed', note: 'The machine did not answer' });
    }, PAIR_TIMEOUT);
    listener.send(JSON.stringify({ type: 'screen', session: session.id }));
  }

  // Agent: its standing connection, which only ever receives session ids.
  async function listen(req, socket, head) {
    const machine = await fleet.machineByKey(bearerOf(req));
    if (!machine) return refuse(socket, 401, 'Unauthorized');
    const ws = await accept(req, socket, head);
    listeners.get(machine.id)?.close(1000, 'Replaced by a newer connection.');
    listeners.set(machine.id, ws);
    keepAlive(ws);
    ws.on('message', () => {}); // nothing to say; pings are protocol-level
    ws.on('close', () => {
      if (listeners.get(machine.id) === ws) listeners.delete(machine.id);
    });
  }

  // Agent: one session's RFB stream, or a text frame {"error": "..."} when it cannot connect.
  async function agentSession(req, socket, head, sessionId) {
    const machine = await fleet.machineByKey(bearerOf(req));
    if (!machine) return refuse(socket, 401, 'Unauthorized');
    const session = sessions.get(sessionId);
    if (!session || session.machine.id !== machine.id || session.agent)
      return refuse(socket, 404, 'Not Found');
    const ws = await accept(req, socket, head);
    session.agent = ws;
    keepAlive(ws);
    clearTimeout(session.timer);
    ws.on('message', (data, binary) => {
      if (binary) {
        session.pairedAt ??= Date.now();
        return send(session.viewer, data);
      }
      let message = {};
      try {
        message = JSON.parse(String(data));
      } catch {}
      if (message.error) {
        session.viewer.close(CLOSE.AGENT_ERROR, reasonOf(message.error));
        end(session, { status: 'failed', note: String(message.error).slice(0, 500) });
      }
    });
    ws.on('close', () => end(session));
    ws.on('error', () => end(session, { status: 'failed' }));
    for (const data of session.queue.splice(0)) send(ws, data);
  }

  const ROUTES = [
    [/^\/api\/fleet\/listen$/, listen],
    [/^\/api\/fleet\/screen\/(s_[A-Za-z0-9_-]{24})$/, agentSession],
    [/^\/api\/fleet\/machines\/(m_[A-Za-z0-9_-]{1,40})\/screen$/, viewer],
  ];
  return {
    // Whether the machine's agent is connected and could start a session now.
    listening: (machineId) => listeners.has(machineId),
    // node:http 'upgrade' handler; false when the path is not ours.
    upgrade(req, socket, head) {
      const url = new URL(req.url, 'http://x');
      const hostname = String(req.headers.host || '')
        .replace(/:\d+$/, '')
        .toLowerCase();
      for (const [pattern, handler] of ROUTES) {
        const match = pattern.exec(url.pathname);
        if (!match) continue;
        if (hostname !== host) {
          refuse(socket, 403, 'Forbidden');
          return true;
        }
        socket.on('error', () => socket.destroy());
        handler(req, socket, head, match[1]).catch((error) => {
          process.stderr.write(
            `fleet screen: ${req.url.split('?')[0]}: ${error?.stack || error}\n`,
          );
          socket.destroy();
        });
        return true;
      }
      return false;
    },
    // A machine left or was removed: stop its listener and sessions.
    drop(machineId) {
      for (const session of sessions.values())
        if (session.machine.id === machineId) end(session, { note: 'The machine was removed' });
      listeners.get(machineId)?.close(1000, 'Removed from the fleet.');
      listeners.delete(machineId);
    },
    revokeSessions(machineId) {
      for (const session of sessions.values())
        if (session.machine.id === machineId)
          end(session, { status: 'failed', note: 'Fleet desktop permissions changed' });
    },
    close() {
      for (const session of sessions.values()) end(session, { note: 'The server restarted' });
      for (const ws of listeners.values()) ws.terminate();
      listeners.clear();
    },
  };
}
