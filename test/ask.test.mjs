import test from 'node:test';
import assert from 'node:assert/strict';
import { createServer } from 'node:http';
import { Client } from '@modelcontextprotocol/sdk/client/index.js';
import { StreamableHTTPClientTransport } from '@modelcontextprotocol/sdk/client/streamableHttp.js';
import { startServer } from '../src/server.mjs';
import { openStore } from '../src/store.mjs';
import { agents, findAgent, searchQuery, pickSources, messages, stripEcho, sseParser, limiter } from '../src/ask.mjs';

test('the registry, aliases, query, sources, prompt and SSE framing', () => {
  const list = agents();
  const b = findAgent('saas', list);
  assert.equal(b.name, 'business');
  assert.equal(b.model, 'hf.co/profullstack/saas-business-strategist-agent-GGUF:latest');
  assert.equal(findAgent('@Strategist', list).name, 'business');
  assert.equal(findAgent('nobody', list), null);
  assert.equal(b.disclaimer, '');
  assert.equal(findAgent('lawyer', list).model, 'hf.co/profullstack/not-a-lawyer-agent-GGUF:latest');
  assert.match(findAgent('@IRS', list).disclaimer, /^Not a CPA\. Not tax advice\./);
  assert.match(findAgent('legal', list).disclaimer, /^Not a lawyer\. Not legal advice\./);
  assert.equal(searchQuery("I can't make money???? HELP!", 'startup revenue'), "I can't make money? HELP! startup revenue");
  const s = pickSources(
    {
      results: [
        { title: 'Bare', url: 'https://a.example/' },
        { title: 'Good', url: 'https://b.example/', description: 'x  y' },
        { title: 'Bad', url: 'javascript:1', description: 'z' },
      ],
    },
    3,
  );
  assert.deepEqual(s.map((x) => x.title), ['Good', 'Bare']);
  assert.equal(s[0].text, 'x y');
  const m = messages(b, 'Q?', s.slice(0, 1));
  assert.equal(m[1].content, 'Question: Q?\n\nNotes from the web:\n[1] Good (https://b.example/)\nx y\n\nAnswer the question in a few practical steps.');
  assert.equal(stripEcho(`${b.system} Do X.`, b.system), 'Do X.');
  const p = sseParser();
  assert.deepEqual(p('event: token\ndata: {"text":"a"}\n'), []);
  assert.deepEqual(p('\r\n\r\nevent: done\ndata: {"text":"ab"}\n\n'), [
    ['token', { text: 'a' }],
    ['done', { text: 'ab' }],
  ]);
  const l = limiter({ max: 2, windowMs: 1000 });
  assert.equal(l('ip', 0).ok, true);
  assert.equal(l('ip', 1).ok, true);
  assert.equal(l('ip', 2).ok, false);
  assert.equal(l('ip', 1001).ok, true);
});

// A fake pit.world search + Infernet chat API on one port.
async function fakeNetwork() {
  const seen = [];
  let posts = 0;
  const server = createServer(async (req, res) => {
    let body = '';
    for await (const c of req) body += c;
    seen.push({ method: req.method, url: req.url, body: body ? JSON.parse(body) : null, auth: req.headers.authorization ?? '' });
    if (req.url.startsWith('/search')) {
      res.setHeader('content-type', 'application/json');
      return res.end(JSON.stringify({ results: [{ title: 'One', url: 'https://one.example/', description: 'first' }] }));
    }
    if (req.method === 'POST' && req.url === '/api/chat') {
      res.setHeader('content-type', 'application/json');
      posts += 1;
      return res.end(JSON.stringify({ jobId: `j${posts}`, source: 'p2p', provider: { id: 'p1', name: 'node-one', nodeId: 'provider-1' }, streamUrl: `/api/chat/stream/j${posts}` }));
    }
    // Odd jobs only repeat the system prompt: the server must ask again.
    const job = /^\/api\/chat\/stream\/j(\d+)$/.exec(req.url);
    if (job && Number(job[1]) % 2 === 1) {
      res.setHeader('content-type', 'text/event-stream');
      const sys = 'You are saas-business-strategist-agent. You give concise, practical business strategy grounded in what independent writers on these subjects are publishing.';
      return res.end(`event: done\ndata: ${JSON.stringify({ text: sys })}\n\n`);
    }
    if (job) {
      res.setHeader('content-type', 'text/event-stream');
      res.write('event: meta\ndata: {"provider_name":"node-one","provider_node_id":"provider-1","engine":"ollama"}\n\n');
      res.write('event: token\ndata: {"text":"Find "}\n\n');
      return res.end('event: token\ndata: {"text":"customers."}\n\nevent: done\ndata: {"text":"Find customers."}\n\n');
    }
    res.statusCode = 404;
    res.end('{}');
  });
  await new Promise((r) => server.listen(0, '127.0.0.1', r));
  return { origin: `http://127.0.0.1:${server.address().port}`, seen, close: () => server.close() };
}

test('POST /api/v1/ask and the w3bs_ask MCP tool route p2p_only, ground and throttle', async () => {
  const net = await fakeNetwork();
  const saved = { ...process.env };
  Object.assign(process.env, {
    W3BS_INFERNET_API: net.origin,
    W3BS_SEARCH_URL: `${net.origin}/search`,
    INFERNET_API_KEY: 'ifc_test',
    W3BS_ASK_PER_HOUR: '2',
  });
  const instance = await startServer({ store: await openStore({ path: ':memory:' }), port: 0, publicOrigin: 'http://localhost:3000' });
  const base = `http://127.0.0.1:${instance.server.address().port}`;
  const post = (body) =>
    fetch(`${base}/api/v1/ask`, { method: 'POST', headers: { 'content-type': 'application/json' }, body: JSON.stringify(body) });
  try {
    const listed = await (await fetch(`${base}/api/v1/ask/agents`)).json();
    assert.equal(listed.agents[0].name, 'business');
    assert.equal(listed.agents[0].system, undefined);

    const r = await post({ agent: 'business', question: 'How do I make money??' });
    assert.equal(r.status, 200);
    const d = await r.json();
    assert.equal(d.answer, 'Find customers.');
    assert.deepEqual(d.sources, [{ n: 1, title: 'One', url: 'https://one.example/' }]);
    assert.equal(d.node.name, 'node-one');
    assert.equal(d.node.engine, 'ollama');
    const chats = net.seen.filter((x) => x.url === '/api/chat');
    assert.equal(chats.length, 2, 'the echo was retried once');
    assert.equal(chats[1].body.messages[1].content, 'How do I make money??');
    assert.match(chats[1].body.messages[0].content, /Notes from the web you may use:/);
    assert.equal(d.job_id, 'j2');
    const chat = chats[0];
    assert.equal(chat.body.p2p_only, true);
    assert.equal(chat.body.modelName, 'hf.co/profullstack/saas-business-strategist-agent-GGUF:latest');
    assert.equal(chat.auth, 'Bearer ifc_test');
    assert.match(chat.body.messages[1].content, /^Question: How do I make money\?\?/);

    const client = new Client({ name: 't', version: '0' });
    await client.connect(new StreamableHTTPClientTransport(new URL(`${base}/mcp`)));
    const tool = await client.callTool({ name: 'w3bs_ask', arguments: { agent: 'saas', question: 'Help?' } });
    assert.equal(tool.structuredContent.answer, 'Find customers.');
    assert.equal(tool.structuredContent.node.id, 'provider-1');
    await client.close();

    assert.equal((await post({ agent: 'nobody', question: 'x' })).status, 429, 'the third question in the hour is throttled');
  } finally {
    await instance.close();
    net.close();
    for (const k of ['W3BS_INFERNET_API', 'W3BS_SEARCH_URL', 'INFERNET_API_KEY', 'W3BS_ASK_PER_HOUR']) {
      if (k in saved) process.env[k] = saved[k];
      else delete process.env[k];
    }
  }
});

test('ask refuses unknown agents, empty questions and non-p2p routes', async () => {
  const { ask } = await import('../src/ask.mjs');
  await assert.rejects(ask({ agent: 'nobody', question: 'x' }), { code: 'ASK_UNKNOWN_AGENT' });
  await assert.rejects(ask({ agent: 'business', question: ' ' }), { code: 'ASK_NO_QUESTION' });
  const fetchImpl = async (url) =>
    new Response(JSON.stringify(String(url).includes('/api/chat') ? { jobId: 'j', source: 'nim', streamUrl: '/api/chat/stream/j' } : { results: [] }), {
      headers: { 'content-type': 'application/json' },
    });
  await assert.rejects(ask({ agent: 'business', question: 'x' }, { fetchImpl, infernet: 'http://127.0.0.1:9' }), { code: 'ASK_NOT_P2P' });
});
