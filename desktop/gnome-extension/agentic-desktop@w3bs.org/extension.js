// Agentic Desktop Layer: GNOME Shell integration (GNOME Shell 46-50).
//
// Two parts, both thin. All authorization, leases, approvals and input stay in the local broker
// (`w3bs desktop daemon`); this extension never injects input.
//
// 1. Native control (PRD section 4). A top-bar indicator that shows the layer's state as an icon
//    AND a text label, with a menu: Open control panel… (the control app at
//    http://127.0.0.1:7801/control, signed in with a one-time link this control mints), status,
//    Enable/Disable agent access, pending approvals with Approve/Deny, an Agents submenu (mode,
//    Revoke), Recent activity (read-only audit metadata), Stop all agent activity, Quit and disable. It holds a native-control
//    subscription on $XDG_RUNTIME_DIR/w3bs-desktop/control.sock (newline-delimited JSON). When
//    the extension is disabled or unloaded (logout, lock screen, `gnome-extensions disable`,
//    a shell crash) that connection closes, the broker sees its native control go away, and
//    agent access is disabled. Actions that reduce access (disable, stop, approve/deny) go over
//    short-lived connections so they are never queued behind a slow enable (a portal dialog).
//
// 2. Read-only window data for the broker, exported on the session bus as
//    org.w3bs.Desktop.Shell at /org/w3bs/Desktop/Shell (interface org.w3bs.Desktop.Shell):
//      GetWindows() -> s    JSON snapshot: every window actor topmost first, focus, interlock,
//                           lock state, the keyboard layout group, and the live Mutter
//                           remote-access sessions (so the broker notices when its RemoteDesktop
//                           session is stopped behind the portal's back)
//      GetFocused() -> s    JSON of the focused window, or "null"
//      GetInterlock() -> s  why agent input must wait right now ("" when clear)
//      Activate(s id) -> b  raise and focus a window by id
//    Window ids are Meta.Window.get_id() as a decimal string: unique for the life of the shell
//    process, never reused for another window. Callers are restricted: a method call is
//    answered only when the caller's D-Bus connection belongs to the same uid as this shell
//    AND to the process at the other end of our control.sock connection (the broker, identified
//    by SO_PEERCRED on that socket). With no broker connected, every call is refused. There is
//    no shared secret in the extension.
//
// Written against GNOME Shell's public extension and JS APIs as documented at gjs.guide; no code
// from GNOME Shell or other extensions was copied.

import GLib from 'gi://GLib';
import Gio from 'gi://Gio';
import GObject from 'gi://GObject';
import St from 'gi://St';
import Clutter from 'gi://Clutter';
import Meta from 'gi://Meta';
import Shell from 'gi://Shell';

import {Extension} from 'resource:///org/gnome/shell/extensions/extension.js';
import * as Main from 'resource:///org/gnome/shell/ui/main.js';
import * as MessageTray from 'resource:///org/gnome/shell/ui/messageTray.js';
import * as KeyboardManager from 'resource:///org/gnome/shell/misc/keyboardManager.js';
import * as PanelMenu from 'resource:///org/gnome/shell/ui/panelMenu.js';
import * as PopupMenu from 'resource:///org/gnome/shell/ui/popupMenu.js';

const SCHEMA_VERSION = 1;
const SNAPSHOT_VERSION = 1;
const BUS_NAME = 'org.w3bs.Desktop.Shell';
const OBJECT_PATH = '/org/w3bs/Desktop/Shell';
const STATUS_KEY = 'agentic-desktop';
const MAX_BACKOFF_S = 5;

const IFACE_XML = `<node>
  <interface name="org.w3bs.Desktop.Shell">
    <method name="GetWindows"><arg type="s" direction="out" name="json"/></method>
    <method name="GetFocused"><arg type="s" direction="out" name="json"/></method>
    <method name="GetInterlock"><arg type="s" direction="out" name="reason"/></method>
    <method name="Activate">
      <arg type="s" direction="in" name="id"/>
      <arg type="b" direction="out" name="activated"/>
    </method>
  </interface>
</node>`;

function myUid() {
    return new Gio.Credentials().get_unix_user();
}

function socketPath() {
    const override = GLib.getenv('W3BS_DESKTOP_RUNTIME_DIR');
    if (override)
        return GLib.build_filenamev([override, 'control.sock']);
    return GLib.build_filenamev([GLib.get_user_runtime_dir(), 'w3bs-desktop', 'control.sock']);
}

// ------------------------------------------------------------------ window data

const TYPE_NAMES = new Map();
for (const [k, v] of Object.entries(Meta.WindowType)) {
    if (typeof v === 'number')
        TYPE_NAMES.set(v, k.toLowerCase());
}

function appIdOf(win) {
    // The app the shell itself associates with the window (desktop file id), else what the
    // client calls itself (GTK application id, then the Wayland app id / X11 WM_CLASS).
    const app = Shell.WindowTracker.get_default().get_window_app(win);
    const id = app ? app.get_id() : null;
    if (id && !id.startsWith('window:'))
        return id.replace(/\.desktop$/, '');
    return win.get_gtk_application_id() || win.get_sandboxed_app_id?.() || win.get_wm_class() || '';
}

function interlock() {
    if (Main.sessionMode.isLocked || Main.screenShield?.locked)
        return 'the session is locked';
    if (Main.sessionMode.isGreeter)
        return 'the login screen is showing';
    if (Main.overview.visible)
        return 'the Activities overview is open';
    if (Main.modalCount > 0)
        return 'a GNOME Shell system dialog or menu has the screen (an authentication prompt, the end-session dialog or an open shell menu); the human must close it';
    return '';
}

// The layout group Mutter's keyboard is in, as the shell's keyboard manager applied it. The
// broker checks typed characters against this group of the keymap (Mutter types a keysym only if
// the current group has it).
function keyboardGroup() {
    try {
        const g = KeyboardManager.getKeyboardManager().currentLayout?.groupIndex;
        return Number.isInteger(g) ? g : null;
    } catch (_e) {
        return null;
    }
}

// Mutter's remote-access sessions (RemoteDesktop and screen casts), each with an id unique for
// the life of this extension instance. xdg-desktop-portal-gnome does not tell portal clients
// when their session is stopped through the backend (an impl Session.Close), but Mutter reports
// the stop here, so the broker compares the ids it saw appear at enable time with this list.
class RemoteAccessWatch {
    constructor() {
        this._epoch = GLib.uuid_string_random().slice(0, 8);
        this._next = 1;
        this._live = new Map();
    }

    start() {
        const controller = global.backend.get_remote_access_controller?.();
        if (!controller)
            return;
        this._controller = controller;
        this._newId = controller.connect('new-handle', (_c, handle) => {
            const id = `${this._epoch}:${this._next++}`;
            const stoppedId = handle.connect('stopped', () => {
                this._live.delete(id);
                handle.disconnect(stoppedId);
            });
            this._live.set(id, {handle, stoppedId});
        });
    }

    stop() {
        if (this._controller && this._newId)
            this._controller.disconnect(this._newId);
        this._controller = null;
        for (const {handle, stoppedId} of this._live.values())
            handle.disconnect(stoppedId);
        this._live.clear();
    }

    // null when this Mutter has no remote-access controller (nothing to compare against)
    ids() {
        return this._controller ? [...this._live.keys()] : null;
    }
}

let remoteAccess = null;

function describe(actor, focus, workspace) {
    const win = actor.get_meta_window();
    if (!win)
        return null;
    const r = win.get_frame_rect();
    return {
        id: String(win.get_id()),
        app_id: appIdOf(win),
        wm_class: win.get_wm_class() ?? '',
        title: win.get_title() ?? '',
        x: r.x, y: r.y, width: r.width, height: r.height,
        minimized: win.minimized,
        hidden: !actor.visible,
        on_current_workspace: win.is_on_all_workspaces() || win.located_on_workspace(workspace),
        monitor: win.get_monitor(),
        type: TYPE_NAMES.get(win.get_window_type()) ?? String(win.get_window_type()),
        focused: win === focus,
    };
}

function snapshot() {
    const focus = global.display.focus_window;
    const workspace = global.workspace_manager.get_active_workspace();
    // get_window_actors() is in stacking order, bottom-most first.
    const actors = global.get_window_actors();
    const windows = [];
    for (let i = actors.length - 1; i >= 0; i--) {
        const w = describe(actors[i], focus, workspace);
        if (w)
            windows.push(w);
    }
    // The top bar is shell chrome above every window (it hides itself over fullscreen windows).
    const bar = Main.layoutManager.panelBox;
    if (bar && bar.visible && bar.mapped) {
        const [x, y] = bar.get_transformed_position();
        const [width, height] = bar.get_transformed_size();
        windows.unshift({
            id: 'shell:top-bar', app_id: 'org.gnome.Shell', wm_class: '', title: 'Top bar',
            x: Math.round(x), y: Math.round(y), width: Math.round(width), height: Math.round(height),
            minimized: false, hidden: false, on_current_workspace: true, monitor: Main.layoutManager.primaryIndex,
            type: 'shell', focused: false,
        });
    }
    return {
        version: SNAPSHOT_VERSION,
        source: 'get_window_actors',
        focused: focus ? String(focus.get_id()) : null,
        interlock: interlock(),
        locked: !!(Main.sessionMode.isLocked || Main.screenShield?.locked),
        keyboard_group: keyboardGroup(),
        remote_access: remoteAccess?.ids() ?? null,
        windows,
    };
}

function findWindow(id) {
    for (const a of global.get_window_actors()) {
        const w = a.get_meta_window();
        if (w && String(w.get_id()) === id)
            return w;
    }
    return null;
}

class WindowService {
    constructor(brokerPid) {
        this._brokerPid = brokerPid;
        this._uid = myUid();
        this._creds = new Map();
    }

    export() {
        this._impl = Gio.DBusExportedObject.wrapJSObject(IFACE_XML, this);
        this._impl.export(Gio.DBus.session, OBJECT_PATH);
        this._nameId = Gio.bus_own_name_on_connection(Gio.DBus.session, BUS_NAME, Gio.BusNameOwnerFlags.NONE, null, null);
    }

    unexport() {
        if (this._nameId)
            Gio.bus_unown_name(this._nameId);
        this._nameId = 0;
        this._impl?.unexport();
        this._impl = null;
        this._creds.clear();
    }

    // Who is calling: uid and pid of the sender's bus connection, asked of the bus daemon.
    _credentials(sender) {
        const known = this._creds.get(sender);
        if (known)
            return Promise.resolve(known);
        return new Promise((resolve, reject) => {
            Gio.DBus.session.call('org.freedesktop.DBus', '/org/freedesktop/DBus', 'org.freedesktop.DBus',
                'GetConnectionCredentials', new GLib.Variant('(s)', [sender]), new GLib.VariantType('(a{sv})'),
                Gio.DBusCallFlags.NONE, 2000, null, (conn, res) => {
                    try {
                        const [d] = conn.call_finish(res).recursiveUnpack();
                        const c = {uid: d.UnixUserID, pid: d.ProcessID};
                        // Unique names are never reused on a bus, so caching by sender is safe.
                        if (this._creds.size > 64)
                            this._creds.clear();
                        this._creds.set(sender, c);
                        resolve(c);
                    } catch (e) {
                        reject(e);
                    }
                });
        });
    }

    async _authorize(invocation) {
        const broker = this._brokerPid();
        if (!broker)
            throw new Error('no W3BS desktop broker is connected to the GNOME Shell extension');
        const c = await this._credentials(invocation.get_sender());
        if (c.uid !== this._uid)
            throw new Error('caller is not the session user');
        if (c.pid !== broker)
            throw new Error('only the connected W3BS desktop broker may query window data');
    }

    _answer(invocation, build) {
        this._authorize(invocation).then(() => {
            let v;
            try {
                v = build();
            } catch (e) {
                invocation.return_dbus_error('org.w3bs.Desktop.Shell.Error.Failed', String(e?.message ?? e));
                return;
            }
            invocation.return_value(v);
        }).catch(e => {
            invocation.return_dbus_error('org.freedesktop.DBus.Error.AccessDenied', String(e?.message ?? e));
        });
    }

    GetWindowsAsync(_params, invocation) {
        this._answer(invocation, () => new GLib.Variant('(s)', [JSON.stringify(snapshot())]));
    }

    GetFocusedAsync(_params, invocation) {
        this._answer(invocation, () => {
            const s = snapshot();
            return new GLib.Variant('(s)', [JSON.stringify(s.windows.find(w => w.id === s.focused) ?? null)]);
        });
    }

    GetInterlockAsync(_params, invocation) {
        this._answer(invocation, () => new GLib.Variant('(s)', [interlock()]));
    }

    ActivateAsync([id], invocation) {
        this._answer(invocation, () => {
            const w = findWindow(String(id));
            if (!w)
                return new GLib.Variant('(b)', [false]);
            Main.activateWindow(w, global.get_current_time());
            return new GLib.Variant('(b)', [true]);
        });
    }
}

// ------------------------------------------------------------------ control socket

const encoder = new TextEncoder();

function connectControl(path, cancellable) {
    return new Promise((resolve, reject) => {
        new Gio.SocketClient().connect_async(Gio.UnixSocketAddress.new(path), cancellable, (client, res) => {
            let conn;
            try {
                conn = client.connect_finish(res);
            } catch (e) {
                reject(e);
                return;
            }
            // The broker must run as this user (SO_PEERCRED of the socket's other end).
            let cred = null;
            try {
                cred = conn.get_socket().get_credentials();
            } catch (_e) {}
            if (!cred || cred.get_unix_user() !== myUid()) {
                conn.close(null);
                reject(new Error('control socket is not served by this user'));
                return;
            }
            resolve([conn, cred.get_unix_pid()]);
        });
    });
}

// Write every byte (GLib.Bytes keeps the buffer alive for the async call).
function writeAll(out, bytes, cancellable) {
    return new Promise((resolve, reject) => {
        const step = buf => out.write_bytes_async(new GLib.Bytes(buf), GLib.PRIORITY_DEFAULT, cancellable, (s, res) => {
            let n;
            try {
                n = s.write_bytes_finish(res);
            } catch (e) {
                reject(e);
                return;
            }
            if (n < buf.length)
                step(buf.subarray(n));
            else
                resolve();
        });
        step(bytes);
    });
}

function request(id, method, params) {
    return encoder.encode(`${JSON.stringify({v: SCHEMA_VERSION, id, method, params})}\n`);
}

function errorOf(v) {
    const e = new Error(v.error?.message ?? 'request failed');
    e.code = v.error?.code;
    return e;
}

// One request on its own connection; resolves with the result.
async function oneShot(method, params) {
    const cancellable = new Gio.Cancellable();
    let timer = GLib.timeout_add_seconds(GLib.PRIORITY_DEFAULT, 10, () => {
        timer = 0;
        cancellable.cancel();
        return GLib.SOURCE_REMOVE;
    });
    let conn = null;
    try {
        [conn] = await connectControl(socketPath(), cancellable);
        await writeAll(conn.get_output_stream(), request('1', method, params), cancellable);
        const input = new Gio.DataInputStream({base_stream: conn.get_input_stream(), close_base_stream: false});
        const line = await new Promise((resolve, reject) => {
            input.read_line_async(GLib.PRIORITY_DEFAULT, cancellable, (s, res) => {
                try {
                    resolve(s.read_line_finish_utf8(res)[0]);
                } catch (e) {
                    reject(e);
                }
            });
        });
        if (line === null)
            throw new Error('the broker closed the connection');
        const v = JSON.parse(line);
        if (v.error)
            throw errorOf(v);
        return v.result;
    } finally {
        if (timer)
            GLib.source_remove(timer);
        conn?.close(null);
    }
}

// The persistent native-control subscription, reconnecting with backoff.
class ControlLink {
    constructor(handlers) {
        this._handlers = handlers;
        this._conn = null;
        this._pid = null;
        this._seq = 0;
        this._pending = new Map();
        this._queue = [];
        this._writing = false;
        this._backoff = 1;
        this._timer = 0;
        this._stopped = false;
    }

    get brokerPid() {
        return this._conn ? this._pid : null;
    }

    get connected() {
        return this._conn !== null;
    }

    start() {
        this._cancellable = new Gio.Cancellable();
        this._connect();
    }

    stop() {
        this._stopped = true;
        if (this._timer)
            GLib.source_remove(this._timer);
        this._timer = 0;
        this._cancellable?.cancel();
        this._drop(this._conn, false);
    }

    call(method, params = {}) {
        if (!this._conn)
            return Promise.reject(new Error('broker not running'));
        const id = String(++this._seq);
        return new Promise((resolve, reject) => {
            this._pending.set(id, {resolve, reject});
            this._queue.push(request(id, method, params));
            if (!this._writing)
                this._flush(this._conn);
        });
    }

    async _connect() {
        try {
            const [conn, pid] = await connectControl(socketPath(), this._cancellable);
            if (this._stopped) {
                conn.close(null);
                return;
            }
            this._conn = conn;
            this._pid = pid;
            this._out = conn.get_output_stream();
            this._in = new Gio.DataInputStream({base_stream: conn.get_input_stream(), close_base_stream: false});
            this._backoff = 1;
            this._read(conn);
            await this.call('subscribe', {native_control: true});
            this._handlers.onUp();
        } catch (e) {
            if (this._conn)
                this._drop(this._conn, true);
            else
                this._retry();
            if (!(e instanceof GLib.Error))
                console.warn(`agentic-desktop: control connection: ${e.message}`);
        }
    }

    _retry() {
        if (this._stopped || this._timer)
            return;
        this._timer = GLib.timeout_add_seconds(GLib.PRIORITY_DEFAULT, this._backoff, () => {
            this._timer = 0;
            this._connect();
            return GLib.SOURCE_REMOVE;
        });
        this._backoff = Math.min(this._backoff * 2, MAX_BACKOFF_S);
    }

    _read(conn) {
        this._in.read_line_async(GLib.PRIORITY_DEFAULT, this._cancellable, (s, res) => {
            let line;
            try {
                [line] = s.read_line_finish_utf8(res);
            } catch (_e) {
                line = null;
            }
            if (this._conn !== conn)
                return;
            if (line === null) {
                this._drop(conn, true);
                return;
            }
            this._line(line);
            if (this._conn === conn)
                this._read(conn);
        });
    }

    _line(line) {
        let v;
        try {
            v = JSON.parse(line);
        } catch (_e) {
            return;
        }
        if (typeof v.event === 'string') {
            this._handlers.onEvent(v.event, v.data);
            return;
        }
        const p = this._pending.get(v.id);
        if (!p)
            return;
        this._pending.delete(v.id);
        if (v.error)
            p.reject(errorOf(v));
        else
            p.resolve(v.result);
    }

    _flush(conn) {
        if (this._conn !== conn || this._queue.length === 0) {
            this._writing = false;
            return;
        }
        this._writing = true;
        writeAll(this._out, this._queue.shift(), this._cancellable).then(() => this._flush(conn), () => {
            this._writing = false;
            this._drop(conn, true);
        });
    }

    _drop(conn, retry) {
        if (!conn || this._conn !== conn)
            return;
        this._conn = null;
        this._pid = null;
        this._queue = [];
        this._writing = false;
        try {
            conn.close(null);
        } catch (_e) {}
        for (const p of this._pending.values())
            p.reject(new Error('broker connection closed'));
        this._pending.clear();
        this._handlers.onDown();
        if (retry)
            this._retry();
    }
}

// ------------------------------------------------------------------ top-bar indicator

// The preset an agent's policy matches, as `w3bs desktop agents add --preset` names them.
function modeLabel(p) {
    const caps = p?.capabilities ?? [];
    if (!caps.length)
        return 'no access';
    if (p.mode === 'restricted')
        return `restricted: ${(p.apps ?? []).join(', ') || 'no apps'}`;
    return caps.includes('pointer') || caps.includes('keyboard') ? 'broad desktop' : 'observe';
}

const STATE_TEXT = {
    enabled: 'Enabled',
    disabled: 'Disabled',
    enabling: 'Enabling',
    stopping: 'Stopping',
    blocked: 'Blocked',
};

const Indicator = GObject.registerClass(
class AdlIndicator extends PanelMenu.Button {
    _init(ext) {
        super._init(0.5, 'Agentic Desktop Layer', false);
        this._ext = ext;
        this._view = {up: false, state: '', reason: null, lease: null, approvals: new Map(), drives: new Map(), alwaysAllow: false, code: null, note: null};
        this._driveNotes = new Map();
        this._names = new Map();

        const box = new St.BoxLayout({style_class: 'panel-status-menu-box'});
        this._icon = new St.Icon({style_class: 'system-status-icon'});
        this._label = new St.Label({style_class: 'adl-indicator-label', y_align: Clutter.ActorAlign.CENTER});
        box.add_child(this._icon);
        box.add_child(this._label);
        this.add_child(box);

        const info = (cls = null) => {
            const it = new PopupMenu.PopupMenuItem('', {reactive: false, can_focus: false});
            if (cls)
                it.label.add_style_class_name(cls);
            this.menu.addMenuItem(it);
            return it;
        };
        this._openItem = this._action('Open control panel…', () => this._openPanel());
        this.menu.addMenuItem(new PopupMenu.PopupSeparatorMenuItem());
        this._statusItem = info('adl-menu-status');
        this._leaseItem = info();
        this._noteItem = info('adl-menu-note');
        this.menu.addMenuItem(new PopupMenu.PopupSeparatorMenuItem());
        this._toggleItem = this._action('', () => this._toggle());
        this._confirmItem = this._action('', () => this._confirmPending());
        // Agents asking to drive: Allow, Deny, Postpone, Always allow.
        this._driveSection = new PopupMenu.PopupMenuSection();
        this.menu.addMenuItem(this._driveSection);
        this._alwaysItem = new PopupMenu.PopupSwitchMenuItem("Always allow agents to drive (I'm away)", false);
        this._alwaysItem.connect('toggled', (_it, on) => this._run('drive.always_allow', {on}));
        this.menu.addMenuItem(this._alwaysItem);
        this._approvalSection = new PopupMenu.PopupMenuSection();
        this.menu.addMenuItem(this._approvalSection);
        this._approvalItems = new Map();
        this.menu.addMenuItem(new PopupMenu.PopupSeparatorMenuItem());
        this._agentsMenu = new PopupMenu.PopupSubMenuMenuItem('Agents');
        this.menu.addMenuItem(this._agentsMenu);
        this._activityMenu = new PopupMenu.PopupSubMenuMenuItem('Recent activity');
        this.menu.addMenuItem(this._activityMenu);
        this.menu.connect('open-state-changed', (_m, open) => {
            if (open)
                this._refreshLists();
        });
        this.menu.addMenuItem(new PopupMenu.PopupSeparatorMenuItem());
        this._stopItem = this._action('Stop all agent activity', () => this._run('stop', {reason: 'stopped from the GNOME top bar'}));
        this._quitItem = this._action('Quit and disable', () => this._run('disable', {reason: 'quit from the GNOME top bar'}));
        this._render();
    }

    _action(text, fn, section = this.menu) {
        const it = new PopupMenu.PopupMenuItem(text);
        it.connect('activate', () => fn());
        section.addMenuItem(it);
        return it;
    }

    // ---- events from the broker

    up() {
        this._view.up = true;
        this._view.note = null;
        this._render();
        // State the broker had before this connection: always-allow and requests still waiting.
        oneShot('status', {}).then(st => {
            this.event('drive_always_allow', {on: !!st?.drive_always_allow});
        }).catch(() => {});
        oneShot('drive.list', {}).then(list => {
            for (const d of list ?? []) {
                if (d.status === 'pending')
                    this.event('drive_requested', d);
            }
        }).catch(() => {});
    }

    down() {
        Object.assign(this._view, {up: false, state: '', reason: null, lease: null, code: null});
        this._view.approvals.clear();
        for (const id of [...this._view.drives.keys()])
            this._dropDrive(id);
        this._render();
    }

    event(name, data) {
        const v = this._view;
        switch (name) {
        case 'state':
            v.state = data?.state ?? '';
            v.reason = data?.reason ?? null;
            if (v.state !== 'enabling')
                v.code = null;
            if (v.state !== 'enabled') {
                v.lease = null;
                v.approvals.clear();
                for (const id of [...v.drives.keys()])
                    this._dropDrive(id);
            }
            break;
        case 'enable_requested':
            v.code = data?.code ?? null;
            break;
        case 'lease':
            v.lease = data ? {agent: data.agent, task: data.task} : null;
            if (data)
                this._learnName(data.agent);
            break;
        case 'approval_requested':
            if (data?.id) {
                v.approvals.set(data.id, {agent: data.agent, summary: data.summary ?? ''});
                this._learnName(data.agent);
            }
            break;
        case 'approval_decided':
            v.approvals.delete(data?.id);
            break;
        case 'drive_requested':
            if (data?.id) {
                v.drives.set(data.id, {name: data.agent_name ?? '', description: data.description ?? ''});
                this._notifyDrive(data.id, data.agent_name ?? '', data.description ?? '');
            }
            break;
        case 'drive_decided':
            this._dropDrive(data?.id);
            break;
        case 'drive_always_allow':
            v.alwaysAllow = !!data?.on;
            break;
        default:
            return;
        }
        this._render();
    }

    // Events carry agent ids; show the registered name.
    _learnName(id) {
        if (!id || this._names.has(id))
            return;
        this._names.set(id, id);
        oneShot('agents.list', {}).then(list => {
            for (const a of list ?? [])
                this._names.set(a.id, a.name);
            this._render();
        }).catch(() => {});
    }

    _name(id) {
        return this._names.get(id) ?? id ?? '';
    }

    // ---- actions

    _toggle() {
        if (this._view.state === 'enabled')
            this._run('disable', {reason: 'disabled from the GNOME top bar'});
        else
            this._enable();
    }

    async _enable() {
        const link = this._ext.link;
        try {
            // The code comes back only because this connection is the native control.
            const ch = await link.call('enable.request', {});
            if (!ch?.code)
                throw new Error('the broker did not hand the code to this control');
            await link.call('enable.confirm', {code: ch.code});
            this._note(null);
        } catch (e) {
            this._note(`Last action failed: ${e.message}`);
        }
    }

    async _confirmPending() {
        const code = this._view.code;
        if (!code)
            return;
        try {
            await this._ext.link.call('enable.confirm', {code});
            this._note(null);
        } catch (e) {
            this._note(`Last action failed: ${e.message}`);
        }
    }

    async _run(method, params) {
        try {
            await oneShot(method, params);
            this._note(null);
        } catch (e) {
            this._note(`Last action failed: ${e.message}`);
        }
    }

    // The control app, signed in with a one-time link minted on the native-control connection.
    async _openPanel() {
        try {
            const r = await this._ext.link.call('control.login', {via: 'GNOME panel'});
            Gio.AppInfo.launch_default_for_uri(r.url, null);
            this._note(null);
        } catch (e) {
            this._note(`Could not open the control panel: ${e.message}`);
        }
    }

    // Agents and recent activity have no broker events; read them each time the menu opens.
    async _refreshLists() {
        if (!this._view.up)
            return;
        let agents, recent;
        try {
            [agents, recent] = await Promise.all([oneShot('agents.list', {}), oneShot('audit.recent', {limit: 5})]);
        } catch (_e) {
            return;
        }
        for (const a of agents ?? [])
            this._names.set(a.id, a.name);
        const am = this._agentsMenu.menu;
        am.removeAll();
        const active = (agents ?? []).filter(a => !a.revoked);
        if (!active.length)
            am.addMenuItem(new PopupMenu.PopupMenuItem('No agents registered', {reactive: false, can_focus: false}));
        for (const a of active) {
            am.addMenuItem(new PopupMenu.PopupMenuItem(`${a.name} (${modeLabel(a.policy)})`, {reactive: false, can_focus: false}));
            this._action(`    Revoke ${a.name}`, () => this._run('agents.revoke', {agent: a.id}), am);
        }
        const rm = this._activityMenu.menu;
        rm.removeAll();
        const rows = [...(recent ?? [])].reverse();
        if (!rows.length)
            rm.addMenuItem(new PopupMenu.PopupMenuItem('No activity yet', {reactive: false, can_focus: false}));
        for (const r of rows) {
            const t = GLib.DateTime.new_from_unix_local(Math.floor((r.ts_ms ?? 0) / 1000)).format('%H:%M:%S');
            let line = `${t}  ${r.op}`;
            if (r.agent)
                line += ` · ${this._name(r.agent)}`;
            line += ` · ${r.decision}`;
            if (r.meta?.reason)
                line += ` (${r.meta.reason})`;
            rm.addMenuItem(new PopupMenu.PopupMenuItem(line, {reactive: false, can_focus: false}));
        }
    }

    _decide(id, approve) {
        this._run('approvals.decide', {approval: id, approve});
    }

    _drive(id, decision) {
        this._run('drive.decide', {drive: id, decision});
    }

    _dropDrive(id) {
        this._view.drives.delete(id);
        const n = this._driveNotes.get(id);
        this._driveNotes.delete(id);
        try {
            n?.destroy();
        } catch (_e) {}
    }

    // A pop-up with the four answers, like the tray's notification on KDE. The menu has the same
    // choices if the notification cannot be shown. Agent text is set as plain text.
    _notifyDrive(id, name, description) {
        if (this._driveNotes.has(id))
            return;
        try {
            const source = MessageTray.getSystemSource();
            const n = new MessageTray.Notification({
                source,
                title: `${name} wants to drive your desktop`,
                body: description ? `Task: ${description}` : 'It will move the pointer and type until its task ends.',
                urgency: MessageTray.Urgency.CRITICAL,
                resident: true,
            });
            n.addAction('Allow', () => this._drive(id, 'allow'));
            n.addAction('Deny', () => this._drive(id, 'deny'));
            n.addAction('Postpone 10 min', () => this._drive(id, 'postpone'));
            n.addAction('Always allow', () => this._drive(id, 'always_allow'));
            source.addNotification(n);
            this._driveNotes.set(id, n);
        } catch (e) {
            console.warn(`Agentic Desktop Layer: drive prompt notification failed: ${e.message}`);
        }
    }

    _note(text) {
        this._view.note = text;
        this._render();
    }

    // ---- view

    stateText() {
        const v = this._view;
        if (!v.up)
            return 'Broker not running';
        const s = STATE_TEXT[v.state] ?? (v.state || 'Connecting');
        return v.reason ? `${s} (${v.reason})` : s;
    }

    _render() {
        const v = this._view;
        const enabled = v.up && v.state === 'enabled';
        const attention = v.approvals.size > 0 || v.drives.size > 0 || v.code !== null || v.state === 'blocked';
        let icon = 'changes-prevent-symbolic', short = 'Off';
        if (!v.up) {
            icon = 'action-unavailable-symbolic';
            short = 'No broker';
        } else if (attention) {
            icon = 'dialog-warning-symbolic';
            short = v.drives.size > 0 ? 'Agent asking' : v.approvals.size > 0 ? `${enabled ? 'On' : 'Off'}, approval` : (v.state === 'blocked' ? 'Blocked' : 'Confirm');
        } else if (enabled) {
            icon = v.lease ? 'input-mouse-symbolic' : 'changes-allow-symbolic';
            short = v.lease ? 'On, active' : 'On';
        } else if (v.state === 'enabling' || v.state === 'stopping') {
            short = '…';
        }
        this._icon.icon_name = icon;
        this._label.text = `Agents: ${short}`;
        if (enabled)
            this.add_style_class_name('adl-indicator-enabled');
        else
            this.remove_style_class_name('adl-indicator-enabled');

        let a11y = `Agentic Desktop Layer: agent access ${this.stateText()}`;
        if (v.lease)
            a11y += `; agent ${this._name(v.lease.agent)} is controlling input`;
        if (v.approvals.size)
            a11y += `; ${v.approvals.size} approval(s) waiting`;
        if (v.drives.size)
            a11y += `; ${v.drives.size} agent(s) asking to drive`;
        this.accessible_name = a11y;

        this._statusItem.label.text = `Status: ${this.stateText()}`;
        this._leaseItem.visible = !!v.lease;
        if (v.lease)
            this._leaseItem.label.text = `Active: ${this._name(v.lease.agent)}, task ${v.lease.task}`;
        this._noteItem.visible = !!v.note;
        this._noteItem.label.text = v.note ?? '';

        this._toggleItem.visible = v.up;
        this._toggleItem.label.text = enabled ? 'Disable agent access' : 'Enable agent access';
        this._confirmItem.visible = v.up && !!v.code;
        this._confirmItem.label.text = v.code ? `Confirm the enable request from a terminal (code ${v.code})` : '';
        this._stopItem.visible = v.up;
        this._openItem.visible = v.up;
        this._agentsMenu.visible = v.up;
        this._activityMenu.visible = v.up;
        this._quitItem.visible = v.up;

        this._alwaysItem.visible = v.up;
        if (this._alwaysItem.state !== v.alwaysAllow)
            this._alwaysItem.setToggleState(v.alwaysAllow);
        const driveIds = [...v.drives.keys()].join(',');
        if (driveIds !== this._driveIds) {
            this._driveIds = driveIds;
            this._driveSection.removeAll();
            for (const [id, d] of v.drives) {
                const text = d.description ? `${d.name} wants to drive: ${d.description}` : `${d.name} wants to drive`;
                this._driveSection.addMenuItem(new PopupMenu.PopupMenuItem(text, {reactive: false, can_focus: false}));
                this._action('    Allow', () => this._drive(id, 'allow'), this._driveSection);
                this._action('    Deny', () => this._drive(id, 'deny'), this._driveSection);
                this._action('    Postpone 10 min', () => this._drive(id, 'postpone'), this._driveSection);
                this._action('    Always allow', () => this._drive(id, 'always_allow'), this._driveSection);
            }
        }

        // Rebuild the approvals only when the set changes, so keyboard focus is not lost.
        const ids = [...v.approvals.keys()].join(',');
        if (ids !== this._approvalIds) {
            this._approvalIds = ids;
            this._approvalSection.removeAll();
            this._approvalItems.clear();
            for (const [id, a] of v.approvals) {
                const label = new PopupMenu.PopupMenuItem('', {reactive: false, can_focus: false});
                this._approvalSection.addMenuItem(label);
                const approve = this._action('    Approve', () => this._decide(id, true), this._approvalSection);
                const deny = this._action('    Deny', () => this._decide(id, false), this._approvalSection);
                approve.accessible_name = `Approve: ${a.summary}`;
                deny.accessible_name = `Deny: ${a.summary}`;
                this._approvalItems.set(id, {label, approve, deny});
            }
        }
        for (const [id, a] of v.approvals) {
            const it = this._approvalItems.get(id);
            if (it)
                it.label.label.text = `Approval: ${this._name(a.agent)} wants: ${a.summary}`;
        }
    }
});

// ------------------------------------------------------------------ extension

export default class AgenticDesktopExtension extends Extension {
    enable() {
        this._indicator = new Indicator(this);
        Main.panel.addToStatusArea(STATUS_KEY, this._indicator);
        this.link = new ControlLink({
            onUp: () => this._indicator?.up(),
            onDown: () => this._indicator?.down(),
            onEvent: (name, data) => this._indicator?.event(name, data),
        });
        remoteAccess = new RemoteAccessWatch();
        remoteAccess.start();
        this._service = new WindowService(() => this.link?.brokerPid ?? null);
        this._service.export();
        this.link.start();
    }

    disable() {
        // Closing the subscription is what tells the broker its native control went away.
        this.link?.stop();
        this.link = null;
        this._service?.unexport();
        this._service = null;
        remoteAccess?.stop();
        remoteAccess = null;
        this._indicator?.destroy();
        this._indicator = null;
    }
}
