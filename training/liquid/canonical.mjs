// Canonical JSON + content hashes for liquid-agent/v1. Byte-identical to Infernet's
// packages/liquid-agent/src/canonical.js (and its Python twin): keys sorted, no whitespace,
// numbers as JSON.stringify prints them, non-finite numbers rejected. Do not "improve" this;
// test/training-liquid.test.mjs pins it against the vendored fixtures.
import { createHash } from 'node:crypto';

export function canonicalize(value) {
  if (value === null) return 'null';
  switch (typeof value) {
    case 'boolean':
      return value ? 'true' : 'false';
    case 'number':
      if (!Number.isFinite(value)) throw new TypeError('canonical JSON: non-finite number');
      return JSON.stringify(value);
    case 'string':
      return JSON.stringify(value);
    case 'object': {
      if (Array.isArray(value)) return `[${value.map(canonicalize).join(',')}]`;
      const keys = Object.keys(value)
        .filter((k) => value[k] !== undefined)
        .sort();
      return `{${keys.map((k) => `${JSON.stringify(k)}:${canonicalize(value[k])}`).join(',')}}`;
    }
    default:
      throw new TypeError(`canonical JSON: unsupported type ${typeof value}`);
  }
}

export const sha256Hex = (data) => createHash('sha256').update(data).digest('hex');
export const hashCanonical = (value) => `sha256:${sha256Hex(canonicalize(value))}`;

export function hashWithout(value, fields) {
  const copy = { ...value };
  for (const f of fields) delete copy[f];
  return hashCanonical(copy);
}

export const episodeHash = (episode) => hashWithout(episode, ['content_hash']);
export const datasetHash = (manifest) =>
  hashCanonical({ episodes: manifest.episodes, splits: manifest.splits });
export const manifestHash = (manifest) => hashWithout(manifest, ['manifest_hash', 'signature']);
