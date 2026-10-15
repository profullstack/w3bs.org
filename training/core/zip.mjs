// Minimal ZIP reader and writer for demonstration bundles.
//
// The reader is written for hostile input: it never touches the filesystem,
// rejects encryption, zip64, symlinks, unsafe names, duplicate names and
// declared-size lies, and bounds entry count, per-entry size, total size and
// compression ratio before inflating anything.
import { deflateRawSync, inflateRawSync } from 'node:zlib';
import { W3bsError } from '../../src/core.mjs';

const CRC_TABLE = new Uint32Array(256).map((_, n) => {
  let c = n;
  for (let k = 0; k < 8; k += 1) c = c & 1 ? 0xedb88320 ^ (c >>> 1) : c >>> 1;
  return c >>> 0;
});
export function crc32(buffer) {
  let crc = 0xffffffff;
  for (let i = 0; i < buffer.length; i += 1)
    crc = CRC_TABLE[(crc ^ buffer[i]) & 0xff] ^ (crc >>> 8);
  return (crc ^ 0xffffffff) >>> 0;
}

export const DEFAULT_LIMITS = {
  maxEntries: 20000,
  maxEntryBytes: 512 * 1024 * 1024,
  maxTotalBytes: 2 * 1024 * 1024 * 1024,
  maxRatio: 200,
};

const reject = (message) => new W3bsError('BUNDLE_REJECTED', message, 422);

export function safeEntryName(name) {
  return (
    name.length > 0 &&
    name.length <= 240 &&
    !name.startsWith('/') &&
    !name.includes('\\') &&
    !name.includes('\0') &&
    !/^[A-Za-z]:/.test(name) &&
    name.split('/').every((part) => part !== '..' && part !== '.')
  );
}

export function readZip(buffer, limits = {}) {
  const limit = { ...DEFAULT_LIMITS, ...limits };
  if (!Buffer.isBuffer(buffer)) buffer = Buffer.from(buffer);
  // The end-of-central-directory record sits in the last 22 + 65535 bytes.
  let eocd = -1;
  for (let i = buffer.length - 22; i >= Math.max(0, buffer.length - 65557); i -= 1) {
    if (buffer.readUInt32LE(i) === 0x06054b50) {
      eocd = i;
      break;
    }
  }
  if (eocd < 0) throw reject('Not a ZIP archive.');
  const count = buffer.readUInt16LE(eocd + 10);
  const cdSize = buffer.readUInt32LE(eocd + 12);
  const cdOffset = buffer.readUInt32LE(eocd + 16);
  if (count === 0xffff || cdOffset === 0xffffffff)
    throw reject('ZIP64 archives are not supported.');
  if (count > limit.maxEntries)
    throw reject(`Archive has ${count} entries; limit is ${limit.maxEntries}.`);
  if (cdOffset + cdSize > eocd) throw reject('Central directory is out of bounds.');
  const entries = [];
  const seen = new Set();
  let total = 0;
  let cursor = cdOffset;
  for (let n = 0; n < count; n += 1) {
    if (cursor + 46 > eocd || buffer.readUInt32LE(cursor) !== 0x02014b50)
      throw reject('Corrupt central directory.');
    const flags = buffer.readUInt16LE(cursor + 8);
    const method = buffer.readUInt16LE(cursor + 10);
    const crc = buffer.readUInt32LE(cursor + 16);
    const compressed = buffer.readUInt32LE(cursor + 20);
    const size = buffer.readUInt32LE(cursor + 24);
    const nameLength = buffer.readUInt16LE(cursor + 28);
    const extraLength = buffer.readUInt16LE(cursor + 30);
    const commentLength = buffer.readUInt16LE(cursor + 32);
    const madeBy = buffer.readUInt16LE(cursor + 4) >> 8;
    const external = buffer.readUInt32LE(cursor + 38);
    const offset = buffer.readUInt32LE(cursor + 42);
    const name = buffer.toString('utf8', cursor + 46, cursor + 46 + nameLength);
    cursor += 46 + nameLength + extraLength + commentLength;
    if (flags & 1) throw reject(`${name}: encrypted entries are not supported.`);
    if (compressed === 0xffffffff || size === 0xffffffff || offset === 0xffffffff)
      throw reject(`${name}: ZIP64 entries are not supported.`);
    if (name.endsWith('/')) continue; // directory entry
    if (!safeEntryName(name)) throw reject(`Unsafe entry name: ${JSON.stringify(name)}.`);
    if (seen.has(name)) throw reject(`Duplicate entry: ${name}.`);
    seen.add(name);
    const mode = external >>> 16;
    if (madeBy === 3 && (mode & 0o170000) === 0o120000)
      throw reject(`${name}: symbolic links are not allowed.`);
    if (madeBy === 3 && mode & 0o111) throw reject(`${name}: executable entries are not allowed.`);
    if (method !== 0 && method !== 8)
      throw reject(`${name}: compression method ${method} is not supported.`);
    if (size > limit.maxEntryBytes)
      throw reject(`${name}: ${size} bytes exceeds the per-entry limit.`);
    if (method === 8 && compressed > 0 && size / compressed > limit.maxRatio)
      throw reject(`${name}: compression ratio exceeds ${limit.maxRatio}:1.`);
    total += size;
    if (total > limit.maxTotalBytes)
      throw reject('Archive exceeds the total uncompressed size limit.');
    entries.push({ name, method, crc, compressed, size, offset });
  }
  const files = new Map();
  for (const entry of entries) {
    const at = entry.offset;
    if (at + 30 > buffer.length || buffer.readUInt32LE(at) !== 0x04034b50)
      throw reject(`${entry.name}: corrupt local header.`);
    const start = at + 30 + buffer.readUInt16LE(at + 26) + buffer.readUInt16LE(at + 28);
    const end = start + entry.compressed;
    if (end > cdOffset) throw reject(`${entry.name}: data is out of bounds.`);
    const raw = buffer.subarray(start, end);
    let data;
    if (entry.method === 0) {
      if (entry.compressed !== entry.size) throw reject(`${entry.name}: size mismatch.`);
      data = Buffer.from(raw);
    } else {
      try {
        // maxOutputLength stops a lying size field from inflating unbounded.
        data = inflateRawSync(raw, { maxOutputLength: Math.max(1, entry.size) });
      } catch {
        throw reject(`${entry.name}: inflated beyond its declared size or is corrupt.`);
      }
    }
    if (data.length !== entry.size) throw reject(`${entry.name}: declared size does not match.`);
    if (crc32(data) !== entry.crc) throw reject(`${entry.name}: CRC mismatch.`);
    files.set(entry.name, data);
  }
  return files;
}

// files: Map or array of [name, Buffer|string]. Timestamps are fixed so the
// same content always produces the same archive.
export function writeZip(files) {
  const locals = [];
  const central = [];
  let offset = 0;
  for (const [name, content] of files) {
    if (!safeEntryName(name)) throw new W3bsError('INVALID_PATH', `Unsafe entry name ${name}.`);
    const data = Buffer.isBuffer(content) ? content : Buffer.from(content);
    const deflated = deflateRawSync(data);
    const useDeflate = deflated.length < data.length;
    const body = useDeflate ? deflated : data;
    const nameBuffer = Buffer.from(name, 'utf8');
    const crc = crc32(data);
    const header = Buffer.alloc(30);
    header.writeUInt32LE(0x04034b50, 0);
    header.writeUInt16LE(20, 4);
    header.writeUInt16LE(0x0800, 6); // UTF-8 names
    header.writeUInt16LE(useDeflate ? 8 : 0, 8);
    header.writeUInt16LE(0, 10);
    header.writeUInt16LE(0x21, 12); // 1980-01-01
    header.writeUInt32LE(crc, 14);
    header.writeUInt32LE(body.length, 18);
    header.writeUInt32LE(data.length, 22);
    header.writeUInt16LE(nameBuffer.length, 26);
    locals.push(header, nameBuffer, body);
    const entry = Buffer.alloc(46);
    entry.writeUInt32LE(0x02014b50, 0);
    entry.writeUInt16LE((3 << 8) | 20, 4);
    entry.writeUInt16LE(20, 6);
    entry.writeUInt16LE(0x0800, 8);
    entry.writeUInt16LE(useDeflate ? 8 : 0, 10);
    entry.writeUInt16LE(0, 12);
    entry.writeUInt16LE(0x21, 14);
    entry.writeUInt32LE(crc, 16);
    entry.writeUInt32LE(body.length, 20);
    entry.writeUInt32LE(data.length, 24);
    entry.writeUInt16LE(nameBuffer.length, 28);
    entry.writeUInt32LE((0o100644 << 16) >>> 0, 38);
    entry.writeUInt32LE(offset, 42);
    central.push(entry, nameBuffer);
    offset += header.length + nameBuffer.length + body.length;
  }
  const cd = Buffer.concat(central);
  const end = Buffer.alloc(22);
  end.writeUInt32LE(0x06054b50, 0);
  const count = central.length / 2;
  end.writeUInt16LE(count, 8);
  end.writeUInt16LE(count, 10);
  end.writeUInt32LE(cd.length, 12);
  end.writeUInt32LE(offset, 16);
  return Buffer.concat([...locals, cd, end]);
}
