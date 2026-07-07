# Changelog

All notable Dairo CLI changes are tracked here.

## 0.0.7 - 2026-07-07

### Changed

- Release tags are now v-prefixed (`v0.0.7`). Bare versions keep working
  everywhere a version is accepted: the installers and
  `dairo.app/downloads/cli/{version}/` normalize `0.0.7` to `v0.0.7`.
- Installer hardening: downloads pin HTTPS + TLS 1.2 with retries, checksum
  verification works on systems with only `sha256sum` (minimal Linux) or only
  `shasum` (macOS), and the downloaded binary must execute successfully before
  it replaces an existing install. The Windows installer now persists the
  install directory on the user PATH and enforces TLS 1.2 on Windows
  PowerShell 5.1.
- Release pipeline: missing publish credentials now fail the release instead
  of silently skipping (npm publishing stays behind an explicit `NPM_PUBLISH`
  launch gate until the npm org token exists), every built binary is smoke
  tested (`--version` must match Cargo.toml) before upload, release notes
  generation is mandatory, and workflow token permissions are scoped per job.

## 0.0.2 - 0.0.6 - 2026-07-06/07

- `dairo update`: in-place self-update with checksum verification; detects
  Homebrew-managed installs (including bin shims) and defers to
  `brew upgrade dairo` instead of overwriting them.
- Installer: simplified PATH guidance with an interactive opt-in prompt.
- Releases are published by the dairo bot GitHub App; the Homebrew tap
  (`dairo-app/homebrew-tap`) is updated automatically on release.

## 0.0.1 - 2026-07-06

Everything below (previously tracked as Unreleased) shipped in 0.0.1.

### Channel-agnostic API rename (breaking)

- Send and outbound now ride the unified messages collection. `dairo send` posts
  to `POST /v1/messages` (was `POST /v1/emails`) and `dairo outbound`
  (list/get/cancel/events) is re-pathed onto `/v1/messages*`: the outbound list
  reads `GET /v1/messages?direction=outbound` and is equivalent to
  `dairo messages list --direction outbound`. Command names (`send`, `outbound`,
  `messages`) are unchanged for script stability.
- `dairo messages list` gains a `--channel` filter (alongside the existing
  `--direction`), and `dairo send` gains an optional `--channel` (defaults to the
  inbox's channel, `email`).
- `Message` now carries `channel` + `channelMetadata` (outbound delivery metadata
  — `providerMessageId`, `provider`, `lastEventType`, bounce/complaint timestamps
  — folds in here), `Inbox` carries `channel` and drops the `username` alias, and
  the send request accepts an optional `channel`.
- Webhook event vocabulary renamed: `--event` now takes `message.sent`,
  `message.delivered`, `message.bounced`, `message.complained` (were `email.*`);
  `message.received` is unchanged.
- Vendored OpenAPI + the canonical/implemented contract projections re-synced to
  the messages surface.

### Help & UX

- Reworked `dairo --help` into a scannable, grouped command list (Getting
  started / Email / Realtime and webhooks / Physical mail / Account and access /
  Agents and governance / Tooling) with quick-start examples and a docs link,
  instead of a flat 30-entry dump. A `root_help_lists_every_command` test guards
  the curated list against drift.
- `dairo listen` no longer dumps its full multi-line description as the one-line
  summary in the top-level help (the summary is now a single line; the long form
  shows only in `dairo listen --help`).
- Filled in missing `dairo send` flag descriptions (`--to`, `--subject`,
  `--text`, `--html`, `--attachment`).
- The "missing Dairo API token" error now points to `dairo login` first
  (browser sign-in), then `DAIRO_API_KEY` / `dairo auth token set`.

### Docs

- README now documents `dairo login` (browser OAuth) as the primary sign-in
  path, adds a Quick start section, and corrects the token lookup order.
- Added `CONTRIBUTING.md` (project layout, build/test commands, and how to add a
  command).

### Tooling

- New `dairo init <framework>` scaffolder. Drops a working Dairo starter into a
  project — a configured SDK client, an inbound-webhook handler stub that
  verifies delivery signatures against the **raw** request body, `DAIRO_API_KEY`
  env wiring, and a `DAIRO.md` README snippet. Tier-1 frameworks: `next`,
  `express`, `hono`, `cloudflare-workers`, `fastapi`, `flask`, `go-http`.
  Templates are embedded in the binary, so it works offline. It is idempotent
  (never clobbers existing files without `--force`; `package.json` is merged and
  `.env`/`.gitignore` lines are appended only when missing), confines all writes
  to `--dir`, and writes secret-capable files (`.env*`, `.dev.vars*`) `0600`
  with only empty placeholders (no real secret on disk). Flags: `--dir`,
  `--force`, `--no-install`, `--package-manager`, `--inbox-route`, `--no-verify`,
  and global `--json` (emits the file manifest). An optional best-effort
  `GET /v1/whoami` connectivity check runs after scaffolding (skip with
  `--no-verify` or when no key is configured).
  - The templates pin published SDK versions (npm `dairo`, PyPI `dairo`, Go
    `github.com/dairo-app/dairo-go`).

### Security & reliability

- Reject non-HTTPS base URLs so the bearer API key never travels in cleartext.
  Plain `http://` is allowed only for explicit loopback hosts (`localhost`,
  `127.0.0.1`, `[::1]`, `*.localhost`) for local development.
- Never leak the API key: `ApiClient`'s `Debug` is redacted, and the User-Agent
  (`dairo-cli/<version>`) and error chains never include the token.
- Every request now has a 30s timeout and a bounded retry/backoff (up to 3
  retries, exponential 250ms..5s) for transient `429`/`502` and connect/timeout
  errors.
- `Idempotency-Key` is now stable: caller-supplied where available, otherwise a
  deterministic UUIDv5 of `METHOD path`, so retried mutations de-duplicate
  instead of each carrying a fresh random key.
- Added offline `dairo webhook verify` — constant-time HMAC verification of a
  received delivery (reads the raw body from stdin, checks the
  `X-Dairo-Signature`/`X-Dairo-Timestamp` headers against the `whsec_...`
  secret) matching the backend signing scheme.

### Endpoints

- API redesign alignment: the CLI now uses the current `/v1/emails`,
  `/v1/lists`, `/v1/messages`, `/v1/agents/*`, `/v1/account/residency`,
  `/v1/erasure-jobs`, and `/v1/inboxes/{inbox}/verification-waits` surfaces.
  Legacy paths such as `/v1/send-email`, `/v1/outbound-*`,
  `/v1/email-lists`, `/v1/a2a/*`, and `/v1/compliance/*` are old/replaced
  migration references only.
- Scheduled send: `dairo send --send-at <RFC3339>` stages a future send. The
  `SendMessageRequest` gains `sendAt` and `SendMessageResponse` gains `scheduledAt`
  (status `scheduled`).
- Cancel a scheduled send: `dairo outbound cancel <id>` →
  `POST /v1/emails/{id}/cancel` (`messages:send`); returns the canceled
  email or a conflict if it is no longer scheduled. The outbound-email model
  gains `scheduled`/`canceled` statuses and `scheduledAt`/`canceledAt` fields.
- Audit logs: `dairo audit-logs list [--limit N] [--cursor C]` →
  `GET /v1/audit-logs` (`messages:read`), returning
  `{ logs: [...], pagination: { nextCursor } }`.
- API-key IP allowlist: `dairo api-key create --allowed-ip <ip-or-cidr>` (repeat
  for multiple). `CreateApiKeyRequest` gains `allowedIps`; the API-key object
  (create, list, whoami) gains `allowedIps`.
- Dedicated IPs: `dairo dedicated-ips status` → `GET /v1/dedicated-ips`
  (`messages:read`), returning the dedicated IP pool status.
- Templates: `dairo templates list|create|get|update|delete|versions|version|publish`
  over `/v1/templates` (+ `/{id}/versions[/{version}]`). Reads use `messages:read`;
  `create`/`update` (PATCH)/`delete`/`publish` use `messages:send`. `create`/`publish`
  read the React-email source inline (`--source`) or from a file
  (`--source-file`); `--variables` takes a JSON-object schema; `get` accepts
  `--version`, `publish` accepts `--no-promote` (publish a draft).
- Events ledger: `dairo events list` → `GET /v1/events` (`messages:read`) with
  `--limit/--cursor/--inbox-id/--type/--wait/--tail`; `dairo events replay` →
  `POST /v1/events/replay` (`webhooks:write`) re-delivers a slice to your
  webhooks given exactly one lower bound (`--since`, `--since-seq` + `--inbox-id`,
  or `--since-timestamp`), with `--until/--type.../--webhook-id/--max-events`.
- Agent passport: `dairo agents list|get <idOrAgent>` → `GET /v1/agents[/{id}]`
  (`agents:read`); `dairo agents verify` → public `GET /v1/agents/verify` (always a
  verdict). Verify takes either `--id <messageId>` or the signature form
  (`--agent --kid --sig`, plus optional `--from/--to/--subject/--ts`). There is
  no PATCH/DELETE for agents.
- Reputation: `dairo reputation list` → `GET /v1/agents/reputation` (`agents:read`),
  the fleet circuit-breaker view.
- Budgets: `dairo budgets list|get <scope>` → `GET /v1/budgets[/{scope}]` (`budgets:read`);
  `dairo budgets set --scope … [--scope-id …]` → `PUT /v1/budgets`
  (`budgets:write`, idempotent upsert) with at least one limit
  (`--max-sends-per-day`, `--max-new-recipients-per-hour`,
  `--hard-stop-on-complaint`); `dairo budgets delete <scope>` → `DELETE
  /v1/budgets/{scope}`.
- Compliance: `dairo compliance residency` → `GET /v1/account/residency`
  (`account:read`). `dairo erasure-jobs list|create|get` uses
  `/v1/erasure-jobs` (`compliance:read`/`compliance:write`).
- A2A mail: `dairo a2a list [--limit --cursor --inbox-id]` →
  `GET /v1/messages?channel=a2a` and `dairo a2a get <id>` → `GET /v1/messages/{id}`
  (`messages:read`), the cross-tenant agent-to-agent hop receipts.
- Added `dairo outbound` commands: `list`, `get <id>`, `events`, `bounces`,
  `complaints` — backed by the public `/v1/emails`,
  `/v1/emails/{id}`, and `/v1/emails/{id}/events` routes (`messages:read`).
  Surfaces delivery/bounce/complaint outcomes after an async `queued` send.
- `dairo attachments share` now calls the branded `/v1/attachments/{id}/link`
  route (returns a Dairo `shareUrl`) instead of the raw signed-S3 `/url` route.
- Added `dairo lists delete <listId>` (`DELETE /v1/lists/{listId}`).
- `dairo lists add` and CSV import now use the canonical
  `POST /v1/lists/{listId}/members` endpoint.
- Added `dairo inbox schema get|set|delete` for
  `/v1/inboxes/{inbox}/schema` and `dairo inbox verification-waits
  register|list|get|cancel` for `/v1/inboxes/{inbox}/verification-waits`.
- Letters — generated payment slips: `dairo letter send` gains a structured
  payment object that Dairo *generates* and composites full-width at the bottom
  of a template-rendered letter. New `--payment-type qr|sepaDe|sepaAt`
  (Swiss QR-bill in CHF, German/Austrian SEPA Zahlschein + GiroCode in EUR) with
  `--payment-amount` (> 0, ≤ 2 decimals), an optional `--payment-currency`
  (defaulted from the type), `--payment-reference`/`--payment-message`, a required
  creditor block (`--payment-creditor-name`/`-iban`/`-country`, plus optional
  `-bic`/`-street`/`-house-number`/`-postal-code`/`-city`), and an optional debtor
  block (`--payment-debtor-*`) that defaults to the letter's recipient.
  `CreateLetterRequest` gains a `payment` object and a `templateId`; the generated
  slip is honored only on the new `--template-id` (Dairo-render) path — a
  `--pdf`/`--attachment-id` letter plus `payment` is rejected with "payment slips
  require a template". When `payment` is present the request's `paymentSlip` flag
  is set from `payment.type`. The bare `--payment-slip` string flag stays for
  bring-your-own-slip PDFs (and is mutually exclusive with `--payment-type`).

## 0.1.0

- Initial Rust CLI for domains, inboxes, send-email, webhooks, and API keys.
- Token lookup through `DAIRO_API_KEY` or local config.
- Local token config writes are Unix permission-hardened and atomic.
- `--json` failures use a machine-readable error envelope.
- CI runs formatting, clippy, tests, release build, cargo-deny, and cargo-audit with locked dependencies.
