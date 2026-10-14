// Account email (signup and sign-in links) through Resend, by way of the house
// emailer (@profullstack/stack/email). Without RESEND_API_KEY, development prints the
// message to stderr so a link can be followed locally; production refuses to pretend.
import { createEmailer } from '@profullstack/stack/email';

export function createMailer({
  apiKey = process.env.RESEND_API_KEY,
  from = process.env.W3BS_MAIL_FROM || 'W3BS <noreply@w3bs.org>',
  production = process.env.NODE_ENV === 'production',
} = {}) {
  if (!apiKey)
    return {
      async send({ to, subject, text }) {
        if (production) {
          process.stderr.write(`W3BS mail: RESEND_API_KEY is not set; not sending "${subject}"\n`);
          return { sent: false, error: 'not configured' };
        }
        process.stderr.write(`W3BS mail (dev, not sent) to ${to}: ${subject}\n${text}\n`);
        return { sent: true };
      },
    };
  const emailer = createEmailer({ resendApiKey: apiKey });
  return {
    async send(message) {
      const result = await emailer.send({ from, ...message });
      if (!result.sent)
        process.stderr.write(`W3BS mail to ${message.to} failed: ${result.error || 'unknown'}\n`);
      return result;
    },
  };
}
