# Contributing to the Dairo CLI

Thanks for helping improve the Dairo CLI. This repository is in **private
preview** (see [README](README.md)); breaking command/output changes can still
happen, but every change should keep the documented behavior working and the
contract tests green.

## Prerequisites

- Rust **1.85+** (the crate sets `rust-version = "1.85"`).
- No system C dependencies are required: the keychain backends are pure-Rust
  (macOS Keychain, Windows Credential Manager, Linux keyutils) and TLS is
  `rustls`, so Linux/CI builds need no `libdbus`/OpenSSL.

## Build, test, lint

The same checks CI runs (`.github/workflows/ci.yml`):

```sh
cargo fmt --check
cargo clippy --locked --all-targets --all-features -- -D warnings
cargo test --locked --all-features
cargo build --release --locked
cargo deny --locked check
cargo audit --file Cargo.lock
```

Run the CLI locally without installing:

```sh
cargo run --locked -- --help
cargo run --locked -- send --from you@example.com --to a@b.com --text Hi --dry-run
```

## Project layout

```
src/
  main.rs            Entry point + the top-level command dispatch (`run()` match).
  cli.rs             clap definitions: every command, subcommand, and flag.
  api/               HTTP client and request/response models for the Dairo API.
  auth.rs            Browser OAuth (PKCE) login for `dairo login`.
  config.rs          Token resolution + on-disk/keychain credential storage.
  output.rs          Human/JSON output formatting and the JSON error envelope.
  init/              `dairo init` scaffolder + embedded framework templates.
  listen.rs          `dairo listen` event streaming.
  doctor.rs          `dairo doctor` local health check.
  mcp_install.rs     `dairo mcp install` client configuration.
  update.rs          `dairo update` release check.
  webhook.rs         Offline webhook signature verification.
tests/
  cli_contract.rs    End-to-end CLI behavior (auth errors, init, JSON envelope).
  contract_projection.rs  Keeps the CLI surface aligned with the API contract.
contract/            Canonical vs implemented operation snapshots.
```

## Adding or changing a command

1. Define it in `src/cli.rs` (a `Command` variant and, usually, a subcommand
   enum + an `Args` struct). Give every command and flag a **short** doc comment
   — clap turns the first line into the one-line summary shown in `--help`, so
   keep that first line to one tight sentence and put detail on later lines.
2. Wire it into the dispatch `match` in `src/main.rs` (`run()`), delegating to an
   `api` method or a module.
3. **Update the curated top-level help.** `dairo --help` renders a hand-grouped
   list from `ROOT_HELP_COMMANDS` in `src/cli.rs`, not clap's auto list. Add the
   new command to the right group. The `root_help_lists_every_command` unit test
   fails if you forget.
4. Add or update a test in `tests/` (and a `contract/` snapshot if the command
   maps to a public API operation).
5. Add a `CHANGELOG.md` entry under `## Unreleased`.

## Conventions

- **JSON support:** honor the global `--json` flag wherever it makes sense.
  Success goes to stdout; failures are the stable `{ "error": { ... } }` envelope
  on stderr (see `src/output.rs`).
- **Never echo secrets.** Tokens are read from stdin or the environment, never
  positional args, and rejected tokens must not be printed back (there are tests
  enforcing this).
- **Mailbox collections are plural** (`messages`, `threads`, `attachments`) with
  singular aliases kept for compatibility.
- Keep diffs `cargo fmt`-clean and clippy-clean (`-D warnings`).

## Reporting issues

While this is in preview, use the private `dairo-app/dairo-cli` repository for
issues and questions.
