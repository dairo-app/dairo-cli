# Changelog

All notable Dairo CLI private-preview changes are tracked here.

## Unreleased

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
    `github.com/dairo-app/dairo-go`). Those SDK publishes are **owner-gated**:
    confirm each package is live at its pinned version before cutting a CLI
    release.

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

- Scheduled send: `dairo send --send-at <RFC3339>` stages a future send. The
  `SendEmailRequest` gains `sendAt` and `SendEmailResponse` gains `scheduledAt`
  (status `scheduled`).
- Cancel a scheduled send: `dairo outbound cancel <id>` →
  `POST /v1/outbound-emails/{id}/cancel` (`mail:send`); returns the canceled
  email or a conflict if it is no longer scheduled. The outbound-email model
  gains `scheduled`/`canceled` statuses and `scheduledAt`/`canceledAt` fields.
- Audit logs: `dairo audit-logs list [--limit N] [--cursor C]` →
  `GET /v1/audit-logs` (`mail:read`), returning
  `{ logs: [...], pagination: { nextCursor } }`.
- API-key IP allowlist: `dairo api-key create --allowed-ip <ip-or-cidr>` (repeat
  for multiple). `CreateApiKeyRequest` gains `allowedIps`; the API-key object
  (create, list, whoami) gains `allowedIps`.
- Dedicated IPs: `dairo dedicated-ips status` → `GET /v1/dedicated-ips`
  (`mail:read`), returning the dedicated IP pool status.
- Templates: `dairo templates list|create|get|update|delete|versions|version|publish`
  over `/v1/templates` (+ `/{id}/versions[/{version}]`). Reads use `mail:read`;
  `create`/`update` (PATCH)/`delete`/`publish` use `mail:send`. `create`/`publish`
  read the React-email source inline (`--source`) or from a file
  (`--source-file`); `--variables` takes a JSON-object schema; `get` accepts
  `--version`, `publish` accepts `--no-promote` (publish a draft).
- Events ledger: `dairo events list` → `GET /v1/events` (`mail:read`) with
  `--limit/--cursor/--inbox-id/--type/--wait/--tail`; `dairo events replay` →
  `POST /v1/events/replay` (`webhooks:write`) re-delivers a slice to your
  webhooks given exactly one lower bound (`--since`, `--since-seq` + `--inbox-id`,
  or `--since-timestamp`), with `--until/--type.../--webhook-id/--max-events`.
- Agent passport: `dairo agents list|get <idOrAgent>` → `GET /v1/agents[/{id}]`
  (`mail:read`); `dairo agents verify` → public `GET /v1/verify` (always a
  verdict). Verify takes either `--id <messageId>` or the signature form
  (`--agent --kid --sig`, plus optional `--from/--to/--subject/--ts`). There is
  no PATCH/DELETE for agents.
- Reputation: `dairo reputation list` → `GET /v1/reputation` (`mail:read`),
  the fleet circuit-breaker view.
- Budgets: `dairo budgets get <scope>` → `GET /v1/budgets/{scope}` (`mail:read`);
  `dairo budgets set --scope … [--scope-id …]` → `PUT /v1/budgets`
  (`keys:write`, idempotent upsert) with at least one limit
  (`--max-sends-per-day`, `--max-new-recipients-per-hour`,
  `--hard-stop-on-complaint`); `--disabled` clears the default-enabled flag.
- Compliance: `dairo compliance residency` → `GET /v1/compliance/residency`
  and `dairo compliance erasure-job <id>` → `GET /v1/compliance/erasure-jobs/{id}`
  (both `mail:read`). There is no root `GET /v1/compliance`.
- A2A mail: `dairo a2a list [--limit --cursor --inbox-id]` →
  `GET /v1/a2a/messages` and `dairo a2a get <id>` → `GET /v1/a2a/messages/{id}`
  (`mail:read`), the cross-tenant agent-to-agent hop receipts.
- Added `dairo outbound` commands: `list`, `get <id>`, `events`, `bounces`,
  `complaints` — backed by the public `/v1/outbound-emails`,
  `/v1/outbound-emails/{id}`, and `/v1/outbound-events` routes (`mail:read`).
  Surfaces delivery/bounce/complaint outcomes after an async `queued` send.
- `dairo attachments share` now calls the branded `/v1/attachments/{id}/link`
  route (returns a Dairo `shareUrl`) instead of the raw signed-S3 `/url` route.
- Added `dairo lists delete <listId>` (`DELETE /v1/email-lists/{listId}`).
- `dairo lists add` now uses the canonical `POST /v1/email-lists/{listId}/members`
  endpoint; CSV import continues to use the `/members/import` alias.

## 0.1.0 - Private preview

- Initial Rust CLI for domains, inboxes, send-email, webhooks, and API keys.
- Token lookup through `DAIRO_API_KEY` or local config.
- Local token config writes are Unix permission-hardened and atomic.
- `--json` failures use a machine-readable error envelope.
- CI runs formatting, clippy, tests, release build, cargo-deny, and cargo-audit with locked dependencies.

Public release requirements before distributing binaries:

- Signed multi-platform artifacts.
- Documented install/update channel.
- Final npm/crates/package naming policy across SDKs.
- Security review of token storage and Windows ACL behavior.
