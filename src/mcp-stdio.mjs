#!/usr/bin/env node
import { StdioServerTransport } from '@modelcontextprotocol/sdk/server/stdio.js';
import { createMcp } from './mcp.mjs';
import { request } from './client.mjs';

await createMcp((operation, args) => request(operation, args)).connect(new StdioServerTransport());
