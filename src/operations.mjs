import { timingSafeEqual } from 'node:crypto';
import { z } from 'zod';
import { W3bsError, TYPES, renderPrompt } from './core.mjs';

const uri = z.string().min(1).max(512);
export const operations = {
  search: {
    description: 'Search the registry for versioned W3BS resources.',
    schema: z.strictObject({
      query: z.string().max(200).default(''),
      type: z.enum(TYPES).optional(),
    }),
  },
  resolve: {
    description: 'Resolve a W3BS URI to its canonical manifest and verification state.',
    schema: z.strictObject({ uri }),
  },
  inspect: {
    description: 'Inspect identity, provenance, permissions and the canonical artifact.',
    schema: z.strictObject({ uri }),
  },
  verify: {
    description:
      'Verify the manifest signature against pinned publisher keys and current revocation state. This does not authorize execution.',
    schema: z.strictObject({ uri }),
  },
  publish: {
    description:
      'Publish an immutable, signed manifest. Requires an authenticated registry publisher.',
    schema: z.strictObject({ manifest: z.record(z.string(), z.unknown()) }),
    write: true,
  },
  run: {
    description:
      'Run the deterministic template runtime with explicit consent. Produces rendered instructions; never calls a model or tool.',
    schema: z.strictObject({
      uri,
      inputs: z.record(z.string(), z.string().max(50000)).default({}),
      consent: z.boolean().default(false),
    }),
  },
  revoke: {
    description:
      'Revoke an exact resource version, retaining its artifact for inspection. Requires registry publisher authorization.',
    schema: z.strictObject({ uri, reason: z.string().min(3).max(1000) }),
    write: true,
  },
  conformance: {
    description:
      'Fetch protocol fixture checks for a resource. Cross-surface interoperability is tested by the separate conformance suite.',
    schema: z.strictObject({ uri }),
  },
};
export function authorize(token, expected = process.env.W3BS_PUBLISH_TOKEN) {
  if (!expected || expected.length < 32)
    throw new W3bsError(
      'PUBLISHING_DISABLED',
      'Publishing is disabled until the operator configures a publisher token of at least 32 characters.',
      503,
    );
  const actual = Buffer.from(token || ''),
    target = Buffer.from(expected);
  if (actual.length !== target.length || !timingSafeEqual(actual, target))
    throw new W3bsError('UNAUTHORIZED', 'A valid registry publisher token is required.', 401);
}
export async function dispatch(store, operation, args, { token, publishToken } = {}) {
  const definition = operations[operation];
  if (!definition) throw new W3bsError('UNKNOWN_OPERATION', 'Unknown W3BS operation.', 404);
  const parsed = definition.schema.safeParse(args);
  if (!parsed.success)
    throw new W3bsError(
      'INVALID_ARGUMENTS',
      parsed.error.issues.map((item) => `${item.path.join('.')}: ${item.message}`).join('; '),
    );
  if (definition.write) authorize(token, publishToken);
  const input = parsed.data;
  if (operation === 'search') return { resources: await store.search(input.query, input.type) };
  if (operation === 'publish') return store.publish(input.manifest);
  if (operation === 'revoke') return store.revoke(input.uri, input.reason);
  const result = await store.inspect(input.uri);
  if (operation === 'verify')
    return { canonicalUri: result.canonicalUri, verification: result.verification };
  if (operation === 'run') {
    if (!result.verification.valid)
      throw new W3bsError(
        'VERIFICATION_FAILED',
        'Resource is revoked, untrusted or has an invalid signature.',
        403,
      );
    return {
      ...renderPrompt(result.manifest, input.inputs, input.consent),
      verification: result.verification,
    };
  }
  if (operation === 'conformance')
    return {
      canonicalUri: result.canonicalUri,
      profile: 'W3BS-reference-draft-1',
      checks: {
        identity: result.manifest.id === result.canonicalUri,
        fullVersion: result.manifest.id.endsWith(`@${result.manifest.version}`),
        signature: result.verification.signatureValid,
        publisher: result.verification.publisherTrusted,
        notRevoked: !result.verification.revoked,
        dataByDefault: result.verification.authority === 'DATA',
      },
      crossSurfaceEvidence:
        'Run npm run conformance. These checks alone do not establish interoperability.',
    };
  return result;
}
