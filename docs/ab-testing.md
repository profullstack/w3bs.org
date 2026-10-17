# W3BS A/B testing

Hosted A/B testing at [w3bs.org/ab-testing](https://w3bs.org/ab-testing), built on the open
[OpenABTest 0.2 draft](https://logicsrc.com/docs/openabtest). You define an experiment, your
server asks w3bs.org which variant each participant gets, and reports what they saw and did.
Results count unique participants per variant. Nothing names a winner for you.

Status: community draft. Free for up to 50 experiments per account, 8 variants each, 10 server
keys.

## The model

| Thing      | What it is                                                                                                                                                                                                                                      |
| ---------- | ----------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| Experiment | An id (`checkout-button`), a name, a randomization unit (user, account, session, device or custom), two or more variants with integer weights and JSON parameters, a window and a minimum sample per variant. The first variant is the control. |
| Assignment | The first `assign` call for a participant picks a variant and stores it. Every later call returns the stored one, on any server, forever.                                                                                                       |
| Exposure   | You showed the participant their variant, on a named surface.                                                                                                                                                                                   |
| Conversion | The goal happened. Once per participant unless you send distinct `conversionId`s. Optionally carries a value (`valueMinor` + `currency`) or a full OpenABTest `price` for purchases.                                                            |

States: `draft` (editable, enrolls nobody), `running`, `paused` (no new participants; existing
assignments are still returned and events still recorded), `closed` (terminal). Starting fixes
the window. A started experiment cannot be edited: to test something else, make a new one. This
is the spec's rule that a revision's variants, weights and metrics are immutable.

## From your server

Make a key on the experiments page and send it as a bearer token. Keys can only call the three
producer endpoints below, for your experiments.

```sh
# Which variant? (sticky)
curl -s https://w3bs.org/api/ab/assign \
  -H "Authorization: Bearer $W3BS_AB_KEY" -H 'Content-Type: application/json' \
  -d '{"experiment":"checkout-button","participant":"user-1842"}'
```

```json
{
  "experiment": "checkout-button",
  "revision": 1,
  "state": "running",
  "participant": "user-1842",
  "assignmentId": "a_9q0...",
  "variant": "green",
  "parameters": { "color": "green" },
  "new": true
}
```

When the experiment is not running (or outside its window) and the participant has no
assignment yet, `variant` and `parameters` are `null` and `reason` says why. Show your default
experience.

```sh
# You showed it
curl -s https://w3bs.org/api/ab/exposure -H "Authorization: Bearer $W3BS_AB_KEY" \
  -H 'Content-Type: application/json' \
  -d '{"experiment":"checkout-button","participant":"user-1842","surface":"checkout"}'

# The goal happened
curl -s https://w3bs.org/api/ab/conversion -H "Authorization: Bearer $W3BS_AB_KEY" \
  -H 'Content-Type: application/json' \
  -d '{"experiment":"checkout-button","participant":"user-1842","valueMinor":4900,"currency":"USD"}'
```

Optional fields: `variant` (rejected with 409 if it is not the stored one), `id` (your event id;
an identical retry is a no-op, the same id with other content is a 409), `at` (RFC 3339, when it
happened), `offerId` on exposures, `conversionId` and `price` on conversions. Exposures and
conversions for a participant without an assignment are refused with 409 `NOT_ASSIGNED`.

Participant ids are opaque: 1-128 letters, digits, `.`, `_`, `:` or `-`. Send an internal id or
a hash. Emails are refused.

### Node

```js
const ab = (path, body) =>
  fetch(`https://w3bs.org/api/ab/${path}`, {
    method: 'POST',
    headers: {
      Authorization: `Bearer ${process.env.W3BS_AB_KEY}`,
      'Content-Type': 'application/json',
    },
    body: JSON.stringify({ experiment: 'checkout-button', ...body }),
  }).then((r) => r.json());

const { variant, parameters } = await ab('assign', { participant: user.id });
const color = parameters?.color ?? 'blue'; // null when not enrolled: the default
// ...render, then:
await ab('exposure', { participant: user.id, surface: 'checkout' });
// ...on purchase:
await ab('conversion', {
  participant: user.id,
  conversionId: order.id,
  valueMinor: order.total,
  currency: 'USD',
});
```

Assignment is a server decision by design: do not call these endpoints from a browser with your
key.

## How a variant is picked

`hmac-sha256-persisted-v1`: HMAC-SHA256 under a secret generated for the experiment (never
shown), over `<experiment id>\n<revision>\n<participant id>`. The first 6 bytes, read as an
unsigned integer, modulo the total weight, walk the variants in order. The first result is
stored with a uniqueness constraint on (experiment, participant); what is stored is what counts.

## Results

`GET /api/ab/experiments/<id>` (or the experiment page, or `w3bs ab show <id>`) returns, per
variant: assigned, exposed, converted (unique participants), conversions converted without a
recorded exposure, value totals per currency, the conversion rate over assigned participants
with a 95% Wilson interval, and the difference from the control with a pooled two-proportion
z-test p-value.

- The denominator is every assigned participant, including those who never saw the variant.
- Zero participants is "no data", not 0%.
- The floors (minimum participants per variant and minimum days running; default 100 and 7) say
  when it is reasonable to look. Meeting them is not statistical significance, and the p-value
  is not corrected for looking repeatedly.
- `winnerPolicy` is `manual-review`: the service never declares a winner. A person decides.

## Export: OpenABTest

- `GET /api/ab/experiments/<id>/manifest.json`: the experiment as an OpenABTest 0.2 manifest
  (`metrics.primary: conversion-rate`, `metrics.conversion: unique-participants-with-goal`,
  `metrics.goal` = the experiment's goal, `metrics.profit: none`).
- `GET /api/ab/experiments/<id>/events.json`: `{ manifest, events, truncated }`, the eligibility,
  assignment, exposure and conversion events as OpenABTest event documents
  (`producerId: w3bs.org`), newest 50,000. A conversion sent with a `price` exports as a
  purchase; any other conversion exports as a goal conversion, with its value when you sent one.

Both validate with `@logicsrc/validators` 0.5.0 (the test suite checks every exported document).
Each experiment has a goal id (default `conversion`; set it when you create the experiment, e.g.
`signup-completed`).

## Owner API

Session cookie or a `w3bs login` bearer.

| Method   | Path                             |                                                                                                                                                                                                         |
| -------- | -------------------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| GET      | `/api/ab/experiments`            | your experiments                                                                                                                                                                                        |
| POST     | `/api/ab/experiments`            | create a draft: `name`, `slug`, `unit`, `goal`, `variants: [{id, weight, parameters}]`, optional `eligibilityRule`, `days`, `startsAt`, `endsAt`, `minimumEligiblePerVariant`, `minimumObservationDays` |
| GET      | `/api/ab/experiments/<id>`       | results                                                                                                                                                                                                 |
| PATCH    | `/api/ab/experiments/<id>`       | edit a draft                                                                                                                                                                                            |
| POST     | `/api/ab/experiments/<id>/state` | `{"state":"running"\|"paused"\|"closed"}`                                                                                                                                                               |
| DELETE   | `/api/ab/experiments/<id>`       | delete it with its assignments and events                                                                                                                                                               |
| GET/POST | `/api/ab/keys`                   | list, make (`{"name"}`; the key is shown once)                                                                                                                                                          |
| DELETE   | `/api/ab/keys/<key id>`          | revoke                                                                                                                                                                                                  |

## From the terminal

```sh
w3bs login
w3bs ab ls
w3bs ab new checkout-button --variant control --variant green='{"color":"green"}'
w3bs ab start checkout-button
w3bs ab show checkout-button
w3bs ab assign checkout-button user-1842
w3bs ab pause|resume|close checkout-button
w3bs ab export checkout-button > checkout-button.openabtest.json
```
