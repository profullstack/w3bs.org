// `ask` on the server: POST /api/v1/ask and the w3bs_ask MCP tool (docs/ask.md).
// Same flow as `w3bs ask` (package-manager/crates/w3bs-cli/src/ask.rs): the agent registry in
// fixtures/agents.json, pit.world search for grounding, then Infernet's public chat API with
// p2p_only, so a c0mpute/Infernet node answers and never a hosted (paid) model.
import { readFileSync } from 'node:fs';
import { W3bsError } from './core.mjs';

const REGISTRY = JSON.parse(
  readFileSync(new URL('../fixtures/agents.json', import.meta.url), 'utf8'),
);
const SNIPPET = 700;

export function agents(registry = REGISTRY) {
  return (registry.agents ?? [])
    .filter((a) => /^[a-z0-9-]{1,40}$/.test(a?.name ?? '') && a.model && a.system)
    .map((a) => ({
      name: a.name,
      aliases: (a.aliases ?? []).map((x) => String(x).toLowerCase()),
      title: a.title ?? '',
      by: a.by ?? '',
      model: a.model,
      huggingface: a.huggingface ?? '',
      gguf: a.gguf ?? '',
      system: a.system,
      search: Boolean(a.grounding?.search),
      querySuffix: a.grounding?.query_suffix ?? '',
      results: Math.min(Math.max(Number(a.grounding?.results) || 3, 1), 8),
      maxTokens: Math.min(Math.max(Number(a.max_tokens) || 350, 16), 2048),
      temperature: Number.isFinite(a.temperature) ? a.temperature : 0.2,
    }));
}

export const publicAgent = (a) => ({
  name: a.name,
  aliases: a.aliases,
  title: a.title,
  by: a.by,
  model: a.model,
  huggingface: a.huggingface,
  gguf: a.gguf,
  grounding: a.search ? 'pit.world' : 'none',
});

export function findAgent(name, list = agents()) {
  const n = String(name ?? '')
    .trim()
    .replace(/^@/, '')
    .toLowerCase();
  return list.find((a) => a.name === n) ?? list.find((a) => a.aliases.includes(n)) ?? null;
}

export function searchQuery(question, suffix = '') {
  const q = question
    .replace(/([?!.])\1+/g, '$1')
    .split(/\s+/)
    .filter(Boolean)
    .join(' ');
  return suffix ? `${q} ${suffix}` : q;
}

export function pickSources(body, k) {
  const all = (Array.isArray(body?.results) ? body.results : [])
    .filter(
      (r) => typeof r?.title === 'string' && r.title.trim() && /^https?:\/\//.test(r?.url ?? ''),
    )
    .map((r) => ({
      title: r.title.trim(),
      url: r.url.trim(),
      text: String(r.description ?? '')
        .split(/\s+/)
        .filter(Boolean)
        .join(' ')
        .slice(0, SNIPPET),
    }));
  return [...all.filter((s) => s.text), ...all.filter((s) => !s.text)].slice(0, k);
}

// Question first: with the background first, the 0.5B strategist repeats its system prompt.
// Layout 1 is the retry after an echo: the background moves into the system message.
export function messages(agent, question, sources, layout = 0) {
  if (!sources.length)
    return [
      { role: 'system', content: agent.system },
      { role: 'user', content: question },
    ];
  const ctx = sources.map((s, i) => `[${i + 1}] ${s.title} (${s.url})\n${s.text}`).join('\n\n');
  if (layout === 1)
    return [
      { role: 'system', content: `${agent.system}\n\nNotes from the web you may use:\n${ctx}` },
      { role: 'user', content: question },
    ];
  return [
    { role: 'system', content: agent.system },
    {
      role: 'user',
      content: `Question: ${question}\n\nNotes from the web:\n${ctx}\n\nAnswer the question in a few practical steps.`,
    },
  ];
}

export function stripEcho(answer, system) {
  const a = String(answer ?? '').trim();
  return a.startsWith(system.trim()) ? a.slice(system.trim().length).trim() : a;
}

/** Parse an SSE body incrementally: push text, get complete [event, data] frames. */
export function sseParser() {
  let buf = '';
  return (chunk) => {
    buf += chunk.replace(/\r\n/g, '\n');
    const out = [];
    let end;
    while ((end = buf.indexOf('\n\n')) >= 0) {
      const frame = buf.slice(0, end);
      buf = buf.slice(end + 2);
      let event = 'message';
      const data = [];
      for (const line of frame.split('\n')) {
        if (line.startsWith('event:')) event = line.slice(6).trim();
        else if (line.startsWith('data:')) data.push(line.slice(5).replace(/^ /, ''));
      }
      if (!data.length) continue;
      let value = data.join('\n');
      try {
        value = JSON.parse(value);
      } catch {}
      out.push([event, value]);
    }
    return out;
  };
}

const fail = (code, message, status = 502) => new W3bsError(code, message, status);

export async function ask(
  { agent: name, question, search = true },
  {
    infernet = process.env.W3BS_INFERNET_API || 'https://infernetprotocol.com',
    searchUrl = process.env.W3BS_SEARCH_URL || 'https://pit.world/api/search',
    key = process.env.INFERNET_API_KEY,
    timeoutMs = 240_000,
    fetchImpl = fetch,
  } = {},
) {
  const list = agents();
  const agent = findAgent(name ?? list[0]?.name, list);
  if (!agent)
    throw new W3bsError(
      'ASK_UNKNOWN_AGENT',
      `No agent called ${JSON.stringify(String(name))}; agents: ${list.map((a) => a.name).join(', ')}.`,
      404,
    );
  const q = String(question ?? '').trim();
  if (!q || q.length > 2000)
    throw new W3bsError('ASK_NO_QUESTION', 'Ask a question of 1 to 2000 characters.', 400);
  const started = Date.now();
  const warnings = [];
  let sources = [];
  if (search && agent.search) {
    try {
      const u = new URL(searchUrl);
      u.searchParams.set('q', searchQuery(q, agent.querySuffix));
      const r = await fetchImpl(u, {
        headers: { accept: 'application/json', 'user-agent': 'w3bs.org (ask)' },
        signal: AbortSignal.timeout(20_000),
      });
      if (!r.ok) throw new Error(`HTTP ${r.status}`);
      sources = pickSources(await r.json(), agent.results);
    } catch (e) {
      warnings.push(`pit.world search failed (${e.message}); answering without sources`);
    }
  }
  let result;
  for (let attempt = 0; attempt < 2 && !result; attempt++) {
    const auth = key ? { authorization: `Bearer ${key}` } : {};
    const origin = new URL(infernet).origin;
    const posted = await fetchImpl(`${origin}/api/chat`, {
      method: 'POST',
      headers: { 'content-type': 'application/json', ...auth },
      body: JSON.stringify({
        modelName: agent.model,
        messages: messages(agent, q, sources, attempt),
        maxTokens: agent.maxTokens,
        temperature: agent.temperature,
        p2p_only: true,
      }),
      signal: AbortSignal.timeout(30_000),
    }).catch((e) => {
      throw fail('ASK_UNAVAILABLE', `Infernet is unreachable: ${e.message}`);
    });
    const job = await posted.json().catch(() => ({}));
    if (posted.status === 429)
      throw fail(
        'ASK_RATE_LIMITED',
        'The c0mpute network is rate limiting questions right now; try again later.',
        429,
      );
    if (posted.status === 503)
      throw fail(
        'ASK_NO_NODE',
        `No c0mpute node is serving ${agent.model} right now; try again in a few minutes.`,
        503,
      );
    if (!posted.ok) throw fail('ASK_FAILED', `Infernet: ${job.error ?? `HTTP ${posted.status}`}`);
    if (job.source !== 'p2p')
      throw fail(
        'ASK_NOT_P2P',
        `Infernet offered a ${job.source ?? 'unknown'} route, not a c0mpute node; refused.`,
      );
    if (!job.jobId || !/^\/api\/chat\/stream\/[\w-]+$/.test(job.streamUrl ?? ''))
      throw fail('ASK_FAILED', "Infernet's answer had no job stream.");
    const node = {
      id: job.provider?.nodeId ?? null,
      name: job.provider?.name ?? null,
      provider_id: job.provider?.id ?? null,
      engine: null,
    };
    const stream = await fetchImpl(`${origin}${job.streamUrl}`, {
      headers: { accept: 'text/event-stream', ...auth },
      signal: AbortSignal.timeout(timeoutMs),
    }).catch((e) => {
      throw fail('ASK_UNAVAILABLE', `The answer stream failed: ${e.message}`);
    });
    if (!stream.ok || !stream.body)
      throw fail('ASK_FAILED', `The answer stream returned HTTP ${stream.status}.`);
    const parse = sseParser();
    const decoder = new TextDecoder();
    let streamed = '',
      done = null,
      firstToken = null;
    try {
      outer: for await (const chunk of stream.body) {
        for (const [event, data] of parse(decoder.decode(chunk, { stream: true }))) {
          if (event === 'meta') {
            node.id = data.provider_node_id ?? node.id;
            node.name = data.provider_name ?? node.name;
            node.engine = data.engine ?? null;
          } else if (event === 'token' && typeof data.text === 'string') {
            firstToken ??= Date.now() - started;
            streamed += data.text;
          } else if (event === 'done') {
            done = typeof data.text === 'string' ? data.text : streamed;
            break outer;
          } else if (event === 'error') {
            throw fail(
              'ASK_FAILED',
              `${node.name ?? 'The node'} could not answer: ${data.message ?? data.error ?? 'error'}`,
            );
          }
        }
      }
    } catch (e) {
      if (e instanceof W3bsError) throw e;
      throw fail('ASK_TIMEOUT', `No complete answer (job ${job.jobId}): ${e.message}`, 504);
    }
    if (done === null)
      throw fail(
        'ASK_FAILED',
        `The answer stream ended before the node finished (job ${job.jobId}).`,
      );
    const answer = stripEcho(done, agent.system);
    if (!answer) {
      if (attempt === 0) continue;
      throw fail(
        'ASK_NO_ANSWER',
        `${agent.name} repeated its instructions instead of answering, twice (job ${job.jobId}); ask again, perhaps in other words.`,
      );
    }
    result = { answer, node, job, firstToken };
  }
  const { answer, node, job, firstToken } = result;
  return {
    agent: agent.name,
    title: agent.title,
    model: agent.model,
    question: q,
    answer,
    sources: sources.map((s, i) => ({ n: i + 1, title: s.title, url: s.url })),
    node,
    transport: 'infernet',
    job_id: job.jobId,
    latency_ms: Date.now() - started,
    first_token_ms: firstToken,
    warnings,
  };
}

/** A fixed-window limiter by key (client IP): `max` per `windowMs`. */
export function limiter({ max, windowMs }) {
  const hits = new Map();
  return (key, now = Date.now()) => {
    if (hits.size > 10_000) for (const [k, v] of hits) if (v.reset <= now) hits.delete(k);
    let h = hits.get(key);
    if (!h || h.reset <= now) hits.set(key, (h = { count: 0, reset: now + windowMs }));
    h.count += 1;
    return {
      ok: h.count <= max,
      remaining: Math.max(0, max - h.count),
      resetAt: new Date(h.reset).toISOString(),
    };
  };
}
