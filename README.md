# Dairo CLI

Official Dairo command-line interface.

Status: private preview. The CLI is intended for Dairo developers and early
integrators while the public API, package signing, and release channels settle.
Do not treat this repository as a stable public distribution channel yet.

## Supported platforms

Release automation builds native binaries for:

- macOS arm64 and x64
- Linux arm64 and x64
- Windows x64

Runtime requires outbound HTTPS access to the Dairo API. Windows token-file
permissions are best-effort; use `DAIRO_API_KEY` for CI or stricter ACL needs.

## Install

Official install channels:

```sh
npm install -g @dairo/cli
brew install dairo-app/tap/dairo
curl -fsSL https://dairo.app/install.sh | sh
```

Windows PowerShell:

```powershell
irm https://dairo.app/install.ps1 | iex
```

Direct official download URLs redirect through Dairo's domain to GitHub Release
artifacts, for example:

```text
https://dairo.app/downloads/cli/latest/dairo-aarch64-apple-darwin.tar.gz
https://dairo.app/downloads/cli/latest/dairo-x86_64-pc-windows-msvc.zip
```

From the repository:

```sh
cargo install --path . --locked
```

Or run without installing:

```sh
cargo run --locked -- --help
```

Release tags (`vX.Y.Z`) build all platform binaries, create a GitHub Release,
produce npm platform packages, and can update the Homebrew tap when the release
secrets are configured.

## Authentication and token security

The CLI authenticates with a Dairo API key using bearer auth.

Token lookup order:

1. `DAIRO_API_KEY`
2. local config file set by `dairo auth token set`

Prefer environment variables for CI and short-lived automation:

```sh
export DAIRO_API_KEY="dairo_..."
```

Save a token locally by piping it through stdin:

```sh
printf '%s' "$DAIRO_API_KEY" | dairo auth token set
```

Do not pass tokens as command-line arguments. They can leak through shell
history and process listings, so `dairo auth token set dairo_...` is rejected.

Preview token storage uses a local TOML config file, not an OS keychain. On Unix
platforms the CLI writes the config directory as `0700` and the config file as
`0600` using an atomic replace. On Windows, use `DAIRO_API_KEY` or a dedicated
preview account if ACL-backed storage is required.

Config file locations follow the platform config directory, for example
`~/.config/dairo/config.toml` on Linux. The API URL can be overridden with
`DAIRO_API_URL` or hidden global `--api-url` for tests and staging.

## Commands

List domains:

```sh
dairo domain list
```

Add a domain:

```sh
dairo domain add example.com
```

Recheck a domain's DNS/SES status:

```sh
dairo domain recheck example.com
```

List inboxes:

```sh
dairo inbox list
```

Create an inbox:

```sh
dairo inbox create billing --domain example.com
```

Send an email. At least one non-empty `--to` recipient is required, along with at least one body option: `--text`, `--html`, or `--react-source`. Inline attachments use `--attachment`; `--attachment-delivery` accepts `attachment`, `link`, or `auto`. `attachment` sends files inline. `auto` sends inline only when the files fit Dairo's safe inline limit. `link` is explicit but currently cannot upload a new local file because the CLI has no standalone file upload/link API contract yet, so it fails with guided instructions instead of pretending to send or editing the email body. Dairo never auto-inserts links into `--text` or `--html`.

Complaint suppression is enforced before Dairo queues the send. If a recipient
previously complained, the CLI shows an actionable error and does not send by
default. Override only deliberately with `--ignore-complaints`; API/MCP callers
use `ignoreComplaints=true`. Use `--json` where supported to preserve raw
warning/error metadata such as `recipient`, `sourceOutboundEmailId`,
`providerMessageId`, `complaintFeedbackType`, `complaintUserAgent`, and
`lastEventAt` when returned by the backend.

For link-style delivery today, create or reuse a persisted email attachment link with `dairo attachments share <attachment-id> --expiry-hours <1-168>`, place the printed URL exactly where you want it in `--text`/`--html`, then send without the local file attachment. Use `--attachment-link-expiry-hours <1-168>` on `send` to make the guided `link`/oversize message use the same expiry window you intend to request once standalone local file links exist.

```sh
dairo send \
  --inbox-id 018f0000-0000-0000-0000-000000000000 \
  --to max@example.com \
  --subject "Hello from Dairo" \
  --text "This was sent with the Dairo CLI." \
  --attachment ./invoice.pdf \
  --attachment-delivery auto
```

Send using hosted React rendering. The CLI passes the React source and optional props through to Dairo; rendering happens server-side before outbound delivery.

```sh
dairo send \
  --inbox-id 018f0000-0000-0000-0000-000000000000 \
  --to max@example.com \
  --subject "Your receipt" \
  --react-source 'export default function Email(props) { return <p>Hello {props.name}</p>; }' \
  --react-props '{"name":"Max"}'
```

Schedule a send for a future time with `--send-at` (RFC3339 with an explicit
timezone offset). The response status is `scheduled` with a `scheduledAt`
timestamp; the message is staged and fires at the requested time. Cancel a
scheduled send before it fires with `dairo outbound cancel <emailId>` (this
fails if the email is no longer scheduled).

```sh
dairo send \
  --inbox-id 018f0000-0000-0000-0000-000000000000 \
  --to max@example.com \
  --subject "Reminder" \
  --text "Sent on schedule." \
  --send-at 2026-06-11T09:00:00Z

dairo outbound cancel <emailId>           # cancel a still-scheduled send
```

List and create webhooks. `message.received` is the primary event for coding
agents and external automation that need to react when new inbox mail arrives:

```sh
dairo webhook list
dairo webhook create \
  --url https://example.com/dairo/webhook \
  --event message.received \
  --event email.delivered
```

Supported events are `message.received`, `email.sent`, `email.delivered`,
`email.bounced`, and `email.complained`. List output includes status, events,
and the latest successful delivery time when the backend has one. The create
command prints a one-time signing secret. Store it immediately.

Inspect outbound email history and delivery events via the `dairo outbound`
commands (backed by `GET /v1/outbound-emails` and `GET /v1/outbound-events`):

```sh
dairo outbound list --limit 20            # recent outbound emails
dairo outbound get <emailId>              # one email + its delivery timeline
dairo outbound cancel <emailId>           # cancel a scheduled (not-yet-sent) email
dairo outbound events --email-id <id>     # delivery events (delivered/bounced/...)
dairo outbound bounces                    # only bounce events
dairo outbound complaints                 # only complaint events
```

Outbound emails carry `status` (including `scheduled` and `canceled`) plus
`scheduledAt`/`canceledAt` timestamps when set.

Event rows carry metadata-only join keys such as `emailId`, `recipient`,
`providerMessageId`, `subject`, `from`, `to`, event `type`, bounce/complaint
details, and `occurredAt`. Output is JSON; use `--json` for the same machine
form in scripts.

Delete a webhook by ID or URL:

```sh
dairo webhook delete https://example.com/dairo/webhook
```

Verify a received webhook delivery offline (no API call). Pipe the raw request
body to stdin and pass the `whsec_...` signing secret plus the
`X-Dairo-Signature` and `X-Dairo-Timestamp` header values. Verification is
constant-time and the timestamp is checked against `--tolerance-seconds`
(default 300; pass `0` to skip the freshness check):

```sh
printf '%s' "$RAW_BODY" | dairo webhook verify \
  --secret "$DAIRO_WEBHOOK_SECRET" \
  --signature "$X_DAIRO_SIGNATURE" \
  --timestamp "$X_DAIRO_TIMESTAMP"
```

List and create API keys:

```sh
dairo api-key list
dairo api-key create \
  --name CI \
  --scope mail:send \
  --scope mail:read
```

The create command prints a one-time API key secret. Store it immediately.

Restrict a key to specific source IPs or CIDR ranges with one or more
`--allowed-ip` flags. Omit the flag to allow the key from any IP. The allowlist
is shown in `dairo api-key list`, `dairo whoami`, and the create output.

```sh
dairo api-key create \
  --name "prod worker" \
  --scope mail:send \
  --allowed-ip 203.0.113.0/24 \
  --allowed-ip 198.51.100.7
```

Install Dairo MCP for coding agents with one command. It saves the token through
stdin, then configures supported local clients without printing the key:

```sh
printf '%s' "$DAIRO_API_KEY" | dairo auth token set && dairo mcp install --client auto
```

`--client auto` configures Hermes, Codex, Cursor, and a project `.mcp.json` for
Claude. You can target one client with `--client hermes`, `--client codex`,
`--client cursor`, or `--client claude`. The remote endpoint is
`https://api.dairo.app/mcp` and exposes agent-first tools like
`dairo.whoami`, `dairo.send.email`, `dairo.list.outbound.events`, and
`dairo.send.email.list`.

Revoke an API key:

```sh
dairo api-key revoke key_123
```

Inspect mailbox messages. List output includes an attachment indicator; `get` prints body text/html and attachment metadata when present:

```sh
dairo messages list --inbox-id 018f0000-0000-0000-0000-000000000000
dairo messages get msg_123
```

Download attachment files. `attachments url` prints a short-lived direct download URL; `attachments share` prints the short-lived human share-page URL. Add `--expiry-hours <1-168>` when a longer handoff window is needed. `attachments download` uses the direct Dairo API fast path and writes bytes to a file or directory:

```sh
dairo attachments url att_456 --expiry-hours 24
dairo attachments share att_456 --expiry-hours 24
dairo attachments download att_456 --out ./invoice.pdf
dairo messages download-attachments msg_123 --out ./downloads
```

Inspect mailbox threads:

```sh
dairo threads list --inbox-id 018f0000-0000-0000-0000-000000000000
dairo threads get thread_123
```

Inspect the account audit log of security-relevant control-plane actions
(resource create/delete, key revoke, email send). Output is JSON with keyset
pagination; pass the returned `pagination.nextCursor` to `--cursor` for the next
page:

```sh
dairo audit-logs list --limit 50
dairo audit-logs list --limit 50 --cursor <nextCursor>
```

Inspect dedicated IP pool status (available on plans with dedicated IPs):

```sh
dairo dedicated-ips status
```

Singular aliases (`message`, `thread`, `attachment`) remain available, but the documented command surface is plural for mailbox collections.

## JSON and error contract

Use `--json` for machine-readable success output where supported:

```sh
dairo --json domain list
```

Failures with `--json` are emitted to stderr as a stable envelope:

```json
{
  "error": {
    "message": "missing Dairo API token; set DAIRO_API_KEY or run `dairo auth token set`",
    "code": "command_failed",
    "status": null
  }
}
```

Human output remains table/text oriented. One-time secrets from webhook/API-key
creation are intentionally printed once; redirect or capture stdout carefully.

## Troubleshooting

- `missing Dairo API token`: set `DAIRO_API_KEY` or pipe a token into
  `dairo auth token set`.
- Network errors: verify `DAIRO_API_URL` is unset or points at a reachable Dairo
  API such as `https://api.dairo.app`.
- Permission errors writing config: prefer `DAIRO_API_KEY`, or remove and
  recreate the platform config directory with user-only permissions.
- SES provider errors: SES remains the source of truth for sender/domain verification,
  quotas, suppression, bounces, complaints, and provider rejections.

## Release policy

Version `0.1.x` is private preview. Breaking command/output changes may still
happen, but security fixes should preserve documented behavior where possible.
Before public release, Dairo should add signed multi-platform artifacts,
changelog-based release notes, and a documented install channel.

## Development

```sh
cargo fmt --check
cargo clippy --locked --all-targets --all-features -- -D warnings
cargo test --locked --all-features
cargo build --release --locked
cargo deny --locked check
cargo audit --file Cargo.lock
```

Support/contact: use the private `dairo-app/dairo-cli` repository while this is
in preview.

License: MIT
