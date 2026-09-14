// Railway mounts volumes as root. If launched with RAILWAY_RUN_UID=0,
// initialize ownership of this service's data files, then drop privileges
// before importing the HTTP server or accepting requests.
import { chownSync, existsSync, lstatSync, mkdirSync } from 'node:fs';
import { resolve } from 'node:path';

if (process.getuid?.() === 0) {
  const directory = resolve(process.env.W3BS_DATA_DIR || '/data');
  mkdirSync(directory, { recursive: true });
  if (lstatSync(directory).isSymbolicLink())
    throw new Error('Data directory must not be a symlink.');
  chownSync(directory, 1000, 1000);
  for (const filename of ['registry.sqlite', 'registry.sqlite-wal', 'registry.sqlite-shm']) {
    const path = resolve(directory, filename);
    if (existsSync(path)) {
      if (!lstatSync(path).isFile()) throw new Error('Registry data must be regular files.');
      chownSync(path, 1000, 1000);
    }
  }
  process.setgroups([]);
  process.setgid(1000);
  process.setuid(1000);
}
const { startServer } = await import('./server.mjs');
const instance = await startServer();
process.stderr.write(
  `W3BS listening on port ${instance.server.address().port} as uid ${process.getuid?.() ?? 'n/a'}\n`,
);
for (const signal of ['SIGINT', 'SIGTERM'])
  process.once(signal, async () => {
    await instance.close();
    process.exit(0);
  });
