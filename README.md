# Dairo CLI

Official Dairo command-line interface.

Status: initial Rust implementation.

## Install

```sh
cargo install --path .
```

Or run from the repository:

```sh
cargo run -- --help
```

## Authentication

The CLI authenticates with a Dairo API key using bearer auth.

Token lookup order:

1. `DAIRO_API_KEY`
2. local config file set by `dairo auth token set`

Save a token locally:

```sh
printf '%s' "$DAIRO_API_KEY" | dairo auth token set
```

You can also pass the token as an argument:

```sh
dairo auth token set dairo_test_...
```

The config file is written under the platform config directory, for example
`~/.config/dairo/config.toml` on Linux.

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

Send an email:

```sh
dairo send \
  --inbox-id 018f0000-0000-0000-0000-000000000000 \
  --to max@example.com \
  --subject "Hello from Dairo" \
  --text "This was sent with the Dairo CLI."
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

Use `--json` for machine-readable output where supported:

```sh
dairo --json domain list
```

## Development

```sh
cargo fmt --check
cargo test
```

License: MIT
