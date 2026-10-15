// Import-time privacy policy. Runs before anything is persisted, indexed or
// sent anywhere. Secret detection is defence in depth: it cannot recognise
// every secret, which is why flagged and unflagged episodes alike need human
// review before admission.

const SECRET_PATTERNS = [
  ['private_key', /-----BEGIN [A-Z ]*PRIVATE KEY-----/],
  ['aws_access_key', /\b(AKIA|ASIA)[0-9A-Z]{16}\b/],
  ['github_token', /\b(gh[pousr]_[A-Za-z0-9]{30,}|github_pat_[A-Za-z0-9_]{40,})\b/],
  ['api_key', /\b(sk|rk|pk)[-_](live|test|proj|ant)?[-_]?[A-Za-z0-9_-]{20,}\b/],
  ['slack_token', /\bxox[abprs]-[A-Za-z0-9-]{10,}\b/],
  ['jwt', /\beyJ[A-Za-z0-9_-]{8,}\.[A-Za-z0-9_-]{8,}\.[A-Za-z0-9_-]{8,}\b/],
  ['bearer', /\bBearer\s+[A-Za-z0-9._~+/-]{16,}=*/i],
  ['password_assignment', /\b(pass(word)?|passwd|pwd|secret|token)\s*[:=]\s*\S{4,}/i],
  ['seed_phrase', /\b(?:[a-z]{3,8}\s+){11}[a-z]{3,8}\b/],
];

// Contexts blocked by default: credential flows and password managers.
const SENSITIVE_CONTEXT = [
  /\b1password\b/i,
  /\bbitwarden\b/i,
  /\bkeepass(xc)?\b/i,
  /\blastpass\b/i,
  /\bdashlane\b/i,
  /\bpassword manager\b/i,
  /\b(sign[ -]?in|log[ -]?in|password|two[- ]factor|2fa|one[- ]time code|verification code)\b/i,
  /\bprivate (browsing|window)\b|\bincognito\b/i,
];

function luhn(digits) {
  let sum = 0;
  for (let i = 0; i < digits.length; i += 1) {
    let d = Number(digits[digits.length - 1 - i]);
    if (i % 2 === 1) {
      d *= 2;
      if (d > 9) d -= 9;
    }
    sum += d;
  }
  return sum % 10 === 0;
}

export function detectSecrets(value) {
  const found = [];
  for (const [kind, pattern] of SECRET_PATTERNS) if (pattern.test(value)) found.push(kind);
  for (const match of value.matchAll(/\b(?:\d[ -]?){13,19}\b/g)) {
    const digits = match[0].replace(/\D/g, '');
    if (digits.length >= 13 && luhn(digits)) found.push('payment_card');
  }
  return found;
}

export function isSensitiveContext(value) {
  return SENSITIVE_CONTEXT.some((pattern) => pattern.test(value));
}

// Keep scheme, host and path; drop query and fragment, where tokens live.
export function stripUrl(value) {
  try {
    const url = new URL(value);
    return `${url.protocol}//${url.host}${url.pathname}`;
  } catch {
    return '[unparseable-url]';
  }
}

// Walks a record and redacts in place. `path` names the location for the
// warning. Returns a list of { kind, path } findings.
export function scrubRecord(record, path, { allowText = false } = {}) {
  const findings = [];
  const visit = (node, at) => {
    if (!node || typeof node !== 'object') return;
    for (const key of Object.keys(node)) {
      const value = node[key];
      const here = `${at}.${key}`;
      if (typeof value === 'string') {
        if (key === 'text' && !allowText) {
          delete node[key];
          node.text_placeholder ??= '[text omitted by capture policy]';
          findings.push({ kind: 'text_value_removed', path: here, blocking: false });
          continue;
        }
        if (key === 'url' || key === 'url_origin') {
          const stripped = stripUrl(value);
          if (stripped !== value) {
            node[key] = stripped;
            findings.push({ kind: 'url_query_removed', path: here, blocking: false });
          }
        }
        const secrets = detectSecrets(node[key]);
        if (secrets.length) {
          node[key] = `[redacted:${secrets[0]}]`;
          for (const kind of secrets)
            findings.push({ kind: `secret_${kind}`, path: here, blocking: true });
          continue;
        }
        if (
          ['window', 'name', 'app', 'url', 'url_origin'].includes(key) &&
          isSensitiveContext(node[key])
        )
          findings.push({ kind: 'sensitive_context', path: here, blocking: true });
      } else if (typeof value === 'object') visit(value, here);
    }
  };
  visit(record, path);
  return findings;
}
