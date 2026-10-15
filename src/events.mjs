// Product events: a few server-side milestones on the way from sign-up to a working fleet,
// one row each, so the funnel is a SQL count. No IP, user agent, email, hostname or other
// personal data goes in: the name is fixed, user_id is the internal account id (cleared
// if the account is deleted), and props holds only fixed labels.
//
//   SELECT name, COUNT(*), COUNT(DISTINCT user_id) FROM events
//     WHERE at >= '2026-10-15' GROUP BY name;
//
// signup_completed     the emailed link was used and the account created
// cli_login_completed  `w3bs login` got tokens (props.method: browser | device)
// fleet_first_report   the account's first machine report from `w3bs fleet agent`
//                      (the activation event; at most one per account)

const tables = (serial) => `
  CREATE TABLE IF NOT EXISTS events (sequence ${serial}, name TEXT NOT NULL, user_id TEXT REFERENCES users(id) ON DELETE SET NULL, props TEXT NOT NULL, at TEXT NOT NULL);
  CREATE INDEX IF NOT EXISTS events_name_at ON events(name, at);
  CREATE UNIQUE INDEX IF NOT EXISTS events_first_report ON events(user_id) WHERE name = 'fleet_first_report';`;
export const EVENT_SCHEMA = {
  sqlite: tables('INTEGER PRIMARY KEY AUTOINCREMENT'),
  postgres: tables('BIGSERIAL PRIMARY KEY'),
};
export const EVENTS = ['signup_completed', 'cli_login_completed', 'fleet_first_report'];

export function createEvents(db) {
  // Accounts already counted as activated, so a heartbeat every 30 seconds costs no query.
  const reported = new Set();
  // Never throws: losing a count must not break a sign-in or a heartbeat.
  async function record(name, userId = null, props = {}) {
    if (!EVENTS.includes(name)) throw new Error(`Unknown event ${name}`);
    try {
      await db.run(
        'INSERT INTO events (name, user_id, props, at) VALUES (?, ?, ?, ?) ON CONFLICT DO NOTHING',
        [name, userId, JSON.stringify(props), new Date().toISOString()],
      );
      return true;
    } catch (error) {
      process.stderr.write(`W3BS event ${name} not recorded: ${error.message}\n`);
      return false;
    }
  }
  async function firstReport(userId) {
    if (reported.has(userId)) return;
    if (await record('fleet_first_report', userId)) reported.add(userId);
  }
  const count = async (name) =>
    Number((await db.get('SELECT COUNT(*) AS n FROM events WHERE name = ?', [name])).n);
  return { record, firstReport, count };
}
