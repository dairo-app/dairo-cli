# Changelog

All notable Dairo CLI private-preview changes are tracked here.

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
