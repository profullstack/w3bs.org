import { isIP } from 'node:net';
import { randomUUID } from 'node:crypto';
import { W3bsError } from './core.mjs';

export const RECORD_TYPES = ['AAAA', 'CNAME', 'MX', 'TXT'];
const host = (value) => {
  const text = String(value ?? '').trim().toLowerCase().replace(/\.$/, '');
  return text.length <= 253 && text.split('.').length >= 2 && text.split('.').every(l => /^[a-z0-9](?:[a-z0-9-]{0,61}[a-z0-9])?$/.test(l)) && !isIP(text) ? text : null;
};
export function normalizeNetRecord(input, name) {
  const type = String(input?.type ?? '').toUpperCase();
  if (!RECORD_TYPES.includes(type)) throw new W3bsError('INVALID_RECORD', 'Choose AAAA, CNAME, MX or TXT.', 400);
  let value = String(input?.value ?? '').trim();
  if (!value || Buffer.byteLength(value) > 1024) throw new W3bsError('INVALID_RECORD', 'Enter a record value of at most 1024 bytes.', 400);
  const ttl = Number(input.ttl ?? 300);
  if (!Number.isInteger(ttl) || ttl < 60 || ttl > 86400) throw new W3bsError('INVALID_TTL', 'TTL must be 60–86400 seconds.', 400);
  let priority = 0;
  if (type === 'AAAA') {
    value = value.replace(/^\[|\]$/g, '').toLowerCase();
    if (isIP(value) !== 6 || value.includes('%')) throw new W3bsError('INVALID_RECORD', 'AAAA needs an IPv6 address. Use CNAME for a hostname.', 400);
    value = new URL(`http://[${value}]/`).hostname.slice(1,-1);
    if (/^(?:::(?:1)?$|::ffff:|f[cd]|fe[89ab]|ff)/i.test(value)) throw new W3bsError('INVALID_RECORD', 'Use a public IPv6 address, not a local, mapped or multicast address.', 400);
  } else if (type === 'CNAME' || type === 'MX') {
    value = host(value);
    if (!value) throw new W3bsError('INVALID_RECORD', `${type} needs a hostname, without https://, a path or a port.`, 400);
    if (type === 'CNAME' && [name, `${name}.w3bs.org`].includes(value)) throw new W3bsError('INVALID_RECORD', 'A CNAME cannot point at itself.', 400);
    if (type === 'MX') {
      priority = Number(input.priority ?? 10);
      if (!Number.isInteger(priority) || priority < 0 || priority > 65535) throw new W3bsError('INVALID_PRIORITY', 'MX priority must be 0–65535.', 400);
    }
  }
  return { type, value, ttl, priority };
}

export function createNetRecords(db) {
  let writes = Promise.resolve();
  const transaction = (fn) => {
    const next = writes.then(() => db.transaction(fn));
    writes = next.catch(() => {});
    return next;
  };
  const parts = (name) => {
    const p = String(name).toLowerCase().split('.');
    if (p.length !== 2 || !p.every(l=>/^[a-z0-9](?:[a-z0-9-]{0,61}[a-z0-9])?$/.test(l))) throw new W3bsError('INVALID_NAME','Use label.namespace.',400);
    return [p[1],p[0]];
  };
  const rows = async (tx, tld, label) => (await tx.all('SELECT id,type,value,ttl,priority FROM net_dns_records WHERE tld=? AND label=? ORDER BY type,value,priority',[tld,label]));
  async function authorize(tx, name, ownerId) {
    const [tld,label] = parts(name);
    const row = await tx.get('SELECT n.owner_id,n.status,t.status AS namespace_status FROM net_names n JOIN net_tlds t ON t.tld=n.tld WHERE n.tld=? AND n.label=?' + (db.dialect === 'postgres' ? ' FOR UPDATE OF n' : ''),[tld,label]);
    if (!row || row.owner_id !== ownerId) throw new W3bsError('NAME_OWNER_REQUIRED','Only the W3BS owner of this name can change its DNS records.',403);
    if (row.status !== 'active' || row.namespace_status !== 'active') throw new W3bsError('NAME_NOT_ACTIVE','The name and namespace must be active before publishing DNS records.',409);
    return [tld,label];
  }
  return {
    async owned(ownerId) {
      return db.all('SELECT n.label,n.tld,n.status,n.target,t.status AS namespace_status FROM net_names n JOIN net_tlds t ON t.tld=n.tld WHERE n.owner_id=? ORDER BY n.tld,n.label',[ownerId]);
    },
    async list(name) {
      const [tld,label] = parts(name);
      const active = await db.get("SELECT n.label FROM net_names n JOIN net_tlds t ON t.tld=n.tld WHERE n.tld=? AND n.label=? AND n.status='active' AND t.status='active'",[tld,label]);
      return active ? rows(db,tld,label) : [];
    },
    async put(name, ownerId, input) {
      const record = normalizeNetRecord(input,name);
      return transaction(async tx => {
        const [tld,label] = await authorize(tx,name,ownerId);
        const existing = await rows(tx,tld,label);
        if (existing.some(r=>r.type==='CNAME' && (record.type!=='CNAME' || r.value!==record.value)) || (record.type==='CNAME' && existing.some(r=>r.type!=='CNAME')))
          throw new W3bsError('CNAME_CONFLICT','CNAME must be the only record on a name. Remove the existing records first.',409);
        if (existing.length >= 50 && !existing.some(r=>r.type===record.type && r.value===record.value && r.priority===record.priority)) throw new W3bsError('RECORD_LIMIT','A name can have at most 50 records.',409);
        await tx.run('INSERT INTO net_dns_records(id,tld,label,type,value,ttl,priority,created_at) VALUES(?,?,?,?,?,?,?,?) ON CONFLICT(tld,label,type,value,priority) DO UPDATE SET ttl=excluded.ttl',[randomUUID(),tld,label,record.type,record.value,record.ttl,record.priority,new Date().toISOString()]);
        return rows(tx,tld,label);
      });
    },
    async remove(name, ownerId, id) {
      return transaction(async tx => {
        const [tld,label] = await authorize(tx,name,ownerId);
        await tx.run('DELETE FROM net_dns_records WHERE id=? AND tld=? AND label=?',[String(id),tld,label]);
        return rows(tx,tld,label);
      });
    },
  };
}
