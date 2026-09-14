#!/usr/bin/env node
import { mkdirSync, writeFileSync } from 'node:fs';
import { homedir } from 'node:os';
import { resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
import { execFileSync } from 'node:child_process';

if (process.platform !== 'linux')
  throw new Error('This installer supports Linux desktop environments.');
if (!process.argv.includes('--install'))
  throw new Error('Run with --install to register w3bs:// for your user.');
const applications = resolve(
  process.env.XDG_DATA_HOME || resolve(homedir(), '.local/share'),
  'applications',
);
mkdirSync(applications, { recursive: true });
// Desktop-entry Exec quoting, with no shell. Percent signs must be escaped too.
const quote = (value) => '"' + value.replaceAll('%', '%%').replace(/["`$\\]/g, '\\$&') + '"';
const entry = `[Desktop Entry]\nType=Application\nName=W3BS Resource Inspector\nComment=Verify and inspect a W3BS resource\nExec=${quote(process.execPath)} ${quote(fileURLToPath(new URL('./client.mjs', import.meta.url)))} --open %u\nTerminal=false\nNoDisplay=true\nMimeType=x-scheme-handler/w3bs;\n`;
writeFileSync(resolve(applications, 'w3bs-inspector.desktop'), entry);
execFileSync('xdg-mime', ['default', 'w3bs-inspector.desktop', 'x-scheme-handler/w3bs']);
process.stdout.write(
  'Registered w3bs:// for the current user. Remove the desktop entry to uninstall.\n',
);
