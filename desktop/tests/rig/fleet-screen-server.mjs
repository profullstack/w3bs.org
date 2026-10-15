// fleet-screen.sh helper: a local w3bs.org on 127.0.0.1:$PORT (SQLite in $DB) with one
// signed-in user. Writes {cookie, token, origin} to $OUT, then keeps serving.
import { writeFileSync } from 'node:fs';
import { startServer } from '../../../src/server.mjs';
import { openStore } from '../../../src/store.mjs';

const port = Number(process.env.PORT || 3377);
const origin = `http://localhost:${port}`;
const store = await openStore({ path: process.env.DB });
const instance = await startServer({
  store,
  port,
  host: '127.0.0.1',
  publicOrigin: origin,
  allowedHosts: ['localhost'],
  mailer: { send: async () => ({ sent: true }) },
});
const accounts = store.accounts(origin);
const signup = await accounts.startSignup({ username: 'rig', email: 'rig@example.com' });
const { user } = await accounts.consumeLink(signup.link);
const cookie = await accounts.createSession(user.id);
const { token } = await store.fleet.createToken(user.id, { name: 'kde-rig', allowActions: false });
writeFileSync(process.env.OUT, JSON.stringify({ origin, cookie, token }));
process.stderr.write(`rig server on ${origin}\n`);
for (const signal of ['SIGINT', 'SIGTERM'])
  process.once(signal, async () => {
    await instance.close();
    process.exit(0);
  });
