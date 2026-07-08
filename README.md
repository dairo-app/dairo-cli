# Dairo CLI

The official command-line interface for Dairo — the agent-native messaging & storage API. Built for humans, AI agents, and CI/CD pipelines.

## Installation

```sh
brew install dairo-app/tap/dairo
npm install -g @dairo-app/cli
curl -fsSL https://dairo.app/install.sh | sh
```

Native binaries ship for macOS, Linux (glibc and static musl), and Windows; the
installers and the npm launcher pick the right build automatically. Update in
place with `dairo update`.

## Setup

Get an API key at [dairo.app](https://dairo.app), then sign in:

```sh
dairo login                       # browser OAuth (recommended)
export DAIRO_API_KEY="dairo_..."  # ...or set a key directly for CI / headless
```

## Example

Send a message:

```sh
dairo send \
  --from you@yourdomain.com \
  --to jane@example.com \
  --subject "Hello from Dairo" \
  --text "Sent with the Dairo CLI."
```

Share a stored file with a signed link:

```sh
dairo share create --bucket <bucket> --object <object-id>
```

## Commands

Run `dairo --help` for the full grouped command list, or `dairo <command> --help`
for any command's options. Highlights:

- `send`, `messages`, `threads`, `inbox` — send and read mail on the unified inbox
- `domain`, `api-key`, `webhook` — manage sending domains, keys, and webhooks
- `bucket`, `share`, `attachment` — object storage and signed share links
- `audience` — broadcast to a saved audience
- `letter` — send and track physical mail
- `listen` — tail live inbox events (ideal for local webhook development)
- `mcp` — install the Dairo MCP server into your coding agent
- `login`, `whoami`, `init`, `doctor`, `update` — auth, scaffolding, and maintenance

## Documentation

Full documentation lives at [dairo.app](https://dairo.app), with the complete
API and CLI reference at [docs.dairo.app](https://docs.dairo.app).

## License

MIT
