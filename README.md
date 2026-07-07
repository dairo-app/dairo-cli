# Dairo CLI

The official Dairo command-line interface.

## Installation

Dairo ships native binaries for macOS (arm64, x64), Linux (arm64, x64 — glibc 2.35+ and static musl builds for Alpine/container distros), and Windows (x64, arm64). The installers and the npm launcher pick the right flavor automatically, including inside Docker containers and agent sandboxes. The CLI requires outbound HTTPS access to the Dairo API at runtime.

```sh
npm install -g @dairo-app/cli   # or: npx dairo-cli
brew install dairo-app/tap/dairo
curl -fsSL https://dairo.app/install.sh | sh
```

Windows PowerShell:

```powershell
irm https://dairo.app/install.ps1 | iex
```

Direct download URLs are available, for example:

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

Update an installed CLI in place:

```sh
dairo update
```

The updater downloads the native release archive for your platform, verifies it
against `checksums.txt`, checks the downloaded binary's version, and only then
replaces the current executable.

## Quick start

```sh
dairo login                 # sign in with your browser (OAuth)
dairo whoami                # confirm the signed-in account and scopes
dairo inbox create billing --domain example.com
dairo send --from billing@example.com --to jane@example.com \
  --subject "Hello from Dairo" --text "Sent with the Dairo CLI."
dairo listen                # tail live inbox events
```

Run `dairo --help` for the grouped command list, or `dairo <command> --help` for any command's full options.

## Authentication

The CLI authenticates with a Dairo API key using bearer auth. The easiest way to get one is `dairo login`, which runs a browser OAuth (Authorization Code + PKCE) flow — the same one the Dairo MCP clients use — and stores the resulting scoped `dairo_live_*` token for you:

```sh
dairo login                 # opens your browser, then stores the token
dairo logout                # revokes the stored token server-side and clears it
```

For CI and headless hosts where no browser is available, set `DAIRO_API_KEY` or save a token manually with `dairo auth token set` (see below).

Token lookup order:

1. `DAIRO_API_KEY`
2. local config file set by `dairo login` or `dairo auth token set`

Prefer environment variables for CI and short-lived automation:

```sh
export DAIRO_API_KEY="dairo_..."
```

Save a token locally by piping it through stdin:

```sh
printf '%s' "$DAIRO_API_KEY" | dairo auth token set
```

Do not pass tokens as command-line arguments. They can leak through shell history and process listings, so `dairo auth token set dairo_...` is rejected.

Token storage uses a local TOML config file, not an OS keychain. On Unix platforms the CLI writes the config directory as `0700` and the config file as `0600` using an atomic replace. On Windows, use `DAIRO_API_KEY` or a dedicated account if ACL-backed storage is required.

Config file locations follow the platform config directory, for example `~/.config/dairo/config.toml` on Linux. The API URL can be overridden with `DAIRO_API_URL` or the hidden global `--api-url` for tests and staging.

## Commands

### Scaffold a project (`dairo init`)

`dairo init <framework>` drops a working Dairo starter into your project: a configured SDK client, an inbound-webhook handler stub that verifies delivery signatures against the raw request body, `DAIRO_API_KEY` env wiring, and a `DAIRO.md` README snippet. Templates are embedded in the binary, so it works offline.

```sh
dairo init next
```

Tier-1 frameworks: `next`, `express`, `hono`, `cloudflare-workers`, `fastapi`, `flask`, `go-http`. The first three pull the `dairo` npm SDK, the Python pair pull the `dairo` PyPI SDK, and `go-http` pulls `github.com/dairo-app/dairo-go`.

Flags:

- `--dir <PATH>` — target project directory (default `.`, created if missing). All writes are confined to this directory.
- `--force` — overwrite files that already exist. Without it, `init` never clobbers an existing file: it skips and warns, so re-running is safe and idempotent. `package.json` is always merged (your other keys are preserved), and `.env`/`.gitignore` lines are only appended when missing (existing values are never overwritten).
- `--no-install` — only write files and print the manual install command.
- `--package-manager <pm>` — override auto-detection (`npm`/`pnpm`/`yarn`/`bun`, `pip`/`poetry`/`uv`, or `go`).
- `--inbox-route <PATH>` — the URL path the webhook handler is mounted at (default `/api/dairo/webhook`).
- `--no-verify` — skip the post-scaffold `GET /v1/whoami` connectivity check (also auto-skipped when no API key is configured).
- `--json` — emit the file manifest (`{ framework, dir, files, install, nextSteps }`) instead of human text.

The generated `.env.example` (or `.dev.vars.example` for Cloudflare Workers) contains only empty `DAIRO_API_KEY=` / `DAIRO_WEBHOOK_SECRET=` placeholders — a real secret is never written to disk. Secret-capable files are written `0600` on Unix.

After scaffolding, register the webhook and (for local development) forward live events to it:

```sh
dairo webhook create --url https://<your-host>/api/dairo/webhook --event message.received
dairo listen --forward-to http://localhost:3000/api/dairo/webhook
```

### Domains

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

### Inboxes

List inboxes:

```sh
dairo inbox list
```

Create an inbox:

```sh
dairo inbox create billing --domain example.com
```

### Send email

Send an email. At least one non-empty `--to` recipient is required, along with at least one body option: `--text`, `--html`, or `--react-source`. Inline attachments use `--attachment`; `--attachment-delivery` accepts `attachment`, `link`, or `auto`. `attachment` sends files inline. `auto` sends inline only when the files fit Dairo's safe inline limit. `link` is explicit but currently cannot upload a new local file because the CLI has no standalone file upload/link API contract yet, so it fails with guided instructions instead of pretending to send or editing the email body. Dairo never auto-inserts links into `--text` or `--html`.

Complaint suppression is enforced before Dairo queues the send. If a recipient previously complained, the CLI shows an actionable error and does not send by default. Override only deliberately with `--ignore-complaints`; API/MCP callers use `ignoreComplaints=true`. Use `--json` where supported to preserve raw warning/error metadata such as `recipient`, `sourceMessageId`, `providerMessageId`, `complaintFeedbackType`, `complaintUserAgent`, and `lastEventAt` when returned by the backend.

For link-style delivery today, create or reuse a persisted email attachment link with `dairo attachments share <attachment-id> --expiry-hours <1-168>`, place the printed URL exactly where you want it in `--text`/`--html`, then send without the local file attachment. Use `--attachment-link-expiry-hours <1-168>` on `send` to make the guided `link`/oversize message use the same expiry window you intend to request once standalone local file links exist.

```sh
dairo send \
  --inbox-id 018f0000-0000-0000-0000-000000000000 \
  --to jane@example.com \
  --subject "Hello from Dairo" \
  --text "This was sent with the Dairo CLI." \
  --attachment ./invoice.pdf \
  --attachment-delivery auto
```

Send using hosted React rendering. The CLI passes the React source and optional props through to Dairo; rendering happens server-side before outbound delivery.

```sh
dairo send \
  --inbox-id 018f0000-0000-0000-0000-000000000000 \
  --to jane@example.com \
  --subject "Your receipt" \
  --react-source 'export default function Email(props) { return <p>Hello {props.name}</p>; }' \
  --react-props '{"name":"Jane"}'
```

Schedule a send for a future time with `--send-at` (RFC3339 with an explicit timezone offset). The response status is `scheduled` with a `scheduledAt` timestamp; the message is staged and fires at the requested time. Cancel a scheduled send before it fires with `dairo outbound cancel <messageId>` (this fails if the email is no longer scheduled).

```sh
dairo send \
  --inbox-id 018f0000-0000-0000-0000-000000000000 \
  --to jane@example.com \
  --subject "Reminder" \
  --text "Sent on schedule." \
  --send-at 2026-06-11T09:00:00Z

dairo outbound cancel <messageId>           # cancel a still-scheduled send
```

### Webhooks

List and create webhooks. `message.received` is the primary event for agents and external automation that need to react when new inbox mail arrives:

```sh
dairo webhook list
dairo webhook create \
  --url https://example.com/dairo/webhook \
  --event message.received \
  --event message.delivered
```

Supported events are `message.received`, `message.sent`, `message.delivered`, `message.bounced`, and `message.complained`. List output includes status, events, and the latest successful delivery time when the backend has one. The create command prints a one-time signing secret. Store it immediately.

Delete a webhook by ID or URL:

```sh
dairo webhook delete https://example.com/dairo/webhook
```

Verify a received webhook delivery offline (no API call). Pipe the raw request body to stdin and pass the `whsec_...` signing secret plus the `X-Dairo-Signature` and `X-Dairo-Timestamp` header values. Verification is constant-time and the timestamp is checked against `--tolerance-seconds` (default 300; pass `0` to skip the freshness check):

```sh
printf '%s' "$RAW_BODY" | dairo webhook verify \
  --secret "$DAIRO_WEBHOOK_SECRET" \
  --signature "$X_DAIRO_SIGNATURE" \
  --timestamp "$X_DAIRO_TIMESTAMP"
```

### Outbound history and delivery events

Inspect outbound message history and delivery events via the `dairo outbound` commands. The channel-agnostic redesign folds these onto the unified messages collection (backed by `GET /v1/messages?direction=outbound` and `GET /v1/messages/{id}/events`); `dairo outbound list` is equivalent to `dairo messages list --direction outbound`:

```sh
dairo outbound list --limit 20            # recent outbound messages
dairo outbound get <messageId>            # one message + its delivery timeline
dairo outbound cancel <messageId>         # cancel a scheduled (not-yet-sent) message
dairo outbound events --email-id <id>     # delivery events (delivered/bounced/...)
dairo outbound bounces --email-id <id>    # only bounce events
dairo outbound complaints --email-id <id> # only complaint events
```

Outbound messages carry `status` (including `scheduled` and `canceled`) plus `scheduledAt`/`canceledAt` timestamps when set, and channel-specific delivery metadata under `channelMetadata`.

Event rows carry metadata-only join keys such as `messageId`, `recipient`, `providerMessageId`, `subject`, `from`, `to`, event `type`, bounce/complaint details, and `occurredAt`. Output is JSON; use `--json` for the same machine form in scripts.

### Send physical mail (`dairo letter`)

Send and track physical-mail letters from a PDF, backed by `/v1/letters`. Physical mail is irreversible, so `letter send` defaults to a **draft** (`autoSend=false`): pass `--confirm` to submit it for printing and posting. The PDF is read locally and base64-encoded (`--pdf`), or referenced by an existing Dairo attachment (`--attachment-id`). Reads need `letters:read`; `send`/`cancel` need `letters:send`.

```sh
# Create a draft (does NOT post yet) and inspect the exact request first
dairo letter send \
  --pdf invoice.pdf \
  --to-name "Jane Doe" --to-street "Hauptstrasse" --to-house-number 12 \
  --to-postal-code 8001 --to-city "Zürich" --to-country CH \
  --grayscale --duplex --delivery economy \
  --dry-run

# Actually submit it for print + post (color, single-sided, registered)
dairo letter send \
  --pdf invoice.pdf \
  --to-street "Hauptstrasse" --to-house-number 12 --to-zip 8001 --to-country CH \
  --color --simplex --delivery registered \
  --confirm

# Bring-your-own slip: your PDF already carries a slip; --payment-slip just
# tells the provider which paper to use (qr | sepaDe | sepaAt)
dairo letter send \
  --pdf invoice.pdf \
  --to-street "Hauptstrasse" --to-house-number 12 --to-zip 8001 --to-country CH \
  --payment-slip sepaDe --notifications true \
  --confirm

# Dairo-generated slip: render the letter from a template and let Dairo generate
# and composite the slip (Swiss QR-bill in CHF here) full-width at the bottom.
# The debtor defaults to the recipient (--to-*) unless --payment-debtor-* is set.
dairo letter send \
  --template-id tmpl_invoice \
  --to-name "Jane Doe" --to-street "Hauptstrasse" --to-house-number 12 \
  --to-zip 8001 --to-city "Zürich" --to-country CH \
  --payment-type qr --payment-amount 49.90 \
  --payment-creditor-name "Acme AG" \
  --payment-creditor-iban CH9300762011623852957 --payment-creditor-country CH \
  --payment-reference 210000000003139471430009017 \
  --payment-message "Invoice inv_123" \
  --confirm

dairo letter list --status in_transit --country CH   # filter the letter list
dairo letter get <letterId>                          # one letter + its timeline
dairo letter cancel <letterId>                        # cancel before dispatch
dairo letter events <letterId>                        # delivery events (undeliverable
                                                     # events carry a brand-scrubbed reason)
dairo letter price --country CH --page-count 3 --grayscale --duplex  # project cost
```

Print options are `--color`/`--grayscale`, `--simplex`/`--duplex`, and `--address-placement left|right`; delivery is one of `economy|priority|registered|bulk|premium`. The recipient address uses `--to-*` flags (`--to-country` is required, plus either `--to-street` or `--to-po-box`); an optional sender block uses the matching `--from-*` flags. Add `--json` for machine-readable output, as with the rest of the CLI.

A letter comes from exactly one source: an inline `--pdf`, an existing `--attachment-id`, or a Dairo `--template-id` (rendered server-side). Payment slips have two shapes. The bare `--payment-slip qr|sepaDe|sepaAt` flag is for a **bring-your-own** slip — your PDF already carries it and the flag only selects the paper. The structured `--payment-*` flags make Dairo **generate** the slip (Swiss QR-bill in CHF for `qr`, SEPA Zahlschein + GiroCode in EUR for `sepaDe`/`sepaAt`) and composite it full-width at the bottom of the rendered letter; this is honored only on the `--template-id` path (a `--pdf` letter plus a generated slip is rejected with "payment slips require a template"). When you use `--payment-type`, the creditor block (`--payment-creditor-name`/`-iban`/`-country`) and `--payment-amount` (> 0, at most two decimals) are required, the currency defaults from the type, and the debtor defaults to the recipient unless you pass `--payment-debtor-*`.

### API keys

List and create API keys:

```sh
dairo api-key list
dairo api-key create \
  --name CI \
  --scope messages:send \
  --scope messages:read
```

The create command prints a one-time API key secret. Store it immediately.

Restrict a key to specific source IPs or CIDR ranges with one or more `--allowed-ip` flags. Omit the flag to allow the key from any IP. The allowlist is shown in `dairo api-key list`, `dairo whoami`, and the create output.

```sh
dairo api-key create \
  --name "prod worker" \
  --scope messages:send \
  --allowed-ip 203.0.113.0/24 \
  --allowed-ip 198.51.100.7
```

Revoke an API key:

```sh
dairo api-key revoke key_123
```

### Install Dairo MCP

Install Dairo MCP for agents with one command. It saves the token through stdin, then configures supported local clients without printing the key:

```sh
printf '%s' "$DAIRO_API_KEY" | dairo auth token set && dairo mcp install --client auto
```

`--client auto` configures Hermes, Codex, Cursor, and a project `.mcp.json` for Claude. You can target one client with `--client hermes`, `--client codex`, `--client cursor`, or `--client claude`. The remote endpoint is `https://api.dairo.app/mcp` and exposes agent-first tools like `dairo.whoami`, `dairo.send`, `dairo.list.outbound.events`, and `dairo.send.audience`.

### Messages

Inspect mailbox messages (both directions). List output includes an attachment indicator; `get` prints body text/html and attachment metadata when present. Filter with `--direction inbound|outbound` and `--channel email|a2a` (`--direction outbound` is the folded outbound history view):

```sh
dairo messages list --inbox-id 018f0000-0000-0000-0000-000000000000
dairo messages list --direction outbound          # outbound history
dairo messages list --channel a2a                 # agent-to-agent messages
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

Singular aliases (`message`, `thread`, `attachment`) remain available, but the documented command surface is plural for mailbox collections.

### Audit logs

Inspect the account audit log of security-relevant control-plane actions (resource create/delete, key revoke, email send). Output is JSON with keyset pagination; pass the returned `pagination.nextCursor` to `--cursor` for the next page:

```sh
dairo audit-logs list --limit 50
dairo audit-logs list --limit 50 --cursor <nextCursor>
```

### Dedicated IPs

Inspect dedicated IP pool status (available on plans with dedicated IPs):

```sh
dairo dedicated-ips status
```

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

Human output remains table/text oriented. One-time secrets from webhook/API-key creation are intentionally printed once; redirect or capture stdout carefully.

## Troubleshooting

- `missing Dairo API token`: set `DAIRO_API_KEY` or pipe a token into `dairo auth token set`.
- Network errors: verify `DAIRO_API_URL` is unset or points at a reachable Dairo API such as `https://api.dairo.app`.
- Permission errors writing config: prefer `DAIRO_API_KEY`, or remove and recreate the platform config directory with user-only permissions.
- SES provider errors: SES remains the source of truth for sender/domain verification, quotas, suppression, bounces, complaints, and provider rejections.

## Documentation

Full API and CLI documentation is available at [docs.dairo.app](https://docs.dairo.app).

## License

MIT
