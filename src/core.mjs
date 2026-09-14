import { createHash, createPublicKey, sign, verify } from 'node:crypto';
import canonicalize from 'canonicalize';
import { z } from 'zod';

export const TYPES = [
  'prompt',
  'behavior',
  'agent',
  'tool',
  'mcp',
  'schema',
  'eval',
  'identity',
  'service',
  'device',
  'stream',
  'swarm',
];
export const CLASSES = [
  'DATA',
  'CONTENT',
  'USER_INSTRUCTION',
  'TOOL_OUTPUT',
  'AGENT_INSTRUCTION',
  'SYSTEM_POLICY',
];
const segment = '[a-z0-9]+(?:-[a-z0-9]+)*';
const integer = '(?:0|[1-9][0-9]{0,8})';
const version = `${integer}\\.${integer}\\.${integer}`;
const uriPattern = new RegExp(
  `^w3bs://(${TYPES.join('|')})/(${segment})/(${segment})(?:@(${integer}(?:\\.${integer}\\.${integer})?))?$`,
);
export class W3bsError extends Error {
  constructor(code, message, status = 400) {
    super(message);
    this.code = code;
    this.status = status;
  }
}
export function parseUri(value) {
  const match = typeof value === 'string' && value.length <= 512 && uriPattern.exec(value);
  if (!match)
    throw new W3bsError(
      'INVALID_URI',
      'Expected w3bs://type/namespace/name with an optional @major or @major.minor.patch. Use lowercase ASCII; queries, fragments and escapes are not allowed.',
    );
  const [, type, namespace, name, requestedVersion] = match;
  return {
    type,
    namespace,
    name,
    version: requestedVersion ?? null,
    base: `w3bs://${type}/${namespace}/${name}`,
  };
}
const text = z.string().min(1).max(5000);
const inputSchema = z.strictObject({ description: text, required: z.boolean() });
const httpsUrl = z
  .url()
  .refine((value) => new URL(value).protocol === 'https:', 'Expected HTTPS URL');
export const ManifestSchema = z.strictObject({
  specVersion: z.literal('W3BS-MANIFEST-1-draft-1'),
  id: z.string().max(512),
  type: z.enum(TYPES),
  version: z.string().regex(new RegExp(`^${version}$`)),
  name: z.string().min(1).max(120),
  description: z.string().min(1).max(1000),
  publisher: z.strictObject({
    namespace: z.string().regex(new RegExp(`^${segment}$`)),
    name: text,
    keyId: text,
  }),
  license: z.string().min(1).max(100),
  createdAt: z.iso.datetime(),
  provenance: z.strictObject({
    source: httpsUrl,
    derivedFrom: z.array(z.string().max(512)).max(50),
  }),
  trust: z.strictObject({ classification: z.enum(CLASSES) }),
  permissions: z.array(z.string().min(1).max(200)).max(50),
  dependencies: z.array(z.string().max(512)).max(50),
  locations: z.strictObject({ canonical: httpsUrl, mirrors: z.array(httpsUrl).max(20) }),
  content: z.strictObject({
    mediaType: z.literal('text/plain'),
    instructions: z.string().min(1).max(50000),
    inputs: z.record(z.string().regex(/^[a-z][a-z0-9_]{0,63}$/), inputSchema),
    output: z.strictObject({ mediaType: z.literal('text/plain'), description: text }),
    runtime: z.literal('w3bs-template-v1'),
    compatibleModels: z.array(z.string().max(120)).max(50),
    tools: z.array(z.string().max(512)).max(50),
    mcpServers: z.array(z.string().max(512)).max(50),
    evaluations: z.array(z.strictObject({ id: text, description: text })).max(50),
  }),
  proof: z
    .strictObject({
      type: z.literal('Ed25519'),
      keyId: text,
      digest: z.string().regex(/^sha256:[a-f0-9]{64}$/),
      signature: z.string().regex(/^[A-Za-z0-9_-]{86}$/),
    })
    .optional(),
});
export const manifestJsonSchema = z.toJSONSchema(ManifestSchema);
export function validateManifest(input, requireProof = true) {
  const parsed = ManifestSchema.safeParse(input);
  if (!parsed.success)
    throw new W3bsError(
      'INVALID_MANIFEST',
      parsed.error.issues.map((issue) => `${issue.path.join('.')}: ${issue.message}`).join('; '),
    );
  const manifest = parsed.data;
  const uri = parseUri(manifest.id);
  if (
    uri.version !== manifest.version ||
    uri.type !== manifest.type ||
    uri.namespace !== manifest.publisher.namespace
  )
    throw new W3bsError(
      'IDENTITY_MISMATCH',
      'Manifest id, full version, type and publisher namespace must agree.',
    );
  if (requireProof && !manifest.proof)
    throw new W3bsError('SIGNATURE_REQUIRED', 'A signed manifest is required.');
  if (manifest.proof && manifest.proof.keyId !== manifest.publisher.keyId)
    throw new W3bsError('KEY_MISMATCH', 'Proof and publisher key IDs must agree.');
  for (const id of [
    ...manifest.dependencies,
    ...manifest.provenance.derivedFrom,
    ...manifest.content.tools,
    ...manifest.content.mcpServers,
  ]) {
    if (!parseUri(id).version?.includes('.'))
      throw new W3bsError(
        'UNPINNED_DEPENDENCY',
        'Dependencies and provenance references must pin full versions.',
      );
  }
  const variables = [...manifest.content.instructions.matchAll(/\{\{([a-z][a-z0-9_]*)\}\}/g)].map(
    (match) => match[1],
  );
  for (const key of variables)
    if (!Object.hasOwn(manifest.content.inputs, key))
      throw new W3bsError('UNDECLARED_INPUT', `Template input ${key} is not declared.`);
  return manifest;
}
export function signedBytes(manifest) {
  const { proof: _proof, ...payload } = manifest;
  return Buffer.from(canonicalize(payload), 'utf8');
}
export function digest(manifest) {
  return `sha256:${createHash('sha256').update(signedBytes(manifest)).digest('hex')}`;
}
export function signManifest(input, privateKey) {
  const manifest = validateManifest(input, false);
  return {
    ...manifest,
    proof: {
      type: 'Ed25519',
      keyId: manifest.publisher.keyId,
      digest: digest(manifest),
      signature: sign(null, signedBytes(manifest), privateKey).toString('base64url'),
    },
  };
}
export function verifyManifest(input, trustStore, revoked = false) {
  const manifest = validateManifest(input);
  const key = trustStore.keys.find((entry) => entry.id === manifest.proof.keyId);
  let signatureValid = false;
  try {
    if (key) {
      const publicKey = createPublicKey(key.publicKeyPem);
      signatureValid =
        publicKey.asymmetricKeyType === 'ed25519' &&
        digest(manifest) === manifest.proof.digest &&
        verify(
          null,
          signedBytes(manifest),
          publicKey,
          Buffer.from(manifest.proof.signature, 'base64url'),
        );
    }
  } catch {
    /* Unusable keys fail closed. */
  }
  const publisherTrusted = Boolean(
    key && key.namespace === manifest.publisher.namespace && !key.revoked,
  );
  return {
    valid: signatureValid && publisherTrusted && !revoked,
    signatureValid,
    publisherTrusted,
    revoked: Boolean(revoked || key?.revoked),
    digest: digest(manifest),
    keyId: manifest.proof.keyId,
    authority: 'DATA',
  };
}
export function compareVersions(a, b) {
  const left = a.split('.').map(Number),
    right = b.split('.').map(Number);
  for (let i = 0; i < 3; i++) if (left[i] !== right[i]) return left[i] - right[i];
  return 0;
}
export function renderPrompt(manifest, inputs = {}, consent = false) {
  if (!consent)
    throw new W3bsError(
      'CONSENT_REQUIRED',
      'Explicit consent is required to render this instruction artifact.',
      403,
    );
  if (!['prompt', 'behavior'].includes(manifest.type))
    throw new W3bsError(
      'UNSUPPORTED_RUNTIME',
      'Only prompt and behavior resources support the template runtime.',
      422,
    );
  if (
    manifest.permissions.length ||
    manifest.dependencies.length ||
    manifest.content.tools.length ||
    manifest.content.mcpServers.length
  )
    throw new W3bsError(
      'UNSUPPORTED_CAPABILITY',
      'The template runtime cannot grant permissions, resolve executable dependencies or call tools.',
      422,
    );
  if (!inputs || typeof inputs !== 'object' || Array.isArray(inputs))
    throw new W3bsError('INVALID_INPUT', 'Inputs must be an object.');
  for (const [key, value] of Object.entries(inputs)) {
    if (
      !Object.hasOwn(manifest.content.inputs, key) ||
      typeof value !== 'string' ||
      value.length > 50000
    )
      throw new W3bsError('INVALID_INPUT', `Unknown, non-string or oversized input: ${key}`);
  }
  for (const [key, field] of Object.entries(manifest.content.inputs)) {
    if (field.required && (!Object.hasOwn(inputs, key) || !inputs[key].trim()))
      throw new W3bsError('MISSING_INPUT', `Required input: ${key}`);
  }
  // Callback replacement inserts data once; nested template text in an input is never evaluated.
  const output = manifest.content.instructions.replace(
    /\{\{([a-z][a-z0-9_]*)\}\}/g,
    (_match, key) => inputs[key] ?? '',
  );
  return {
    id: manifest.id,
    version: manifest.version,
    runtime: 'w3bs-template-v1',
    output,
    mediaType: 'text/plain',
    modelInvoked: false,
    toolsInvoked: [],
    instructionAuthority: 'USER_INSTRUCTION',
    inputAuthority: 'DATA',
  };
}
