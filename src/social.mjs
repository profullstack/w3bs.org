// W3BS social: the AT Protocol PDS at social.w3bs.org (src/social-routes.mjs). The PDS
// keeps its own accounts; this side only remembers the one invite code each W3BS
// account was given, so a refresh shows the same code instead of minting another.

const tables = () => `
  CREATE TABLE IF NOT EXISTS social_invites (user_id TEXT PRIMARY KEY REFERENCES users(id) ON DELETE CASCADE, code TEXT NOT NULL, created_at TEXT NOT NULL);`;
export const SOCIAL_SCHEMA = { sqlite: tables(), postgres: tables() };

export function createSocial(db) {
  return {
    async invite(userId) {
      const row = await db.get('SELECT code, created_at FROM social_invites WHERE user_id = ?', [
        userId,
      ]);
      return row ? { code: row.code, createdAt: row.created_at } : null;
    },
    // Two clicks racing both mint a code; the first one stored wins and is returned.
    async saveInvite(userId, code) {
      await db.run('INSERT INTO social_invites VALUES (?, ?, ?) ON CONFLICT (user_id) DO NOTHING', [
        userId,
        code,
        new Date().toISOString(),
      ]);
      return this.invite(userId);
    },
  };
}
