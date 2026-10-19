#!/usr/bin/env node
import { StdioServerTransport } from '@modelcontextprotocol/sdk/server/stdio.js';
import { createMcp } from './mcp.mjs';
import { request } from './client.mjs';

await createMcp((operation, args) => request(operation, args), {
  browse: (q) => request('browse', { q }),
}).connect(new StdioServerTransport());
