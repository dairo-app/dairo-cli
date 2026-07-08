# Dairo CLI — Canonical Plan

> Status: **private preview** (`v0.1.0`). This is the single source-of-truth plan
> for `dairo-app/dairo-cli`. Update it in the same PR that changes the command
> surface, the release pipeline, or the API contract.

---

## 1. Purpose

`dairo` is the official Dairo command-line interface, written in **Rust** and
distributed as a single static-ish binary (`reqwest` + `rustls`, no OpenSSL).
It is the human/CI-facing twin of the Dairo MCP server and language SDKs: every
command is a thin, typed wrapper over a public `GET/POST/DELETE /v1/...` route on
`https://api.dairo.app`.

Design goals (in priority order):

1. **Secret safety.** The bearer API key never lands in shell history, process
   listings, `Debug` output, error chains, or the User-Agent. Non-HTTPS base
   URLs are rejected (loopback exempted). One-time secrets (API keys, webhook
   signing secrets) are printed exactly once and redacted everywhere else.
2. **Correctness/parity with the backend `/v1` contract.** Request/response
   bodies use the exact OpenAPI field names (`inboxId`, `sendAt`, `allowedIps`,
   …). A contract-projection test fails CI when the implemented operation set
   drifts from the live contract snapshot.
3. **Agent- and CI-friendly.** `--json` everywhere for machine output; a stable
   JSON error envelope on stderr; deterministic idempotency keys so retries
   de-duplicate; bounded timeout + retry/backoff.
4. **Frictionless install** across npm, Homebrew, `curl | sh`, PowerShell, and
   direct binary download, plus a one-command MCP installer for coding agents.

### Architecture (current files)

| File | Responsibility |
| --- | --- |
| `src/main.rs` | Arg dispatch, send-body assembly, attachment IO, CSV import, error envelope, offline webhook verify wiring. |
| `src/cli.rs` | `clap` derive command tree + arg validation + unit tests. |
| `src/api.rs` | `ApiClient`: URL building, bearer auth, idempotency, timeout/retry, all request methods, all request/response types. |
| `src/output.rs` | Human tables vs `--json` rendering; `print_json` pass-through; bounce/complaint event filtering. |
| `src/config.rs` | Token/`api_url` TOML config; `0700`/`0600` perms; atomic replace; `DAIRO_API_KEY` precedence. |
| `src/webhook.rs` | Constant-time HMAC-SHA256 verify of received deliveries. |
| `src/mcp_install.rs` | Writes MCP server config for Hermes/Codex/Cursor/Claude. |
| `contract/{live,implemented}-operations.json` | OpenAPI operation snapshots; equality enforced by `tests/contract_projection.rs`. |
| `.github/workflows/{ci,release}.yml` | CI gates; multi-platform release + npm + Homebrew. |
| `install/install.{sh,ps1}`, `scripts/*` | Install scripts, npm package generation, Homebrew formula rendering. |

---

## 2. Current state (what ships today)

**Implemented command surface** (`v0.1.0`, all wired end-to-end):

- `auth token set` (stdin-only), `whoami`
- `domain list|add|recheck|delete`
- `inbox list|create|delete`
- `messages (message) list|get|download-attachments`
- `attachments (attachment) url|share|download`
- `threads (thread) list|get`
- `send` (text/html/react, attachments inline/auto, `--send-at` scheduling,
  `--ignore-complaints`)
- `outbound list|get|cancel|events|bounces|complaints`
- `lists (list) list|create|get|delete|add|import-csv|send`
- `webhook list|create|delete|verify` (verify is offline, no API call)
- `api-key list|create|revoke` (with `--allowed-ip` allowlist)
- `audit-logs list` (keyset pagination)
- `dedicated-ips status`
- `mcp install --client auto|hermes|codex|cursor|claude`

**Backend route methods present in `api.rs`** (30 distinct calls): whoami;
domains list/create/delete/recheck; inboxes list/create/delete; send-email;
email-lists list/create/get/delete/members/members-import/send; webhooks
list/create/delete; api-keys list/create/revoke; messages list/get; attachments
url/link/download; threads list/get; outbound-emails list/get/cancel;
outbound-events list; audit-logs list; dedicated-ips list.

**Security/reliability already in place:** HTTPS enforcement (loopback exempt),
redacted `Debug` for client + secret responses, 30s timeout, 3-retry
exponential backoff on `429/502`/connect/timeout, stable `Idempotency-Key`
(caller value or deterministic UUIDv5 of `METHOD path`), atomic `0600` token
file, stdin-only token entry.

**Tests:** `cli.rs` unit tests (parse tree), `api.rs` unit tests (URL/JSON
shape/redaction), `tests/cli_contract.rs` (error-envelope + secret-leak
behavior), `tests/contract_projection.rs` (live vs implemented operation
equality). No live/integration tests against a real or mocked HTTP server.

**Known internal inconsistency:** the `contract/*.json` snapshots declare
**23 operations** and omit several routes the CLI actually calls
(`listOutboundMessages`, `getOutboundMessage`, `listOutboundEvents`,
`getAttachmentBrandedLink`, all 7 `email-lists` ops, `whoami`). The projection
test only checks `live == implemented`, not `implemented == code`, so this drift
is invisible to CI. See gap C1.

---

## 3. Parity gaps vs backend `/v1` + newer features

Baseline: backend exposes ~34 `/v1` routes (per the hosted MCP catalog
families: account, domains, inboxes, mail, attachments, outbound, audiences,
webhooks, api_keys, audit, dedicated_ips) plus scopes `audiences:read`/`audiences:write`
and account-level usage metering (surfaced by `get_account_info`).

Legend: `[ ]` not done · `[~]` partial · `[x]` done (listed for completeness).

### A. Feature-area parity

- [x] **A1. Scheduled send** — `send --send-at` + `SendMessageRequest.sendAt` +
  `SendMessageResponse.scheduledAt` (status `scheduled`). Done.
- [x] **A2. Cancel scheduled send** — `outbound cancel <id>` →
  `POST /v1/outbound-emails/{id}/cancel`. Done.
- [x] **A3. Audit logs** — `audit-logs list` → `GET /v1/audit-logs` with
  `--limit`/`--cursor` keyset pagination. Done.
- [x] **A4. Dedicated IPs** — `dedicated-ips status` → `GET /v1/dedicated-ips`.
  Done.
- [x] **A5. API-key IP allowlisting** — `api-key create --allowed-ip` +
  `allowedIps` on request, list, whoami. Done.
- [x] **A6. Email lists** — full CRUD + members + import + send. Done.
- [x] **A7. Webhooks** — list/create/delete + offline verify. Done.
- [ ] **A8. Template system (UPCOMING).** No `template` command group, no
  `ApiClient` template methods, no `templateId`/`templateData` fields on
  `SendMessageRequest`. **Net-new; blocked on backend route freeze.** See §6.
- [~] **A9. Usage-based metering.** `whoami` surfaces `usage`/`limits`/`period`
  as opaque `serde_json::Value`, so the data is reachable but not typed,
  labeled, or convenient. The MCP surfaces usage and storage metering through
  `get_account_info`; the CLI has no `dairo usage` equivalent and
  no storage-focused view. (Metering has **no** standalone public `/v1/usage`
  route — it is surfaced via `whoami`, so this is an ergonomics gap, not a
  missing route.) See gaps B1, D1.

### B. Missing convenience commands (routes are reachable, UX is not)

- [ ] **B1. `dairo usage` / `dairo whoami --usage`.** A focused current-period
  usage + remaining-headroom view (outbound/inbound this month vs
  `emailsPerMonth`, storage vs `storageBytes`) so agents can budget-gate before
  large sends. Today this requires eyeballing the raw `whoami` JSON.
- [ ] **B2. `dairo outbound events` typed output.** `outbound list/get/events`
  return raw `serde_json::Value` via `print_json` (no typed model, no human
  table). Bounces/complaints are filtered by string-matching `type`. Backend has
  a real `OutboundEvent` shape; add typed structs + a human table.
- [ ] **B3. CC/BCC on `send`.** `SendMessageRequest` has `cc`/`bcc` fields but
  `SendArgs` exposes no `--cc`/`--bcc` flags, so they are always `None`. The
  backend accepts them and meters per accepted recipient including cc/bcc.
- [ ] **B4. `--reply-to` / custom headers / tags** on `send` (if backend
  `SendMessageRequest` supports them — verify against OpenAPI before adding).
- [ ] **B5. List pagination flags** for `lists list` and `audit-logs`/`outbound`
  consistency (`--limit` exists on some, absent on others; unify).

### C. Contract / drift gaps

- [ ] **C1. Contract snapshots are stale and partial.** `contract/*.json`
  declare 23 ops and omit `whoami`, all 7 email-list ops, outbound
  list/get/events, and attachment branded `/link`. Regenerate both snapshots
  from the live backend OpenAPI (`backend/dairo-api/openapi/dairo.openapi.json`)
  so the count matches the ~34-route reality.
- [ ] **C2. Projection test does not cover code.** `tests/contract_projection.rs`
  asserts `live == implemented` but nothing asserts `implemented == what the CLI
  actually calls`. Add a test (or build step) that derives the implemented set
  from `api.rs` route literals, or a doc-comment registry, so adding a method
  without updating the contract fails CI.
- [ ] **C3. No typed audit-log / dedicated-ip / outbound models.** These three
  are `serde_json::Value` pass-throughs. Backend has
  `AuditLogListResponse`, `DedicatedIpListResponse`,
  `CancelOutboundMessageResponse` schemas (referenced in the contract). Add typed
  structs so response-shape drift is caught at compile/test time.

### D. Types / fields

- [ ] **D1. Typed `whoami` usage/limits/period.** Replace the three
  `serde_json::Value` fields with structs matching the documented
  `usage.{outboundEmailsThisMonth,inboundEmailsThisMonth,emailsThisMonth,storageBytes,storageBreakdown,inboxes,domains}`,
  `limits.{emailsPerMonth,storageBytes,inboxes,domains}`, `period.monthStart`,
  and `notes.billing`.
- [ ] **D2. `templateId` / `templateData` send fields** (with A8).
- [ ] **D3. `cc`/`bcc` plumbed from CLI** (with B3).
- [ ] **D4. Verify webhook event enum vs backend.** CLI hardcodes 5 events
  (`message.received`, `email.{sent,delivered,bounced,complained}`). Confirm the
  backend has not added events (e.g. scheduled-send fired, template events).

### E. Tooling / release gaps

- [ ] **E1. Artifact signing.** Release artifacts are unsigned. Add cosign /
  minisign signatures + a published verification key before public GA.
- [ ] **E2. Crates.io publish.** Cargo metadata is crates-ready (`keywords`,
  `categories`, `repository`) but no `cargo publish` step exists. Decide
  whether `dairo` ships on crates.io (and reconcile the name with the `dairo`
  binary / `@dairo/cli` npm name).
- [ ] **E3. SLSA / provenance + checksum signing** for npm and Homebrew.
- [ ] **E4. Shell completions + man page** generation (`clap_complete`,
  `clap_mangen`) shipped in release archives.

**Open gap count: 17** (A8, A9, B1–B5, C1–C3, D1–D4, E1–E4 → 1+1+5+3+4+4 = 18
checkbox items; A9 and D1/B1 overlap as one metering theme). Counting distinct
actionable items: **17**.

---

## 4. Release / publish / deploy runbook

The CLI is published from a **git tag** `vX.Y.Z` that triggers
`.github/workflows/release.yml`. Three distribution channels: GitHub Releases
(source of truth for binaries), npm (`@dairo/*`), Homebrew tap
(`dairo-app/homebrew-tap`). `curl | sh` and PowerShell scripts pull from the
GitHub Release via `https://dairo.app/downloads/cli/...` redirects.

### 4.1 Versioning

- SemVer. `0.x` = private preview; breaking command/output changes allowed but
  documented in `CHANGELOG.md`.
- The single source of truth for the version is **`Cargo.toml` `version`**. The
  tag must be `v<that version>`. npm packages and the Homebrew formula derive
  their version from the release job output (tag-derived, falling back to
  `Cargo.toml`).
- Bump `Cargo.toml`, run `cargo build` to refresh `Cargo.lock`, move the
  `## Unreleased` CHANGELOG section under the new `## X.Y.Z` heading.

### 4.2 Pre-release checklist (run locally, all must pass)

```sh
cargo fmt --check
cargo clippy --locked --all-targets --all-features -- -D warnings
cargo test --locked --all-features
cargo build --release --locked
cargo deny --locked check
cargo audit --file Cargo.lock
```

Then confirm the contract snapshots are current (gap C1): regenerate
`contract/{live,implemented}-operations.json` from the backend OpenAPI and
ensure `tests/contract_projection.rs` passes.

### 4.3 Cut a release (binaries + GitHub Release)

```sh
# from a clean main with the version bump committed and merged
git tag v0.1.1
git push origin v0.1.1
```

The tag push runs `release.yml`:

1. **build** matrix → 5 targets: `aarch64-apple-darwin`,
   `x86_64-apple-darwin`, `x86_64-unknown-linux-gnu`,
   `aarch64-unknown-linux-gnu`, `x86_64-pc-windows-msvc`. Each uploads a
   `binary-<target>` artifact.
2. **release** → archives (`.tar.gz` for unix, `.zip` for windows), bundles the
   install scripts, writes `checksums.txt` (`shasum -a 256`), and creates/updates
   the GitHub Release for the tag.
3. **npm** → `scripts/prepare-npm-packages.mjs` builds 5 platform packages
   (`@dairo/cli-<os>-<cpu>`) + the `@dairo/cli` launcher (Node shim that execs
   the matching native package). On a tag push it publishes all 6 to
   **registry.npmjs.org** when `NPM_TOKEN` is set (else packs tarballs only).
4. **homebrew** → renders `Formula/dairo.rb` via
   `scripts/render-homebrew-formula.py` from `checksums.txt` and pushes to
   `dairo-app/homebrew-tap` when `HOMEBREW_TAP_TOKEN` is set.

### 4.4 Manual / dry-run release

`workflow_dispatch` inputs allow building without a tag and gating publishes:

- `version` — override (defaults to `Cargo.toml`).
- `publish_npm` — publish even on a manual run.
- `update_homebrew` — push the formula even on a manual run.

Use this to validate the pipeline (artifacts + packed tarballs) without
publishing: leave both publish toggles `false`.

### 4.5 Required repo secrets

| Secret | Used by | Effect if missing |
| --- | --- | --- |
| `NPM_TOKEN` | npm job | npm publish skipped (tarballs still uploaded). |
| `HOMEBREW_TAP_TOKEN` | homebrew job | tap update skipped. |
| `github.token` (built-in) | release job | — (always present). |

### 4.6 Which registry / channel

| Channel | Package / location | Trigger |
| --- | --- | --- |
| GitHub Releases | `dairo-<target>.{tar.gz,zip}` + `checksums.txt` | every tag |
| npm | `@dairo/cli` (+ 5 `@dairo/cli-*` native) on npmjs.org | tag (or `publish_npm`) |
| Homebrew | `dairo-app/homebrew-tap` → `brew install dairo-app/tap/dairo` | tag (or `update_homebrew`) |
| curl/PowerShell | `install/install.{sh,ps1}` via `dairo.app/downloads/cli` redirect | reads latest Release |
| crates.io | **not configured** (gap E2) | n/a |

### 4.7 Post-release verification

```sh
npm view @dairo/cli version
brew update && brew info dairo-app/tap/dairo
curl -fsSL https://dairo.app/install.sh | sh && dairo --version
```

Confirm `dairo --version` matches the tag on at least one fresh machine/CI
runner per channel.

---

## 5. Test strategy

### Current

- **Unit (cli.rs):** full `clap` parse tree, alias coverage, range validators,
  body/recipient requirement groups, rejection of unknown webhook events.
- **Unit (api.rs):** URL construction + path encoding, bearer/accept/idempotency
  headers, HTTPS enforcement + loopback exemption, secret-redaction in `Debug`,
  JSON request serialization (OpenAPI names), response deserialization
  (scheduled status, complaint warnings, allowedIps default-empty).
- **Integration (tests/cli_contract.rs):** spawns the built binary
  (`assert_cmd`), asserts the JSON error envelope, secret-non-echo on rejected
  token, `--version` behavior — all **without network**.
- **Contract (tests/contract_projection.rs):** `live == implemented` snapshot
  equality.

### Gaps to close (prioritized)

1. **Mock-HTTP integration tests** (e.g. `wiremock`/`httpmock`): assert each
   `ApiClient` method hits the right method+path+query+body and parses a
   representative success and error response. This is the biggest coverage hole —
   today no test exercises a real request/response round-trip.
2. **Retry/backoff tests:** a mock that returns `429`/`502` then `200` proves the
   stable idempotency key is replayed and the call eventually succeeds; a
   non-retryable `4xx` proves no replay.
3. **Code↔contract test (gap C2):** fail CI when `api.rs` calls a route absent
   from `implemented-operations.json`.
4. **Output golden tests:** snapshot human + `--json` rendering for each printer
   in `output.rs`.
5. **Config tests:** Unix permission bits (`0700`/`0600`), atomic replace,
   `DAIRO_API_KEY` precedence over file, missing-token error.
6. **Webhook verify property tests:** valid/invalid signature, stale timestamp,
   `tolerance-seconds = 0` skip, body tampering.
7. **Cross-platform smoke in release CI:** run `dairo --version` and a
   `--help`/parse-only command on each built target before publishing.

### CI gates (keep + extend)

Keep the existing `ci.yml` chain. Add: the mock-HTTP integration job, the
code↔contract check, and (pre-GA) `cargo llvm-cov` coverage reporting.

---

## 6. Roadmap

### Phase 1 — Parity hardening (no new backend deps)

- C1/C2: regenerate contract snapshots from live OpenAPI; add the code↔contract
  test so drift fails CI.
- B3/D3: add `--cc`/`--bcc` to `send`.
- B1/A9/D1: type the `whoami` usage/limits/period blocks; add a `dairo usage`
  budget view.
- B2/C3: typed models + human tables for `outbound`, `audit-logs`,
  `dedicated-ips` (replace `serde_json::Value` pass-throughs).
- Test: land mock-HTTP + retry tests (test-strategy items 1–2).

### Phase 2 — Template system (A8, blocked on backend)

- Track the upcoming template routes. When frozen, add:
  - `dairo template list|get|create|update|delete`,
  - `ApiClient` template methods + typed `Template` models,
  - `send --template-id <id> --template-data <json>` and `templateId`/
    `templateData` on `SendMessageRequest`,
  - update contract snapshots + docs + CHANGELOG.

### Phase 3 — Distribution GA

- E1/E3: sign artifacts (cosign/minisign) + publish verification key; add SLSA
  provenance; sign npm + Homebrew checksums.
- E2: decide + wire crates.io publish (or document why not).
- E4: ship `clap_complete` shell completions + `clap_mangen` man page in
  release archives; document `dairo completions <shell>`.
- Windows token storage: move off the best-effort TOML file toward an
  ACL/keychain-backed store (called out as a GA blocker in CHANGELOG).

### Phase 4 — Ergonomics

- Interactive `--watch`/poll for `outbound`/`messages` (agent inbox loops).
- `dairo open` deep-links to the dashboard for a resource.
- Config profiles (multiple accounts / environments).
- Output `--format table|json|ndjson` and `--quiet`.

### Definition of "public GA ready"

All of: contract snapshots match live (~34 ops) and are CI-enforced against code;
signed multi-platform artifacts with a published key; documented install/update
channel per registry; mock-HTTP + retry test coverage; template parity (if
shipped backend-side); Windows token-storage decision resolved; CHANGELOG-based
release notes.
