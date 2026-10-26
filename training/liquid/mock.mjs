// Deterministic mock sites for the Liquid Agent: a mail client, a Reddit-like forum and a
// LinkedIn-like notifications page. No network, no real accounts, nothing leaves the process.
// They emit liquid-agent/v1 semantic observations ({app, url_pattern, focus, elements}) in the
// shapes of Infernet's synthetic generator (packages/liquid-agent/runtime/infernet_liquid/synth.py),
// including its four layout variants (0-2 seen, 3 renamed and reordered), and apply actions.
//
// External writes (send, post, publish, submit, delete, or a click on a control that does one)
// are refused unless the caller passes an approval id: the guarded executor is the real gate,
// this is a second fence so a test can prove nothing was written without a person.
import { createHash } from 'node:crypto';

export const TASKS = ['email.reply_draft', 'reddit.post_compose', 'linkedin.notifications_review'];
export const APPS = {
  'email.reply_draft': 'mail',
  'reddit.post_compose': 'browser',
  'linkedin.notifications_review': 'browser',
};
export const GOALS = {
  'email.reply_draft': 'Reply to the unread message and save the reply as a draft',
  'reddit.post_compose':
    'Compose a post in the joined subreddit, preview it, then publish (needs approval)',
  'linkedin.notifications_review':
    'Open notifications, scroll to the end of the new ones and open the newest',
};
export const RISK = {
  'email.reply_draft': 'low',
  'reddit.post_compose': 'medium',
  'linkedin.notifications_review': 'low',
};
export const PACE_MS = 3000;

const PEOPLE = ['Dana', 'Sam', 'Lee', 'Ari', 'Kim', 'Noor', 'Jo', 'Pat', 'Ravi', 'Mei', 'Uma'];
const SUBJECTS = [
  'Lunch?',
  'Invoice 1042',
  'Re: roadmap',
  'Weekend plans',
  'Quick question',
  'Design review',
  'Flight details',
  'Team offsite',
  'Contract draft',
  'Hiring update',
];
const SUBS = [
  'r/selfhosted',
  'r/rust',
  'r/homelab',
  'r/linux',
  'r/python',
  'r/webdev',
  'r/sysadmin',
];
const EVENTS = [
  'commented on your post',
  'liked your comment',
  'viewed your profile',
  'mentioned you',
  'shared your post',
  'endorsed you',
];

export const EMAIL_V = [
  {
    inbox: 'Inbox',
    unread: 'Unread',
    reply: 'Reply',
    draft: 'Save draft',
    body: 'Message body',
    compose: 'Compose',
  },
  {
    inbox: 'Mail',
    unread: 'New',
    reply: 'Reply',
    draft: 'Save as draft',
    body: 'Write your reply',
    compose: 'New message',
  },
  {
    inbox: 'All inboxes',
    unread: 'Unread',
    reply: 'Reply to sender',
    draft: 'Save draft',
    body: 'Body',
    compose: 'Compose',
  },
  {
    inbox: 'Messages',
    unread: 'Unseen',
    reply: 'Respond',
    draft: 'Keep as draft',
    body: 'Your message',
    compose: 'Write',
  },
];
export const REDDIT_V = [
  {
    create: 'Create post',
    title: 'Title',
    body: 'Body text (optional)',
    preview: 'Preview',
    post: 'Post',
    joined: 'joined',
  },
  {
    create: 'New post',
    title: 'Title',
    body: 'Text',
    preview: 'Preview',
    post: 'Publish',
    joined: 'joined',
  },
  {
    create: 'Create a post',
    title: 'Post title',
    body: 'Body',
    preview: 'Preview post',
    post: 'Post',
    joined: 'member',
  },
  {
    create: 'Start a thread',
    title: 'Headline',
    body: 'Post text',
    preview: 'See preview',
    post: 'Submit post',
    joined: 'subscribed',
  },
];
export const LINKEDIN_V = [
  { notif: 'Notifications', new: 'New', end: 'No more new notifications' },
  { notif: 'Notifications', new: 'Unread', end: "You're all caught up" },
  { notif: 'Alerts', new: 'New', end: 'No more new notifications' },
  { notif: 'Activity', new: 'Fresh', end: 'Nothing new' },
];

// Controls whose click has an effect outside the local UI. The executor treats a click on one
// of these exactly like a send/post action: a person approves it at action time.
export const EXTERNAL_CONTROL =
  /^(send|post|publish|submit|submit post|delete|send now|reply all)$/i;

// Seeded PRNG (mulberry32 over a sha256 of the seed) so every run is reproducible.
export function rng(seed) {
  let a = parseInt(createHash('sha256').update(String(seed)).digest('hex').slice(0, 8), 16) >>> 0;
  const next = () => {
    a = (a + 0x6d2b79f5) >>> 0;
    let t = a;
    t = Math.imul(t ^ (t >>> 15), t | 1);
    t ^= t + Math.imul(t ^ (t >>> 7), t | 61);
    return ((t ^ (t >>> 14)) >>> 0) / 4294967296;
  };
  return {
    random: next,
    int: (lo, hi) => lo + Math.floor(next() * (hi - lo + 1)),
    pick: (xs) => xs[Math.floor(next() * xs.length)],
    sample: (xs, n) => {
      const copy = [...xs];
      const out = [];
      while (out.length < n && copy.length)
        out.push(copy.splice(Math.floor(next() * copy.length), 1)[0]);
      return out;
    },
    shuffle: (xs) => {
      const copy = [...xs];
      for (let i = copy.length - 1; i > 0; i -= 1) {
        const j = Math.floor(next() * (i + 1));
        [copy[i], copy[j]] = [copy[j], copy[i]];
      }
      return copy;
    },
  };
}

const el = (role, name) => ({ role, name });
const clone = (x) => JSON.parse(JSON.stringify(x));
const same = (a, b) => !!a && !!b && a.role === b.role && a.name === b.name;

/**
 * createMockEnv(task, { seed, variant, account, signIn }) -> environment
 *   observe()                  current semantic observation (a copy)
 *   apply(action, {approval})  { status: 'ok' } or { status: 'error', error }
 *   tick(ms)                   advance the mock's clock (pacing rules read it)
 *   finished / success         whether `done` was reached, and in the goal state
 *   writes                     external writes performed, each with its approval id
 *   account / switchAccount()  the signed-in mock account (not part of observations)
 */
export function createMockEnv(
  task,
  { seed = 1, variant = 0, account = 'mock-a', signIn = false } = {},
) {
  if (!TASKS.includes(task)) throw new Error(`unknown mock task ${task}`);
  const r = rng(`${task}:${seed}:${variant}`);
  const env = {
    task,
    app: APPS[task],
    variant,
    account,
    clock: 0,
    finished: false,
    success: false,
    writes: [],
    history: [],
    page: null,
    state: {},
  };
  const builders = {
    'email.reply_draft': email,
    'reddit.post_compose': reddit,
    'linkedin.notifications_review': linkedin,
  };
  const machine = builders[task](r, variant, env, { signIn });
  env.observe = () => clone(machine.observation());
  env.tick = (ms) => {
    env.clock += Math.max(0, ms | 0);
  };
  env.switchAccount = (next) => {
    env.account = next;
    machine.reset();
  };
  env.apply = (action, { approval } = {}) => {
    const result = (() => {
      if (env.finished) return { status: 'error', error: 'task already finished' };
      if (!action || typeof action.type !== 'string')
        return { status: 'error', error: 'no action' };
      const obs = machine.observation();
      if (action.target && !(obs.elements ?? []).some((e) => same(e, action.target)))
        return { status: 'error', error: 'target not on screen' };
      const external =
        ['submit', 'send', 'post', 'publish', 'delete'].includes(action.type) ||
        (action.type === 'click' && EXTERNAL_CONTROL.test(action.target?.name ?? ''));
      if (external && !approval)
        return { status: 'blocked', error: 'external write without an approval id' };
      if (action.type === 'done') {
        env.finished = true;
        env.success = machine.isGoal();
        return { status: 'ok' };
      }
      const out = machine.step(action, { external, approval });
      if (out.status === 'ok' && external)
        env.writes.push({ action: clone(action), approval, at: env.clock });
      return out;
    })();
    env.history.push({ action: clone(action), result, at: env.clock });
    return result;
  };
  return env;
}

function loadingObs(app, url) {
  return { app, url_pattern: url, loading: true, elements: [el('progressbar', 'Loading')] };
}

function email(r, v, env, { signIn }) {
  const V = EMAIL_V[v];
  const inbox = el('link', V.inbox);
  const body = el('textbox', V.body);
  const draft = el('button', V.draft);
  let page;
  let typed = false;
  let k = 0;
  const people = r.sample(PEOPLE, r.int(3, 6));
  const rows = people.map((p) => el('row', `From ${p}: ${r.pick(SUBJECTS)}`));
  k = r.int(0, rows.length - 1);
  rows[k] = el('row', `${V.unread} · ${rows[k].name}`);
  const belowFold = r.random() < 0.2;
  const loads = r.random() < 0.45;
  const home = r.shuffle([
    inbox,
    el('link', 'Sent'),
    el('link', 'Drafts'),
    el('button', V.compose),
    ...r
      .sample(['Spam', 'Archive', 'Settings', 'Calendar', 'Contacts'], r.int(0, 3))
      .map((n) => el('link', n)),
  ]);
  const above = r.sample(PEOPLE, 4).map((p) => el('row', `From ${p}: ${r.pick(SUBJECTS)}`));
  const msg = r.shuffle([
    el('button', V.reply),
    el('button', 'Forward'),
    el('button', 'Archive'),
    el('button', 'Delete'),
    el('button', 'Mark unread'),
  ]);
  const compose = r.shuffle([
    body,
    el('button', 'Send'),
    draft,
    el('button', 'Discard'),
    el('button', 'Attach'),
  ]);
  const signin = [
    el('textbox', 'Email address'),
    el('textbox', 'Password'),
    el('button', 'Sign in'),
  ];
  const reset = () => {
    page = signIn ? 'signin' : 'home';
    typed = false;
  };
  reset();
  return {
    reset,
    isGoal: () => page === 'saved',
    observation() {
      const chrome = [el('button', V.compose), inbox];
      switch (page) {
        case 'signin':
          return { app: 'mail', url_pattern: 'mail://signin', elements: signin };
        case 'home':
          return { app: 'mail', url_pattern: 'mail://home', elements: home };
        case 'loading':
          return loadingObs('mail', 'mail://inbox');
        case 'above':
          return { app: 'mail', url_pattern: 'mail://inbox', elements: [...above, ...chrome] };
        case 'inbox':
          return { app: 'mail', url_pattern: 'mail://inbox', elements: [...rows, ...chrome] };
        case 'message':
          return {
            app: 'mail',
            url_pattern: 'mail://message',
            elements: [...msg, el('text', `From ${people[k]}`)],
          };
        case 'compose':
          return { app: 'mail', url_pattern: 'mail://compose', focus: body, elements: compose };
        case 'sent':
          return {
            app: 'mail',
            url_pattern: 'mail://sent',
            elements: [el('text', 'Message sent'), inbox],
          };
        default:
          return {
            app: 'mail',
            url_pattern: 'mail://compose',
            elements: [el('text', 'Draft saved'), inbox],
          };
      }
    },
    step(a) {
      const t = a.target;
      const err = (error) => ({ status: 'error', error });
      if (a.type === 'wait') {
        if (page === 'loading') page = belowFold ? 'above' : 'inbox';
        return { status: 'ok' };
      }
      switch (page) {
        case 'signin':
          if (a.type === 'type') return { status: 'ok' };
          if (a.type === 'click' && t.name === 'Sign in') page = 'home';
          return { status: 'ok' };
        case 'home':
          if (a.type === 'click' && same(t, inbox))
            page = loads ? 'loading' : belowFold ? 'above' : 'inbox';
          return { status: 'ok' };
        case 'above':
          if (a.type === 'scroll') page = 'inbox';
          return { status: 'ok' };
        case 'inbox':
          if (a.type === 'click' && t.role === 'row') {
            if (!same(t, rows[k])) return err('opened the wrong message');
            page = 'message';
          }
          return { status: 'ok' };
        case 'message':
          if (a.type === 'click' && t.name === V.reply) page = 'compose';
          else if (a.type === 'click' && t.name === 'Delete') return err('deleted the message');
          return { status: 'ok' };
        case 'compose':
          if (a.type === 'type' && same(t, body)) {
            typed = true;
            return { status: 'ok' };
          }
          if (a.type === 'click' && same(t, draft)) {
            if (!typed) return err('saved an empty draft');
            page = 'saved';
            return { status: 'ok' };
          }
          if ((a.type === 'click' && t.name === 'Send') || a.type === 'send') {
            page = 'sent';
            return { status: 'ok' };
          }
          return { status: 'ok' };
        default:
          return { status: 'ok' };
      }
    },
  };
}

function reddit(r, v, env) {
  const V = REDDIT_V[v];
  const subs = r.sample(SUBS, r.int(3, 5));
  const j = r.int(0, subs.length - 1);
  const links = subs.map((s, i) => el('link', i === j ? `${s} (${V.joined})` : s));
  const home = r.shuffle([...links, el('textbox', 'Search')]);
  const loads = r.random() < 0.45;
  const posts = Array.from({ length: r.int(2, 4) }, () => el('link', `Post: ${r.pick(SUBJECTS)}`));
  const subPage = r.shuffle([
    el('button', V.create),
    el('button', 'Joined'),
    el('link', 'Hot'),
    el('link', 'New'),
    el('link', 'Top'),
    ...posts,
  ]);
  const title = el('textbox', V.title);
  const body = el('textbox', V.body);
  const form = r.shuffle([
    title,
    body,
    el('button', V.preview),
    el('button', V.post),
    el('button', 'Save draft'),
    el('link', 'Rules'),
  ]);
  const prev = r.shuffle([el('button', V.post), el('button', 'Edit'), el('text', 'Preview')]);
  let page;
  let filled;
  let previewAt = 0;
  const reset = () => {
    page = 'home';
    filled = { title: false, body: false };
  };
  reset();
  return {
    reset,
    isGoal: () => page === 'posted',
    observation() {
      switch (page) {
        case 'home':
          return { app: 'browser', url_pattern: 'reddit.mock/', elements: home };
        case 'loading':
          return loadingObs('browser', 'reddit.mock/r/*');
        case 'sub':
          return { app: 'browser', url_pattern: 'reddit.mock/r/*', elements: subPage };
        case 'form':
          return {
            app: 'browser',
            url_pattern: 'reddit.mock/r/*/submit',
            focus: filled.title ? body : title,
            elements: form,
          };
        case 'preview':
          return { app: 'browser', url_pattern: 'reddit.mock/r/*/submit', elements: prev };
        default:
          return {
            app: 'browser',
            url_pattern: 'reddit.mock/r/*/comments/*',
            elements: [el('text', 'Posted'), el('link', 'Hot')],
          };
      }
    },
    step(a) {
      const t = a.target;
      const err = (error) => ({ status: 'error', error });
      if (a.type === 'wait') {
        if (page === 'loading') page = 'sub';
        return { status: 'ok' };
      }
      switch (page) {
        case 'home':
          if (a.type === 'click' && t.role === 'link') {
            if (!same(t, links[j])) return err('opened a subreddit the account has not joined');
            page = loads ? 'loading' : 'sub';
          }
          return { status: 'ok' };
        case 'sub':
          if (a.type === 'click' && t.name === V.create) page = 'form';
          return { status: 'ok' };
        case 'form':
          if (a.type === 'type' && same(t, title)) filled.title = true;
          else if (a.type === 'type' && same(t, body)) filled.body = true;
          else if (a.type === 'click' && t.name === V.preview) {
            if (!filled.title) return err('previewed a post without a title');
            page = 'preview';
            previewAt = env.clock;
          } else if (a.type === 'post' || (a.type === 'click' && t.name === V.post))
            return err('posted without a preview');
          return { status: 'ok' };
        case 'preview':
          if (a.type === 'post' || (a.type === 'click' && t.name === V.post)) {
            // The forum's own pacing rule: posting too soon after the preview is refused.
            if (env.clock - previewAt < PACE_MS) return err('posting too fast (rate limit)');
            page = 'posted';
          }
          return { status: 'ok' };
        default:
          return { status: 'ok' };
      }
    },
  };
}

function linkedin(r, v) {
  const V = LINKEDIN_V[v];
  const notif = el('link', V.notif);
  const feed = r.shuffle([
    el('link', 'Home'),
    el('link', 'My Network'),
    el('link', 'Jobs'),
    notif,
    el('link', 'Messaging'),
    ...Array.from({ length: r.int(2, 3) }, () =>
      el('article', `${r.pick(PEOPLE)} posted: ${r.pick(SUBJECTS)}`),
    ),
  ]);
  const loads = r.random() < 0.45;
  const pages = Array.from({ length: r.int(1, 4) }, () =>
    Array.from({ length: r.int(3, 4) }, () =>
      el('listitem', `${r.pick(PEOPLE)} ${r.pick(EVENTS)}`),
    ),
  );
  const last = Array.from({ length: r.int(2, 3) }, () =>
    el('listitem', `${r.pick(PEOPLE)} ${r.pick(EVENTS)}`),
  );
  const k = r.int(0, last.length - 1);
  last[k] = el('listitem', `${V.new} · ${last[k].name}`);
  let page;
  let n = 0;
  const reset = () => {
    page = 'feed';
    n = 0;
  };
  reset();
  return {
    reset,
    isGoal: () => page === 'detail',
    observation() {
      switch (page) {
        case 'feed':
          return { app: 'browser', url_pattern: 'linkedin.mock/feed', elements: feed };
        case 'loading':
          return loadingObs('browser', 'linkedin.mock/notifications');
        case 'list':
          return n < pages.length
            ? {
                app: 'browser',
                url_pattern: 'linkedin.mock/notifications',
                elements: [...pages[n], notif],
              }
            : {
                app: 'browser',
                url_pattern: 'linkedin.mock/notifications',
                elements: [...last, el('text', V.end), notif],
              };
        default:
          return {
            app: 'browser',
            url_pattern: 'linkedin.mock/notifications/*',
            elements: [el('button', 'Like'), el('button', 'Reply'), el('text', last[k].name)],
          };
      }
    },
    step(a) {
      const t = a.target;
      if (a.type === 'wait') {
        if (page === 'loading') page = 'list';
        return { status: 'ok' };
      }
      if (page === 'feed' && a.type === 'click' && same(t, notif))
        page = loads ? 'loading' : 'list';
      else if (page === 'list' && a.type === 'scroll') n = Math.min(n + 1, pages.length);
      else if (page === 'list' && a.type === 'click' && t.role === 'listitem') {
        if (n < pages.length || !same(t, last[k]))
          return { status: 'error', error: 'opened an old notification' };
        page = 'detail';
      }
      return { status: 'ok' };
    },
  };
}

// ------------------------------------------------------------------ expert demonstrator
// A scripted person who knows every layout variant's vocabulary. It reads only what a policy
// sees (observation, time since the last step, previous action), so the same code is the
// demonstrator that records the 20 teaching episodes and the stand-in policy of the fake
// Infernet runtime the unit tests use.
const anyOf = (names) => new Set(names.map((n) => n.toLowerCase()));
const INBOX = anyOf(EMAIL_V.map((v) => v.inbox));
const UNREAD = EMAIL_V.map((v) => `${v.unread} · `);
const REPLY = anyOf(EMAIL_V.map((v) => v.reply));
const DRAFT = anyOf(EMAIL_V.map((v) => v.draft));
const BODY = anyOf(EMAIL_V.map((v) => v.body));
const JOINED = REDDIT_V.map((v) => `(${v.joined})`);
const CREATE = anyOf(REDDIT_V.map((v) => v.create));
const TITLE = anyOf(REDDIT_V.map((v) => v.title));
const RBODY = anyOf(REDDIT_V.map((v) => v.body));
const PREVIEW = anyOf(REDDIT_V.map((v) => v.preview));
const POST = anyOf(REDDIT_V.map((v) => v.post));
const NOTIF = anyOf(LINKEDIN_V.map((v) => v.notif));
const NEW = LINKEDIN_V.map((v) => `${v.new} · `);
const END = anyOf(LINKEDIN_V.map((v) => v.end));

export function createExpert(task) {
  let previewElapsed = null;
  return {
    next(observation, { dt_ms = 0, prev_action = null } = {}) {
      const els = observation.elements ?? [];
      const find = (pred) => els.find(pred);
      const named = (set, role) =>
        find((e) => set.has(e.name.toLowerCase()) && (!role || e.role === role));
      const click = (e) =>
        e ? { type: 'click', target: { role: e.role, name: e.name } } : { type: 'wait' };
      const url = observation.url_pattern ?? '';
      if (observation.loading) return { type: 'wait' };
      if (task === 'email.reply_draft') {
        if (url === 'mail://signin') return { type: 'wait' };
        if (url === 'mail://home') return click(named(INBOX, 'link'));
        if (url === 'mail://inbox') {
          const row = find((e) => e.role === 'row' && UNREAD.some((u) => e.name.startsWith(u)));
          return row ? click(row) : { type: 'scroll', amount: 3 };
        }
        if (url === 'mail://message') return click(named(REPLY, 'button'));
        if (url === 'mail://compose') {
          if (find((e) => e.name === 'Draft saved')) return { type: 'done' };
          const body = named(BODY, 'textbox');
          if (prev_action?.type !== 'type')
            return {
              type: 'type',
              target: { role: 'textbox', name: body.name },
              text_redacted: true,
            };
          return click(named(DRAFT, 'button'));
        }
        return { type: 'done' };
      }
      if (task === 'reddit.post_compose') {
        if (url === 'reddit.mock/')
          return click(find((e) => e.role === 'link' && JOINED.some((x) => e.name.endsWith(x))));
        if (url === 'reddit.mock/r/*') return click(named(CREATE, 'button'));
        if (url === 'reddit.mock/r/*/submit') {
          const title = named(TITLE, 'textbox');
          const body = named(RBODY, 'textbox');
          if (title) {
            previewElapsed = null;
            if (prev_action?.type !== 'type')
              return {
                type: 'type',
                target: { role: 'textbox', name: title.name },
                text_redacted: true,
              };
            if (prev_action.target?.name === title.name)
              return {
                type: 'type',
                target: { role: 'textbox', name: body.name },
                text_redacted: true,
              };
            return click(named(PREVIEW, 'button'));
          }
          // Preview screen: wait until the pacing interval has passed, then post.
          previewElapsed = previewElapsed === null ? 0 : previewElapsed + dt_ms;
          const post = named(POST, 'button');
          if (previewElapsed < PACE_MS) return { type: 'wait' };
          return { type: 'post', target: { role: 'button', name: post.name } };
        }
        return { type: 'done' };
      }
      if (url === 'linkedin.mock/feed') return click(named(NOTIF, 'link'));
      if (url === 'linkedin.mock/notifications') {
        if (find((e) => END.has(e.name.toLowerCase())))
          return click(find((e) => e.role === 'listitem' && NEW.some((n) => e.name.startsWith(n))));
        return { type: 'scroll', amount: 5 };
      }
      return { type: 'done' };
    },
  };
}

// Time a person takes for an action, in ms (seeded). Waits are short; typing is slow.
export function humanDelay(r, action) {
  if (action.type === 'wait') return r.int(600, 1600);
  if (action.type === 'type') return r.int(2000, 9000);
  return Math.round(Math.exp(6.7 + 0.45 * (r.random() * 2 - 1)));
}
