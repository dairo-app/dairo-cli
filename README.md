# Dairo CLI

Official Dairo command-line interface.

Status: private preview. The CLI is intended for Dairo developers and early
integrators while the public API, package signing, and release channels settle.
Do not treat this repository as a stable public distribution channel yet.

## Supported platforms

- macOS and Linux are supported for preview development.
- Windows builds should compile, but token-file permissions are best-effort
  because Windows ACL hardening is not implemented yet.
- Runtime requires outbound HTTPS access to the Dairo API.

## Install

From the repository:

```sh
cargo install --path . --locked
```

Or run without installing:

```sh
cargo run --locked -- --help
```

No public binary release channel exists yet. Preview releases should be built
from a reviewed git commit with `cargo build --release --locked`; signed
multi-platform artifacts are planned before a public launch.

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

List and create webhooks:

```sh
dairo webhook list
dairo webhook create \
  --url https://example.com/dairo/webhook \
  --event message.received \
  --event email.delivered
```

The create command prints a one-time signing secret. Store it immediately.

Delete a webhook by ID or URL:

```sh
dairo webhook delete https://example.com/dairo/webhook
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
- SES sandbox errors: the backend may reject arbitrary recipients until Dairo's
  AWS SES production access is approved.

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
