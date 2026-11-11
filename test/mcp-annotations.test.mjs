import { describe, expect, it } from 'bun:test';
import { Client } from '@modelcontextprotocol/sdk/client/index.js';
import { InMemoryTransport } from '@modelcontextprotocol/sdk/inMemory.js';
import { createMcp } from '../src/mcp.mjs';

// Claude + ChatGPT directory rules: a human title and explicit hint booleans on every tool.
describe('MCP tool annotations', () => {
  it('every tool has a title and readOnly/destructive/openWorld hints', async () => {
    const server = createMcp(async () => ({}), { browse: async () => ({}), ask: async () => ({}) });
    const [a, b] = InMemoryTransport.createLinkedPair();
    const client = new Client({ name: 'test', version: '0' });
    await Promise.all([server.connect(a), client.connect(b)]);
    const { tools } = await client.listTools();
    expect(tools.length).toBe(12);
    for (const t of tools) {
      expect(typeof t.title === 'string' && t.title.length > 0).toBe(true);
      for (const hint of ['readOnlyHint', 'destructiveHint', 'openWorldHint']) {
        expect(typeof t.annotations?.[hint]).toBe('boolean');
      }
      if (t.annotations.readOnlyHint) expect(t.annotations.destructiveHint).toBe(false);
    }
    await client.close();
  });
});
