#!/usr/bin/env node
import { StdioServerTransport } from '@modelcontextprotocol/sdk/server/stdio.js';
import { createMcp } from './mcp.mjs';
import { request, apiOrigin } from './client.mjs';

// An answer takes 5 to 60 s, longer than request()'s 15 s budget.
async function ask(args) {
  const r = await fetch(`${apiOrigin()}/api/v1/ask`, {
    method: 'POST',
    redirect: 'error',
    signal: AbortSignal.timeout(300_000),
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify(args),
  });
  const body = await r.json().catch(() => ({}));
  if (!r.ok) throw Object.assign(new Error(body.error?.message ?? `HTTP ${r.status}`), { code: body.error?.code });
  return body;
}

await createMcp((operation, args) => request(operation, args), {
  browse: (q) => request('browse', { q }),
  ask,
}).connect(new StdioServerTransport());
