use anyhow::{Context, Result};
use clap::{ArgGroup, Args, Parser, Subcommand, ValueEnum};
use std::{
    io::{self, Read},
    path::PathBuf,
};

#[derive(Debug, Parser)]
#[command(
    name = "dairo",
    version,
    about = "Official Dairo command-line interface"
)]
pub struct Cli {
    /// Print machine-readable JSON where supported.
    #[arg(long, global = true)]
    pub json: bool,

    /// Override the Dairo API base URL.
    #[arg(long, global = true, hide = true, env = "DAIRO_API_URL")]
    pub api_url: Option<String>,

    #[command(subcommand)]
    pub command: Command,
}

#[derive(Debug, Subcommand)]
pub enum Command {
    /// Manage local authentication.
    Auth {
        #[command(subcommand)]
        command: AuthCommand,
    },
    /// Sign in with your browser (OAuth) and store a scoped API token.
    ///
    /// Runs the same Authorization-Code + PKCE flow the Dairo MCP clients use: a
    /// loopback callback listener is bound on `127.0.0.1`, your browser is opened
    /// to the Dairo authorize page, and the returned code is exchanged for a
    /// `dairo_live_*` API key saved to the local config. The manual key path
    /// (`dairo auth token set`) keeps working unchanged.
    Login(LoginArgs),
    /// Revoke the stored OAuth token server-side and clear it from local config.
    Logout,
    /// Show authenticated account, API key scopes, plan, and storage usage.
    Whoami,
    /// Manage account domains.
    Domain {
        #[command(subcommand)]
        command: DomainCommand,
    },
    /// Manage inboxes.
    Inbox {
        #[command(subcommand)]
        command: InboxCommand,
    },
    /// Inspect mailbox messages.
    #[command(name = "messages", alias = "message")]
    Message {
        #[command(subcommand)]
        command: MessageCommand,
    },
    /// Download message attachments.
    #[command(name = "attachments", alias = "attachment")]
    Attachment {
        #[command(subcommand)]
        command: AttachmentCommand,
    },
    /// Inspect mailbox threads.
    #[command(name = "threads", alias = "thread")]
    Thread {
        #[command(subcommand)]
        command: ThreadCommand,
    },
    /// Manage webhook subscriptions.
    Webhook {
        #[command(subcommand)]
        command: WebhookCommand,
    },
    /// Manage API keys.
    #[command(name = "api-key")]
    ApiKey {
        #[command(subcommand)]
        command: ApiKeyCommand,
    },
    /// Install Dairo MCP for coding agents.
    Mcp {
        #[command(subcommand)]
        command: McpCommand,
    },
    /// Send an email from a Dairo inbox.
    Send(SendArgs),
    /// Inspect outbound email history and delivery events.
    Outbound {
        #[command(subcommand)]
        command: OutboundCommand,
    },
    /// Manage email lists and send to list recipients.
    #[command(name = "lists", alias = "list")]
    EmailList {
        #[command(subcommand)]
        command: EmailListCommand,
    },
    /// Inspect the account audit log (security-relevant control-plane actions).
    #[command(name = "audit-logs", alias = "audit-log")]
    AuditLog {
        #[command(subcommand)]
        command: AuditLogCommand,
    },
    /// Inspect dedicated IP pool status.
    #[command(name = "dedicated-ips", alias = "dedicated-ip")]
    DedicatedIp {
        #[command(subcommand)]
        command: DedicatedIpCommand,
    },
    /// Manage reusable email templates (named container + immutable versions).
    #[command(name = "templates", alias = "template")]
    Template {
        #[command(subcommand)]
        command: TemplateCommand,
    },
    /// Read the durable event ledger and replay it to your webhooks.
    #[command(name = "events", alias = "event")]
    Events {
        #[command(subcommand)]
        command: EventsCommand,
    },
    /// Inspect agent passports and verify outbound provenance.
    #[command(name = "agents", alias = "agent")]
    Agent {
        #[command(subcommand)]
        command: AgentCommand,
    },
    /// View per-agent reputation / circuit-breaker state.
    Reputation {
        #[command(subcommand)]
        command: ReputationCommand,
    },
    /// Inspect and set per-account/key/agent send budgets.
    #[command(name = "budgets", alias = "budget")]
    Budget {
        #[command(subcommand)]
        command: BudgetCommand,
    },
    /// EU data-residency posture and subject-erasure job status.
    Compliance {
        #[command(subcommand)]
        command: ComplianceCommand,
    },
    /// Enqueue and inspect GDPR subject-erasure / inbox-purge jobs.
    #[command(name = "erasure-jobs", alias = "erasure-job")]
    ErasureJobs {
        #[command(subcommand)]
        command: ErasureJobCommand,
    },
    /// Inspect agent-to-agent (A2A) cross-tenant hop receipts.
    #[command(name = "a2a")]
    A2a {
        #[command(subcommand)]
        command: A2aCommand,
    },
    /// Stream live inbound-email (and delivery) events to the terminal and,
    /// optionally, re-POST each one to a local endpoint — the Dairo equivalent of
    /// `stripe listen`. Pulls from the durable event ledger via long-poll, so no
    /// public webhook URL or tunnel is needed; a persisted cursor resumes exactly
    /// where it left off. Requires the `events:read` scope (plus `inboxes:read`
    /// when filtering by an inbox address).
    Listen(ListenArgs),
    /// Scaffold a working Dairo starter into your project for a framework.
    ///
    /// Generates a configured SDK client, an inbound-webhook handler stub (raw
    /// body + signature verification using the SDK's own verify helper),
    /// `DAIRO_API_KEY` env wiring, and a README snippet — the "0-to-first-send +
    /// first-inbound" on-ramp. Templates are embedded in the binary, so it works
    /// offline; the only optional network touch is a friendly `GET /v1/whoami`
    /// connectivity check after scaffolding (skip with `--no-verify`).
    Init(InitArgs),
}

#[derive(Debug, Subcommand)]
pub enum AuditLogCommand {
    /// List audit-log entries (newest first) with keyset pagination.
    List {
        /// Maximum number of entries to return (1..=100; server default 25).
        #[arg(long, value_parser = clap::value_parser!(u32).range(1..=100))]
        limit: Option<u32>,
        /// Opaque pagination cursor from a previous page's `nextCursor`.
        #[arg(long)]
        cursor: Option<String>,
    },
}

#[derive(Debug, Subcommand)]
pub enum DedicatedIpCommand {
    /// Show the dedicated IP pool status for the account.
    Status,
}

#[derive(Debug, Subcommand)]
pub enum TemplateCommand {
    /// List active templates (scope `templates:read`).
    List,
    /// Create a template and publish v1 (scope `templates:write`).
    ///
    /// The source is read inline with `--source` or from a file with
    /// `--source-file` (exactly one is required). The source is dry-rendered at
    /// publish, so a broken template fails fast.
    Create {
        /// URL-safe slug used to reference the template at send time.
        #[arg(long)]
        slug: String,
        /// Human-readable template name.
        #[arg(long)]
        name: String,
        /// Optional description.
        #[arg(long)]
        description: Option<String>,
        /// React-email source for v1 (mutually exclusive with --source-file).
        #[arg(long, conflicts_with = "source_file")]
        source: Option<String>,
        /// Read the React-email source for v1 from this file.
        #[arg(long = "source-file", value_name = "PATH")]
        source_file: Option<PathBuf>,
        /// Optional default subject line.
        #[arg(long)]
        subject: Option<String>,
        /// JSON-Schema-lite variable contract (a JSON object) validated at send time.
        #[arg(long, value_name = "JSON")]
        variables: Option<String>,
        /// Optional free-text notes recorded on v1.
        #[arg(long)]
        notes: Option<String>,
    },
    /// Get a template plus a resolved version (with source).
    Get {
        id_or_slug: String,
        /// Pin a specific version instead of the container's `currentVersion`.
        #[arg(long)]
        version: Option<u32>,
    },
    /// Update template metadata or re-point `currentVersion` (scope `templates:write`).
    ///
    /// The source is immutable — publish a new version to change it.
    Update {
        id_or_slug: String,
        /// New human-readable name.
        #[arg(long)]
        name: Option<String>,
        /// New description. Pass an empty string to clear it.
        #[arg(long)]
        description: Option<String>,
        /// Re-point the mutable current-version pointer to roll back/forward.
        #[arg(long = "current-version")]
        current_version: Option<u32>,
    },
    /// Archive a template (scope `templates:write`).
    Delete { id_or_slug: String },
    /// List a template's versions, newest first (no source).
    Versions { id_or_slug: String },
    /// Read one version of a template, including its source.
    Version { id_or_slug: String, version: u32 },
    /// Publish a new immutable version (scope `templates:write`).
    Publish {
        id_or_slug: String,
        /// React-email source for the new version (mutually exclusive with --source-file).
        #[arg(long, conflicts_with = "source_file")]
        source: Option<String>,
        /// Read the new version's React-email source from this file.
        #[arg(long = "source-file", value_name = "PATH")]
        source_file: Option<PathBuf>,
        /// Optional subject line for the new version.
        #[arg(long)]
        subject: Option<String>,
        /// JSON-Schema-lite variable contract (a JSON object) for the new version.
        #[arg(long, value_name = "JSON")]
        variables: Option<String>,
        /// Publish as a draft instead of advancing `currentVersion`.
        #[arg(long = "no-promote")]
        no_promote: bool,
        /// Optional free-text notes recorded on the version.
        #[arg(long)]
        notes: Option<String>,
    },
}

#[derive(Debug, Subcommand)]
pub enum EventsCommand {
    /// Read a keyset-paginated slice of the durable event ledger.
    List {
        /// Max rows to return (1..=100; server default 50).
        #[arg(long, value_parser = clap::value_parser!(u32).range(1..=100))]
        limit: Option<u32>,
        /// Opaque keyset cursor from a prior page's `pagination.nextCursor`.
        #[arg(long)]
        cursor: Option<String>,
        /// Restrict the stream to one inbox (id).
        #[arg(long = "inbox-id")]
        inbox_id: Option<String>,
        /// Filter to a single event type (e.g. `message.received`).
        #[arg(long = "type")]
        event_type: Option<String>,
        /// Server-side long-poll hold in seconds (0..=25). 0 returns immediately.
        #[arg(long, value_parser = clap::value_parser!(u8).range(0..=25))]
        wait: Option<u8>,
        /// Return `events: []` plus the current head cursor (start-streaming bootstrap).
        #[arg(long)]
        tail: bool,
    },
    /// Re-deliver a ledger slice to your webhooks (scope `events:write`).
    ///
    /// Provide exactly one lower bound: `--since` (a cursor), `--since-seq`
    /// (with `--inbox-id`), or `--since-timestamp`.
    Replay {
        /// Keyset cursor lower bound.
        #[arg(long)]
        since: Option<String>,
        /// Per-partition seq lower bound; requires `--inbox-id`.
        #[arg(long = "since-seq")]
        since_seq: Option<i64>,
        /// RFC3339 lower bound on `createdAt`.
        #[arg(long = "since-timestamp")]
        since_timestamp: Option<String>,
        /// Optional inbox filter (also scopes the partition when --since-seq is set).
        #[arg(long = "inbox-id")]
        inbox_id: Option<String>,
        /// Optional RFC3339 upper bound on `createdAt`.
        #[arg(long)]
        until: Option<String>,
        /// Restrict to these event types. Repeat for several.
        #[arg(long = "type", value_name = "TYPE", action = clap::ArgAction::Append)]
        types: Vec<String>,
        /// Replay to a single webhook id; omit to replay to every active subscription.
        #[arg(long = "webhook-id")]
        webhook_id: Option<String>,
        /// Cap on the replayed slice (1..=5000; server default 1000).
        #[arg(long = "max-events", value_parser = clap::value_parser!(u32).range(1..=5000))]
        max_events: Option<u32>,
    },
}

#[derive(Debug, Subcommand)]
pub enum AgentCommand {
    /// List the caller's agent passports, newest first (scope `agents:read`).
    List,
    /// Get a passport by its uuid id or portable `agt_…` agentId.
    Get { id_or_agent: String },
    /// Verify an agent's outbound attribution (public; always returns a verdict).
    ///
    /// Pass `--id <messageId>` to attest from an outbound record, or the
    /// signature form (`--agent --kid --sig`, with optional signed fields) to
    /// verify a reconstructed provenance signature.
    Verify {
        /// Verify from a stored outbound message id (mutually exclusive with --agent).
        #[arg(long, conflicts_with = "agent")]
        id: Option<String>,
        /// Portable `agt_…` agent id (signature form; requires --kid and --sig).
        #[arg(long, requires = "kid", requires = "sig")]
        agent: Option<String>,
        /// Signing key id (`kid`) for the signature form.
        #[arg(long)]
        kid: Option<String>,
        /// The provenance signature to verify.
        #[arg(long)]
        sig: Option<String>,
        /// Signed `from` address.
        #[arg(long)]
        from: Option<String>,
        /// Signed recipients, comma-joined, matching the signed `to` field.
        #[arg(long)]
        to: Option<String>,
        /// Signed subject.
        #[arg(long)]
        subject: Option<String>,
        /// Signed timestamp.
        #[arg(long)]
        ts: Option<String>,
    },
}

#[derive(Debug, Subcommand)]
pub enum ReputationCommand {
    /// Fleet view of every agent's circuit-breaker state.
    List,
}

#[derive(Debug, Subcommand)]
pub enum BudgetCommand {
    /// List every budget with its live windowed usage (scope `budgets:read`).
    List,
    /// Get a single budget by scope (`account`, or a key/agent `scopeId`).
    Get { scope: String },
    /// Set/replace a budget (scope `budgets:write`). Idempotent upsert on (scope, scopeId).
    ///
    /// At least one enforceable limit is required.
    Set {
        /// Budget scope: `account`, `key`, or `agent`.
        #[arg(long)]
        scope: String,
        /// Required for `key`/`agent`; must be omitted for `account`.
        #[arg(long = "scope-id")]
        scope_id: Option<String>,
        /// Disable the budget (it is enabled by default).
        #[arg(long)]
        disabled: bool,
        /// Maximum sends per rolling day.
        #[arg(long = "max-sends-per-day")]
        max_sends_per_day: Option<u64>,
        /// Maximum new recipients per rolling hour.
        #[arg(long = "max-new-recipients-per-hour")]
        max_new_recipients_per_hour: Option<u64>,
        /// Hard-stop all sends once any complaint is recorded.
        #[arg(long = "hard-stop-on-complaint")]
        hard_stop_on_complaint: bool,
    },
    /// Delete a budget by scope (scope `budgets:write`).
    ///
    /// Replaces the old disable-as-delete; returns 204 on success.
    Delete { scope: String },
}

#[derive(Debug, Subcommand)]
pub enum ComplianceCommand {
    /// Read the data-residency / subprocessor posture (with CLOUD-Act note).
    Residency,
    /// Poll a subject-erasure job (tallies + signed certificate once complete).
    ///
    /// Alias of `dairo erasure-jobs get`; kept for backward compatibility.
    #[command(name = "erasure-job", alias = "erasure-jobs")]
    ErasureJob { job_id: String },
}

/// Subject-erasure / inbox-purge jobs (scope `compliance:read` to read,
/// `compliance:write` to enqueue). The `/v1/compliance/*` junk-drawer was
/// replaced by this real `/v1/erasure-jobs` resource.
#[derive(Debug, Subcommand)]
pub enum ErasureJobCommand {
    /// List erasure jobs, newest first (scope `compliance:read`).
    List,
    /// Enqueue a GDPR erasure job (scope `compliance:write`).
    ///
    /// Provide exactly one of `--subject-email` (erase a data subject across all
    /// stored mail) or `--inbox-id` (purge an inbox).
    Create {
        /// Erase this data subject's mail across the account.
        #[arg(long = "subject-email", conflicts_with = "inbox_id")]
        subject_email: Option<String>,
        /// Purge this inbox.
        #[arg(long = "inbox-id")]
        inbox_id: Option<String>,
    },
    /// Poll a job (tallies + signed certificate once complete; scope `compliance:read`).
    Get { job_id: String },
}

#[derive(Debug, Subcommand)]
pub enum A2aCommand {
    /// List agent-to-agent hop receipts with keyset pagination.
    List {
        /// Max rows to return (1..=100; server default 50).
        #[arg(long, value_parser = clap::value_parser!(u32).range(1..=100))]
        limit: Option<u32>,
        /// Opaque keyset cursor from a prior page's `pagination.nextCursor`.
        #[arg(long)]
        cursor: Option<String>,
        /// Match either the sender or recipient inbox of the hop.
        #[arg(long = "inbox-id")]
        inbox_id: Option<String>,
    },
    /// Get a single A2A hop receipt.
    Get { id: String },
}

#[derive(Debug, Subcommand)]
pub enum OutboundCommand {
    /// List outbound emails (most recent first).
    List {
        #[arg(long)]
        limit: Option<u32>,
    },
    /// Get one outbound email with its delivery-event timeline.
    Get { email_id: String },
    /// Cancel a scheduled outbound email before its fire time.
    ///
    /// Fails with a conflict if the email is no longer scheduled (already sent,
    /// queued, or canceled).
    Cancel { email_id: String },
    /// List the delivery-event timeline for one outbound email
    /// (delivered, bounced, complained, ...).
    ///
    /// Events are now a per-email sub-resource (`GET /v1/emails/{id}/events`),
    /// so `--email-id` is required.
    Events {
        #[arg(long = "email-id")]
        email_id: String,
        #[arg(long)]
        limit: Option<u32>,
    },
    /// List only the bounce events for one outbound email.
    Bounces {
        #[arg(long = "email-id")]
        email_id: String,
        #[arg(long)]
        limit: Option<u32>,
    },
    /// List only the complaint events (recipients who reported spam) for one
    /// outbound email.
    Complaints {
        #[arg(long = "email-id")]
        email_id: String,
        #[arg(long)]
        limit: Option<u32>,
    },
}

#[derive(Debug, Args)]
#[command(group(
    // The sending inbox: exactly one of a UUID (--inbox-id) or an address (--from).
    ArgGroup::new("source")
        .required(true)
        .multiple(false)
        .args(["inbox_id", "from"])
))]
#[command(group(
    ArgGroup::new("body")
        .required(true)
        .multiple(true)
        .args(["text", "text_file", "html", "html_file", "react_source"])
))]
pub struct SendArgs {
    /// Sending inbox by UUID. For readability, prefer `--from <address>`.
    #[arg(long = "inbox-id", value_name = "UUID")]
    pub inbox_id: Option<String>,
    /// Sending inbox by ADDRESS, e.g. `agent@dairo.app` (or `Name <agent@dairo.app>`).
    /// Resolved to the inbox id via your inboxes (needs the `inboxes:read` scope).
    /// Alias: `--inbox`.
    #[arg(long = "from", visible_alias = "inbox", value_name = "ADDRESS")]
    pub from: Option<String>,
    #[arg(long, required = true, action = clap::ArgAction::Append)]
    pub to: Vec<String>,
    /// CC recipient(s). Repeatable.
    #[arg(long = "cc", value_name = "ADDRESS", action = clap::ArgAction::Append)]
    pub cc: Vec<String>,
    /// BCC recipient(s). Repeatable.
    #[arg(long = "bcc", value_name = "ADDRESS", action = clap::ArgAction::Append)]
    pub bcc: Vec<String>,
    #[arg(long, default_value = "")]
    pub subject: String,
    #[arg(long)]
    pub text: Option<String>,
    /// Read the plain-text body from a file (`-` for stdin).
    #[arg(long = "text-file", value_name = "PATH")]
    pub text_file: Option<PathBuf>,
    #[arg(long)]
    pub html: Option<String>,
    /// Read the HTML body from a file (`-` for stdin).
    #[arg(long = "html-file", value_name = "PATH")]
    pub html_file: Option<PathBuf>,
    /// Hosted React component source rendered by Dairo before sending.
    #[arg(long = "react-source", value_name = "SOURCE")]
    pub react_source: Option<String>,
    /// JSON object passed to the hosted React component as props.
    #[arg(long = "react-props", value_name = "JSON", requires = "react_source")]
    pub react_props: Option<String>,
    #[arg(long = "attachment", value_name = "PATH", action = clap::ArgAction::Append)]
    pub attachments: Vec<PathBuf>,
    /// Attachment delivery mode. `auto` keeps files inline when safely below Dairo's inline limit.
    #[arg(long = "attachment-delivery", default_value_t = AttachmentDelivery::Attachment)]
    pub attachment_delivery: AttachmentDelivery,
    /// Requested link expiry in hours for future local file-link delivery. Valid range: 1..168.
    #[arg(
        long = "attachment-link-expiry-hours",
        value_parser = clap::value_parser!(u32).range(1..=168)
    )]
    pub attachment_link_expiry_hours: Option<u32>,
    /// Override complaint suppression. Use only when you intentionally want to contact recipients that previously complained.
    #[arg(long = "ignore-complaints")]
    pub ignore_complaints: bool,
    /// Schedule the send for a future time instead of sending immediately.
    /// RFC3339 with an explicit timezone offset, e.g. `2026-06-11T09:00:00Z` or
    /// `2026-06-11T11:00:00+02:00`. The response status is `scheduled`.
    #[arg(long = "send-at", value_name = "RFC3339")]
    pub send_at: Option<String>,
}

#[derive(Debug, Args)]
pub struct ListenArgs {
    /// Local endpoint to POST each event to. Loopback URLs
    /// (`http://localhost`, `http://127.0.0.1`, `http://[::1]`) are allowed —
    /// this is your own machine. Plain `http://` to a non-loopback host is
    /// rejected (use `https://` for a remote/staging target). When omitted,
    /// `dairo listen` only prints events ("tail my inbox events live").
    #[arg(long = "forward-to", value_name = "URL")]
    pub forward_to: Option<String>,
    /// Restrict the stream to one or more inboxes, by inbox id or address.
    /// Repeat for several. A single value is pushed to the server `inboxId`
    /// filter; multiple values stream the unfiltered account-wide event stream
    /// (one monotonic cursor) and are filtered client-side.
    #[arg(long = "inbox", value_name = "ID_OR_ADDRESS", action = clap::ArgAction::Append)]
    pub inbox: Vec<String>,
    /// Event-type filter. Repeat for several. Exact types (e.g.
    /// `message.received`) and `*`-globs (e.g. `message.*`) are supported;
    /// globs are matched client-side. Defaults to the inbound-sandbox set
    /// (`message.received`, `message.quarantined`). Pass `--events '*'` for
    /// everything, including outbound delivery events.
    #[arg(long = "events", value_name = "GLOB", action = clap::ArgAction::Append)]
    pub events: Vec<String>,
    /// Terminal rendering for each event. `compact` is a one-line human log;
    /// `json` prints each raw event as one JSON line (pipe-friendly); `pretty`
    /// is multi-line.
    #[arg(long = "print", value_name = "MODE", default_value_t = PrintMode::Compact)]
    pub print: PrintMode,
    /// Start from history instead of "now". `--replay 50` replays the last 50
    /// events; `--replay all` replays from the beginning; `--replay 1h` replays
    /// events from the last hour (also accepts `30m`, `2d`). Default (unset)
    /// starts strictly after the newest existing event.
    #[arg(long = "replay", value_name = "N|all|DURATION")]
    pub replay: Option<String>,
    /// Where the resume cursor is persisted (written `0600`). Defaults to a
    /// per-key, per-filter file under the user config dir so two concurrent
    /// listens never clobber each other's cursor.
    #[arg(long = "state-file", value_name = "PATH")]
    pub state_file: Option<PathBuf>,
    /// Ignore any persisted cursor and start fresh from tail (or `--replay`).
    #[arg(long = "no-resume")]
    pub no_resume: bool,
    /// Long-poll hold time per request, in seconds (1..=25). Lower = snappier
    /// shutdown and more requests; higher = fewer requests while idle.
    #[arg(long = "wait", value_name = "SECONDS", default_value_t = 25, value_parser = clap::value_parser!(u8).range(1..=25))]
    pub wait: u8,
    /// Per-event forward retry budget before logging-and-skipping. A bad local
    /// handler can never wedge the stream forever.
    #[arg(long = "max-forward-retries", value_name = "N", default_value_t = 5)]
    pub max_forward_retries: u8,
    /// Disable ephemeral HMAC signing of forwarded events. By default each run
    /// mints a fresh signing secret, prints it once, and signs forwards so a
    /// handler can verify with `DAIRO_WEBHOOK_SECRET=<that>`.
    #[arg(long = "no-sign")]
    pub no_sign: bool,
}

/// Terminal rendering mode for `dairo listen` per-event output. Independent of
/// the global `--json` flag (which governs only the startup banner / error
/// envelope).
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum PrintMode {
    Compact,
    Json,
    Pretty,
}

impl std::fmt::Display for PrintMode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Compact => "compact",
            Self::Json => "json",
            Self::Pretty => "pretty",
        })
    }
}

#[derive(Debug, Args)]
pub struct InitArgs {
    /// Target framework. Omit to see the valid values. One of: `next`,
    /// `express`, `hono`, `cloudflare-workers`, `fastapi`, `flask`, `go-http`.
    pub framework: Option<Framework>,
    /// Explicit alias for the positional framework, for scriptability. If both
    /// the positional and this flag are given and they disagree, `init` errors.
    #[arg(long = "framework", value_name = "FRAMEWORK")]
    pub framework_flag: Option<Framework>,
    /// Target project directory. Created if missing. Files are only ever written
    /// inside this directory.
    #[arg(long, default_value = ".")]
    pub dir: PathBuf,
    /// Overwrite files that already exist. Without it, `init` never clobbers an
    /// existing file (it skips and warns), so re-running is safe and idempotent.
    #[arg(long)]
    pub force: bool,
    /// Skip running the package-manager install step; only write files and print
    /// the manual install command.
    #[arg(long = "no-install")]
    pub no_install: bool,
    /// Override the auto-detected package manager: `npm`/`pnpm`/`yarn`/`bun` for
    /// JS, `pip`/`poetry`/`uv` for Python, `go` for Go.
    #[arg(long = "package-manager", value_name = "PM")]
    pub package_manager: Option<String>,
    /// URL path the webhook handler is mounted at, echoed into the README so you
    /// know what to register with `dairo webhook create --url`. Defaults per
    /// framework (e.g. `/api/dairo/webhook`).
    #[arg(long = "inbox-route", value_name = "PATH")]
    pub inbox_route: Option<String>,
    /// Skip the post-scaffold `GET /v1/whoami` connectivity check. The check is
    /// also auto-skipped when no API key is configured.
    #[arg(long = "no-verify")]
    pub no_verify: bool,
}

/// Tier-1 frameworks `dairo init` can scaffold. The first wave covers the four
/// transport shapes (Node serverful, Node edge, Python ASGI/WSGI, Go net/http)
/// across the JS/TS, Python, and Go SDKs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum Framework {
    /// Next.js App Router (TypeScript) — `dairo` (npm).
    Next,
    /// Express (Node, TypeScript) — `dairo` (npm).
    Express,
    /// Hono (edge/Node) — `dairo` (npm).
    Hono,
    /// Cloudflare Workers (Web Crypto, no Node APIs) — `dairo` (npm).
    #[value(name = "cloudflare-workers")]
    CloudflareWorkers,
    /// FastAPI (Python ASGI) — `dairo` (PyPI).
    Fastapi,
    /// Flask (Python WSGI) — `dairo` (PyPI).
    Flask,
    /// Go `net/http` — `github.com/dairo-app/dairo-go`.
    #[value(name = "go-http")]
    GoHttp,
}

impl Framework {
    /// The canonical `--framework` value, matching the `ValueEnum` spelling.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Next => "next",
            Self::Express => "express",
            Self::Hono => "hono",
            Self::CloudflareWorkers => "cloudflare-workers",
            Self::Fastapi => "fastapi",
            Self::Flask => "flask",
            Self::GoHttp => "go-http",
        }
    }
}

impl std::fmt::Display for Framework {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

// SendArgs is `#[command(flatten)]`-ed into the `Send` variant, which clap
// requires to be the concrete Args type (it cannot be boxed), so the size
// difference between variants is inherent and benign.
#[allow(clippy::large_enum_variant)]
#[derive(Debug, Subcommand)]
pub enum EmailListCommand {
    /// List email lists.
    List,
    /// Create an email list.
    Create {
        name: String,
        #[arg(long)]
        description: Option<String>,
    },
    /// Show list members.
    Get { list_id: String },
    /// Delete (archive) an email list.
    Delete { list_id: String },
    /// Add one recipient manually.
    Add {
        list_id: String,
        #[arg(long)]
        email: String,
        #[arg(long)]
        name: Option<String>,
    },
    /// Import recipients from CSV. Reads first column as email and optional second column as name.
    ImportCsv {
        list_id: String,
        #[arg(long = "file")]
        file: PathBuf,
    },
    /// Send an email to all active recipients in a list.
    Send {
        list_id: String,
        #[command(flatten)]
        send: SendArgs,
    },
}

#[derive(Debug, Subcommand)]
pub enum McpCommand {
    /// Install Dairo MCP into a supported coding-agent client config.
    Install {
        /// Target client. `auto` configures Hermes, Codex, Cursor, and a project .mcp.json for Claude.
        #[arg(long, default_value_t = McpClient::Auto)]
        client: McpClient,
        /// MCP server name in the target client.
        #[arg(long, default_value = "dairo")]
        name: String,
    },
    /// Print the Dairo MCP tool catalog (from the hosted /v1/mcp/catalog).
    Catalog {
        /// Print the raw catalog JSON instead of a table.
        #[arg(long)]
        json: bool,
        /// Annotate each tool with whether the active API key can call it
        /// (requests `?for=me`) and show only the allowed tools.
        #[arg(long = "for-me")]
        for_me: bool,
        /// Show only tools in this family (e.g. `mail`, `outbound`, `agents`).
        #[arg(long)]
        family: Option<String>,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum McpClient {
    Auto,
    Hermes,
    Claude,
    Codex,
    Cursor,
}

impl std::fmt::Display for McpClient {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Auto => "auto",
            Self::Hermes => "hermes",
            Self::Claude => "claude",
            Self::Codex => "codex",
            Self::Cursor => "cursor",
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum AttachmentDelivery {
    Attachment,
    Link,
    Auto,
}

impl std::fmt::Display for AttachmentDelivery {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Attachment => "attachment",
            Self::Link => "link",
            Self::Auto => "auto",
        })
    }
}

#[derive(Debug, Args)]
pub struct LoginArgs {
    /// Scopes to request, space- or comma-separated. Defaults to the `admin`
    /// bundle, which the backend expands to every scope so the CLI is fully
    /// functional. Pass a narrower set (e.g. `--scope "mail:read mail:send"`) to
    /// mint a least-privilege token.
    #[arg(long = "scope", default_value = crate::auth::DEFAULT_LOGIN_SCOPE)]
    pub scope: String,
    /// Override the Dairo API base URL for the OAuth flow. Defaults to the global
    /// `--api-url` / `DAIRO_API_URL` / configured base, then the public API.
    #[arg(long = "api-url", value_name = "URL")]
    pub api_url: Option<String>,
}

#[derive(Debug, Subcommand)]
pub enum AuthCommand {
    /// Save a Dairo API token in the local config file.
    Token(TokenCommand),
}

#[derive(Debug, Args)]
pub struct TokenCommand {
    #[command(subcommand)]
    pub command: TokenSubcommand,
}

#[derive(Debug, Subcommand)]
pub enum TokenSubcommand {
    /// Save a Dairo API token in the local config file.
    Set(TokenSetArgs),
}

#[derive(Debug, Args)]
/// Reads token value from stdin only.
///
/// Positional token arguments are intentionally rejected so secrets do not land
/// in shell history or process listings.
pub struct TokenSetArgs {}

impl TokenCommand {
    pub fn token_value(self) -> Result<String> {
        match self.command {
            TokenSubcommand::Set(args) => args.token_value(),
        }
    }
}

impl TokenSetArgs {
    fn token_value(self) -> Result<String> {
        let mut token = String::new();
        io::stdin()
            .read_to_string(&mut token)
            .context("failed to read token from stdin")?;
        let trimmed = token.trim().to_string();
        anyhow::ensure!(!trimmed.is_empty(), "token cannot be empty");
        Ok(trimmed)
    }
}

#[derive(Debug, Subcommand)]
pub enum DomainCommand {
    /// List domains for the authenticated account.
    List,
    /// Create or return a domain and required DNS records.
    Add { domain: String },
    /// Recheck SES/DNS status for a domain.
    Recheck { domain: String },
    /// Delete a domain by name.
    Delete { domain: String },
}

#[derive(Debug, Subcommand)]
pub enum InboxCommand {
    /// List inboxes for the authenticated account.
    List,
    /// Create or return an inbox on a verified account domain.
    Create {
        username: String,
        #[arg(long)]
        domain: String,
    },
    /// Delete an inbox by ID.
    Delete { inbox_id: String },
    /// Manage the JSON extraction schema attached to an inbox.
    Schema {
        #[command(subcommand)]
        command: InboxSchemaCommand,
    },
    /// Register and inspect durable verification-code waits.
    #[command(name = "verification-waits", alias = "verification-wait")]
    VerificationWaits {
        #[command(subcommand)]
        command: VerificationWaitCommand,
    },
}

#[derive(Debug, Subcommand)]
pub enum InboxSchemaCommand {
    /// Get the schema attached to an inbox.
    Get { inbox: String },
    /// Attach or replace an inbox extraction schema.
    Set {
        inbox: String,
        /// JSON-Schema-lite object. Omit to clear to passthrough.
        #[arg(long, value_name = "JSON")]
        schema: Option<String>,
        /// Read the JSON-Schema-lite object from a file.
        #[arg(long = "schema-file", value_name = "PATH", conflicts_with = "schema")]
        schema_file: Option<PathBuf>,
        /// Validation failure behavior.
        #[arg(long, value_enum)]
        on_validation_error: Option<InboxSchemaValidationMode>,
        /// Optional extractor prompt context.
        #[arg(long)]
        extraction_hint: Option<String>,
    },
    /// Delete the schema attached to an inbox.
    Delete { inbox: String },
}

#[derive(Debug, Clone, Copy, ValueEnum)]
pub enum InboxSchemaValidationMode {
    Quarantine,
    Passthrough,
}

#[derive(Debug, Subcommand)]
pub enum VerificationWaitCommand {
    /// Register a new wait for an inbound verification code.
    Register {
        inbox: String,
        /// Wait lifetime in seconds (30..=1800).
        #[arg(long = "timeout-sec", value_parser = clap::value_parser!(u32).range(30..=1800))]
        timeout_sec: u32,
        /// Optional case-insensitive substring matched against the From address.
        #[arg(long = "from-hint")]
        from_hint: Option<String>,
        /// Optional regex with exactly one capture group for the code.
        #[arg(long)]
        pattern: Option<String>,
        /// Optional idempotency key for safe retries.
        #[arg(long = "idempotency-key")]
        idempotency_key: Option<String>,
    },
    /// List waits for an inbox.
    List { inbox: String },
    /// Get one wait.
    Get { inbox: String, wait_id: String },
    /// Cancel one wait.
    Cancel { inbox: String, wait_id: String },
}

#[derive(Debug, Subcommand)]
pub enum MessageCommand {
    /// List messages.
    List {
        #[arg(long = "inbox-id")]
        inbox_id: Option<String>,
        #[arg(long = "thread-id")]
        thread_id: Option<String>,
        #[arg(long)]
        direction: Option<String>,
        #[arg(long)]
        limit: Option<u32>,
        #[arg(long)]
        cursor: Option<String>,
    },
    /// Get a message by ID.
    Get { message_id: String },
    /// Download every attachment on a message into a directory.
    DownloadAttachments {
        message_id: String,
        #[arg(long, default_value = ".")]
        out: PathBuf,
    },
}

#[derive(Debug, Subcommand)]
pub enum AttachmentCommand {
    /// Print short-lived branded URLs for an attachment.
    Url {
        attachment_id: String,
        /// Expiry in hours. Defaults to about 5 minutes; maximum is 168 hours / one week.
        #[arg(long = "expiry-hours", value_parser = clap::value_parser!(u32).range(1..=168))]
        expiry_hours: Option<u32>,
    },
    /// Print a short-lived human share page URL.
    Share {
        attachment_id: String,
        /// Expiry in hours. Defaults to about 5 minutes; maximum is 168 hours / one week.
        #[arg(long = "expiry-hours", value_parser = clap::value_parser!(u32).range(1..=168))]
        expiry_hours: Option<u32>,
    },
    /// Download one attachment to a file or directory.
    Download {
        attachment_id: String,
        #[arg(long, default_value = ".")]
        out: PathBuf,
    },
}

#[derive(Debug, Subcommand)]
pub enum ThreadCommand {
    /// List threads.
    List {
        #[arg(long = "inbox-id")]
        inbox_id: Option<String>,
        #[arg(long)]
        limit: Option<u32>,
        #[arg(long)]
        cursor: Option<String>,
    },
    /// Get a thread by ID.
    Get { thread_id: String },
}

#[derive(Debug, Subcommand)]
pub enum WebhookCommand {
    /// List webhook subscriptions.
    List,
    /// Create a webhook subscription and print its one-time signing secret.
    Create {
        #[arg(long)]
        url: String,
        /// Event type to deliver. Repeat for multiple events.
        #[arg(long = "event", required = true)]
        events: Vec<WebhookEvent>,
    },
    /// Delete a webhook by ID or URL.
    Delete { webhook: String },
    /// Verify a received webhook delivery's signature (offline; no API call).
    ///
    /// Reads the raw request body from stdin and checks the signature and
    /// timestamp headers against the webhook signing secret.
    Verify {
        /// The webhook signing secret (`whsec_...`) returned at creation.
        /// Read from this flag or the DAIRO_WEBHOOK_SECRET env var.
        #[arg(long, env = "DAIRO_WEBHOOK_SECRET")]
        secret: String,
        /// Value of the `X-Dairo-Signature` header (`v1=<hex>`).
        #[arg(long)]
        signature: String,
        /// Value of the `X-Dairo-Timestamp` header (unix seconds).
        #[arg(long)]
        timestamp: String,
        /// Allowed clock skew in seconds. Use 0 to skip the freshness check.
        #[arg(long = "tolerance-seconds", default_value_t = 300)]
        tolerance_seconds: u64,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum WebhookEvent {
    #[value(name = "message.received")]
    MessageReceived,
    #[value(name = "email.sent")]
    EmailSent,
    #[value(name = "email.delivered")]
    EmailDelivered,
    #[value(name = "email.bounced")]
    EmailBounced,
    #[value(name = "email.complained")]
    EmailComplained,
}

impl WebhookEvent {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::MessageReceived => "message.received",
            Self::EmailSent => "email.sent",
            Self::EmailDelivered => "email.delivered",
            Self::EmailBounced => "email.bounced",
            Self::EmailComplained => "email.complained",
        }
    }
}

impl std::fmt::Display for WebhookEvent {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

#[derive(Debug, Subcommand)]
pub enum ApiKeyCommand {
    /// List API keys.
    List,
    /// Create an API key and print its one-time secret.
    Create {
        #[arg(long)]
        name: String,
        #[arg(long = "scope", required = true)]
        scopes: Vec<String>,
        /// Restrict the key to these source IPs / CIDR ranges. Repeat for
        /// multiple entries. Omit to allow the key from any IP.
        #[arg(long = "allowed-ip", value_name = "IP_OR_CIDR")]
        allowed_ips: Vec<String>,
    },
    /// Revoke an API key by ID.
    Revoke { api_key_id: String },
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::CommandFactory;

    #[test]
    fn cli_command_tree_is_valid() {
        Cli::command().debug_assert();
    }
    #[test]
    fn parses_send_arguments() {
        let cli = Cli::try_parse_from([
            "dairo",
            "send",
            "--inbox-id",
            "inbox_123",
            "--to",
            "max@example.com",
            "--subject",
            "Hello",
            "--text",
            "Body",
            "--attachment",
            "invoice.pdf",
        ])
        .unwrap();

        match cli.command {
            Command::Send(SendArgs {
                inbox_id,
                from,
                to,
                cc,
                bcc,
                subject,
                text,
                text_file,
                html,
                html_file,
                react_source,
                react_props,
                attachments,
                attachment_delivery,
                attachment_link_expiry_hours,
                ignore_complaints,
                send_at,
            }) => {
                assert_eq!(inbox_id.as_deref(), Some("inbox_123"));
                assert_eq!(from, None);
                assert!(cc.is_empty());
                assert!(bcc.is_empty());
                assert_eq!(text_file, None);
                assert_eq!(html_file, None);
                assert_eq!(to, vec!["max@example.com"]);
                assert_eq!(subject, "Hello");
                assert_eq!(text.as_deref(), Some("Body"));
                assert_eq!(html, None);
                assert_eq!(react_source, None);
                assert_eq!(react_props, None);
                assert_eq!(attachments, vec![PathBuf::from("invoice.pdf")]);
                assert_eq!(attachment_delivery, AttachmentDelivery::Attachment);
                assert_eq!(attachment_link_expiry_hours, None);
                assert!(!ignore_complaints);
                assert_eq!(send_at, None);
            }
            _ => panic!("expected send command"),
        }
    }

    #[test]
    fn parses_send_attachment_delivery_modes() {
        let cli = Cli::try_parse_from([
            "dairo",
            "send",
            "--inbox-id",
            "inbox_123",
            "--to",
            "max@example.com",
            "--text",
            "Body",
            "--attachment",
            "invoice.pdf",
            "--attachment-delivery",
            "auto",
            "--attachment-link-expiry-hours",
            "24",
        ])
        .unwrap();

        match cli.command {
            Command::Send(SendArgs {
                attachment_delivery,
                attachment_link_expiry_hours,
                ..
            }) => {
                assert_eq!(attachment_delivery, AttachmentDelivery::Auto);
                assert_eq!(attachment_link_expiry_hours, Some(24));
            }
            _ => panic!("expected send command"),
        }
    }

    #[test]
    fn parses_send_at_for_scheduling() {
        let cli = Cli::try_parse_from([
            "dairo",
            "send",
            "--inbox-id",
            "inbox_123",
            "--to",
            "max@example.com",
            "--text",
            "Body",
            "--send-at",
            "2026-06-11T09:00:00Z",
        ])
        .unwrap();

        match cli.command {
            Command::Send(SendArgs { send_at, .. }) => {
                assert_eq!(send_at.as_deref(), Some("2026-06-11T09:00:00Z"));
            }
            _ => panic!("expected send command"),
        }
    }

    #[test]
    fn parses_outbound_cancel_command() {
        let cli = Cli::parse_from(["dairo", "outbound", "cancel", "email_123"]);
        match cli.command {
            Command::Outbound {
                command: OutboundCommand::Cancel { email_id },
            } => assert_eq!(email_id, "email_123"),
            _ => panic!("expected outbound cancel command"),
        }
    }

    #[test]
    fn parses_audit_logs_list_command() {
        let cli = Cli::parse_from([
            "dairo",
            "audit-logs",
            "list",
            "--limit",
            "50",
            "--cursor",
            "abc",
        ]);
        match cli.command {
            Command::AuditLog {
                command: AuditLogCommand::List { limit, cursor },
            } => {
                assert_eq!(limit, Some(50));
                assert_eq!(cursor.as_deref(), Some("abc"));
            }
            _ => panic!("expected audit-logs list command"),
        }
    }

    #[test]
    fn parses_listen_with_defaults() {
        let cli = Cli::parse_from(["dairo", "listen"]);
        match cli.command {
            Command::Listen(ListenArgs {
                forward_to,
                inbox,
                events,
                print,
                replay,
                state_file,
                no_resume,
                wait,
                max_forward_retries,
                no_sign,
            }) => {
                assert_eq!(forward_to, None);
                assert!(inbox.is_empty());
                assert!(events.is_empty());
                assert_eq!(print, PrintMode::Compact);
                assert_eq!(replay, None);
                assert_eq!(state_file, None);
                assert!(!no_resume);
                assert_eq!(wait, 25);
                assert_eq!(max_forward_retries, 5);
                assert!(!no_sign);
            }
            _ => panic!("expected listen command"),
        }
    }

    #[test]
    fn parses_listen_with_all_flags() {
        let cli = Cli::parse_from([
            "dairo",
            "listen",
            "--forward-to",
            "http://localhost:3000/webhook",
            "--inbox",
            "agent@acme.dev",
            "--inbox",
            "inbox_123",
            "--events",
            "message.received",
            "--events",
            "*",
            "--print",
            "json",
            "--replay",
            "1h",
            "--state-file",
            "/tmp/listen.cursor",
            "--no-resume",
            "--wait",
            "10",
            "--max-forward-retries",
            "3",
            "--no-sign",
        ]);
        match cli.command {
            Command::Listen(args) => {
                assert_eq!(
                    args.forward_to.as_deref(),
                    Some("http://localhost:3000/webhook")
                );
                assert_eq!(args.inbox, vec!["agent@acme.dev", "inbox_123"]);
                assert_eq!(args.events, vec!["message.received", "*"]);
                assert_eq!(args.print, PrintMode::Json);
                assert_eq!(args.replay.as_deref(), Some("1h"));
                assert_eq!(args.state_file, Some(PathBuf::from("/tmp/listen.cursor")));
                assert!(args.no_resume);
                assert_eq!(args.wait, 10);
                assert_eq!(args.max_forward_retries, 3);
                assert!(args.no_sign);
            }
            _ => panic!("expected listen command"),
        }
    }

    #[test]
    fn listen_rejects_out_of_range_wait() {
        let error = Cli::try_parse_from(["dairo", "listen", "--wait", "26"])
            .expect_err("wait above 25 should fail clap validation");
        assert!(error.to_string().contains("26"));

        let error = Cli::try_parse_from(["dairo", "listen", "--wait", "0"])
            .expect_err("wait of 0 should fail clap validation");
        assert!(error.to_string().contains('0'));
    }

    #[test]
    fn listen_rejects_unknown_print_mode() {
        let error = Cli::try_parse_from(["dairo", "listen", "--print", "verbose"])
            .expect_err("unknown print mode should fail clap validation");
        let message = error.to_string();
        assert!(message.contains("verbose"));
        assert!(message.contains("compact"));
    }

    #[test]
    fn parses_dedicated_ips_status_command() {
        let cli = Cli::parse_from(["dairo", "dedicated-ips", "status"]);
        assert!(matches!(
            cli.command,
            Command::DedicatedIp {
                command: DedicatedIpCommand::Status
            }
        ));
    }

    #[test]
    fn parses_api_key_create_with_allowed_ips() {
        let cli = Cli::parse_from([
            "dairo",
            "api-key",
            "create",
            "--name",
            "scoped",
            "--scope",
            "mail:send",
            "--allowed-ip",
            "203.0.113.0/24",
            "--allowed-ip",
            "198.51.100.7",
        ]);
        match cli.command {
            Command::ApiKey {
                command:
                    ApiKeyCommand::Create {
                        allowed_ips,
                        scopes,
                        ..
                    },
            } => {
                assert_eq!(scopes, vec!["mail:send"]);
                assert_eq!(allowed_ips, vec!["203.0.113.0/24", "198.51.100.7"]);
            }
            _ => panic!("expected api-key create command"),
        }
    }

    #[test]
    fn audit_logs_list_rejects_out_of_range_limit() {
        let error = Cli::try_parse_from(["dairo", "audit-logs", "list", "--limit", "101"])
            .expect_err("audit-logs limit above 100 should fail clap validation");
        assert!(error.to_string().contains("101"));
    }

    #[test]
    fn parses_ignore_complaints_flag() {
        let cli = Cli::try_parse_from([
            "dairo",
            "send",
            "--inbox-id",
            "inbox_123",
            "--to",
            "max@example.com",
            "--text",
            "Body",
            "--ignore-complaints",
        ])
        .unwrap();

        match cli.command {
            Command::Send(SendArgs {
                ignore_complaints, ..
            }) => assert!(ignore_complaints),
            _ => panic!("expected send command"),
        }
    }

    #[test]
    fn parses_react_send_arguments() {
        let cli = Cli::try_parse_from([
            "dairo",
            "send",
            "--inbox-id",
            "inbox_123",
            "--to",
            "max@example.com",
            "--subject",
            "Hello",
            "--react-source",
            "export default function Email(props) { return <p>{props.name}</p>; }",
            "--react-props",
            r#"{"name":"Max"}"#,
        ])
        .unwrap();

        match cli.command {
            Command::Send(SendArgs {
                inbox_id,
                to,
                subject,
                text,
                html,
                react_source,
                react_props,
                attachments,
                attachment_delivery,
                attachment_link_expiry_hours,
                ignore_complaints,
                send_at,
                ..
            }) => {
                assert_eq!(inbox_id.as_deref(), Some("inbox_123"));
                assert_eq!(to, vec!["max@example.com"]);
                assert_eq!(subject, "Hello");
                assert_eq!(text, None);
                assert_eq!(html, None);
                assert_eq!(
                    react_source.as_deref(),
                    Some("export default function Email(props) { return <p>{props.name}</p>; }")
                );
                assert_eq!(react_props.as_deref(), Some(r#"{"name":"Max"}"#));
                assert!(attachments.is_empty());
                assert_eq!(attachment_delivery, AttachmentDelivery::Attachment);
                assert_eq!(attachment_link_expiry_hours, None);
                assert!(!ignore_complaints);
                assert_eq!(send_at, None);
            }
            _ => panic!("expected send command"),
        }
    }

    #[test]
    fn send_requires_at_least_one_recipient() {
        let error =
            Cli::try_parse_from(["dairo", "send", "--inbox-id", "inbox_123", "--text", "Body"])
                .expect_err("send without --to should fail clap validation");

        assert!(error.to_string().contains("--to"));
    }

    #[test]
    fn send_requires_at_least_one_body_option() {
        let error = Cli::try_parse_from([
            "dairo",
            "send",
            "--inbox-id",
            "inbox_123",
            "--to",
            "max@example.com",
        ])
        .expect_err("send without a body should fail clap validation");

        let message = error.to_string();
        assert!(message.contains("--text"));
        assert!(message.contains("--html"));
        assert!(message.contains("--react-source"));
    }

    #[test]
    fn parses_attachment_expiry_hours() {
        let cli = Cli::try_parse_from([
            "dairo",
            "attachments",
            "url",
            "att_123",
            "--expiry-hours",
            "24",
        ])
        .unwrap();

        match cli.command {
            Command::Attachment {
                command:
                    AttachmentCommand::Url {
                        attachment_id,
                        expiry_hours,
                    },
            } => {
                assert_eq!(attachment_id, "att_123");
                assert_eq!(expiry_hours, Some(24));
            }
            _ => panic!("expected attachment url command"),
        }

        let cli = Cli::try_parse_from([
            "dairo",
            "attachments",
            "share",
            "att_123",
            "--expiry-hours",
            "168",
        ])
        .unwrap();

        match cli.command {
            Command::Attachment {
                command:
                    AttachmentCommand::Share {
                        attachment_id,
                        expiry_hours,
                    },
            } => {
                assert_eq!(attachment_id, "att_123");
                assert_eq!(expiry_hours, Some(168));
            }
            _ => panic!("expected attachment share command"),
        }
    }

    #[test]
    fn rejects_out_of_range_expiry_hours() {
        let error = Cli::try_parse_from([
            "dairo",
            "attachments",
            "share",
            "att_123",
            "--expiry-hours",
            "169",
        ])
        .expect_err("attachment share expiry above one week should fail clap validation");

        assert!(error.to_string().contains("169"));

        let error = Cli::try_parse_from([
            "dairo",
            "send",
            "--inbox-id",
            "inbox_123",
            "--to",
            "max@example.com",
            "--text",
            "Body",
            "--attachment-delivery",
            "link",
            "--attachment-link-expiry-hours",
            "0",
        ])
        .expect_err("send link expiry below one hour should fail clap validation");

        assert!(error.to_string().contains("0"));
    }

    #[test]
    fn parses_webhook_and_api_key_arguments() {
        let webhook = Cli::parse_from([
            "dairo",
            "webhook",
            "create",
            "--url",
            "https://example.com/hook",
            "--event",
            "message.received",
            "--event",
            "email.delivered",
        ]);
        match webhook.command {
            Command::Webhook {
                command: WebhookCommand::Create { url, events },
            } => {
                assert_eq!(url, "https://example.com/hook");
                assert_eq!(
                    events,
                    vec![WebhookEvent::MessageReceived, WebhookEvent::EmailDelivered]
                );
            }
            _ => panic!("expected webhook create command"),
        }

        let api_key = Cli::parse_from([
            "dairo",
            "api-key",
            "create",
            "--name",
            "CI",
            "--scope",
            "mail:send",
            "--scope",
            "mail:read",
        ]);
        match api_key.command {
            Command::ApiKey {
                command:
                    ApiKeyCommand::Create {
                        name,
                        scopes,
                        allowed_ips,
                    },
            } => {
                assert_eq!(name, "CI");
                assert_eq!(scopes, vec!["mail:send", "mail:read"]);
                assert!(allowed_ips.is_empty());
            }
            _ => panic!("expected api-key create command"),
        }
    }

    #[test]
    fn parses_plural_message_thread_and_singular_aliases() {
        let messages = Cli::parse_from(["dairo", "messages", "get", "msg_123"]);
        assert!(matches!(
            messages.command,
            Command::Message {
                command: MessageCommand::Get { .. }
            }
        ));

        let message_alias = Cli::parse_from(["dairo", "message", "get", "msg_123"]);
        assert!(matches!(
            message_alias.command,
            Command::Message {
                command: MessageCommand::Get { .. }
            }
        ));

        let threads = Cli::parse_from(["dairo", "threads", "list"]);
        assert!(matches!(
            threads.command,
            Command::Thread {
                command: ThreadCommand::List { .. }
            }
        ));

        let thread_alias = Cli::parse_from(["dairo", "thread", "list"]);
        assert!(matches!(
            thread_alias.command,
            Command::Thread {
                command: ThreadCommand::List { .. }
            }
        ));
    }

    #[test]
    fn parses_email_list_delete_command() {
        let cli = Cli::parse_from(["dairo", "lists", "delete", "list_123"]);
        match cli.command {
            Command::EmailList {
                command: EmailListCommand::Delete { list_id },
            } => assert_eq!(list_id, "list_123"),
            _ => panic!("expected lists delete command"),
        }
    }

    #[test]
    fn parses_webhook_verify_command() {
        let cli = Cli::parse_from([
            "dairo",
            "webhook",
            "verify",
            "--secret",
            "whsec_abc",
            "--signature",
            "v1=deadbeef",
            "--timestamp",
            "1717000000",
            "--tolerance-seconds",
            "120",
        ]);
        match cli.command {
            Command::Webhook {
                command:
                    WebhookCommand::Verify {
                        secret,
                        signature,
                        timestamp,
                        tolerance_seconds,
                    },
            } => {
                assert_eq!(secret, "whsec_abc");
                assert_eq!(signature, "v1=deadbeef");
                assert_eq!(timestamp, "1717000000");
                assert_eq!(tolerance_seconds, 120);
            }
            _ => panic!("expected webhook verify command"),
        }
    }

    #[test]
    fn parses_mcp_install_command() {
        let cli = Cli::parse_from([
            "dairo",
            "mcp",
            "install",
            "--client",
            "hermes",
            "--name",
            "dairo-prod",
        ]);
        match cli.command {
            Command::Mcp {
                command: McpCommand::Install { client, name },
            } => {
                assert_eq!(client, McpClient::Hermes);
                assert_eq!(name, "dairo-prod");
            }
            _ => panic!("expected mcp install command"),
        }
    }

    #[test]
    fn parses_mcp_catalog_defaults() {
        let cli = Cli::parse_from(["dairo", "mcp", "catalog"]);
        match cli.command {
            Command::Mcp {
                command:
                    McpCommand::Catalog {
                        json,
                        for_me,
                        family,
                    },
            } => {
                assert!(!json);
                assert!(!for_me);
                assert_eq!(family, None);
            }
            _ => panic!("expected mcp catalog command"),
        }
    }

    #[test]
    fn parses_mcp_catalog_with_flags() {
        let cli = Cli::parse_from([
            "dairo", "mcp", "catalog", "--json", "--for-me", "--family", "outbound",
        ]);
        match cli.command {
            Command::Mcp {
                command:
                    McpCommand::Catalog {
                        json,
                        for_me,
                        family,
                    },
            } => {
                assert!(json);
                assert!(for_me);
                assert_eq!(family.as_deref(), Some("outbound"));
            }
            _ => panic!("expected mcp catalog command"),
        }
    }

    #[test]
    fn webhook_create_rejects_unknown_events() {
        let error = Cli::try_parse_from([
            "dairo",
            "webhook",
            "create",
            "--url",
            "https://example.com/hook",
            "--event",
            "message.created",
        ])
        .expect_err("unknown webhook events should fail clap validation");

        let message = error.to_string();
        assert!(message.contains("message.created"));
        assert!(message.contains("message.received"));
        assert!(message.contains("email.complained"));
    }

    #[test]
    fn parses_init_command() {
        let cli = Cli::parse_from([
            "dairo",
            "init",
            "next",
            "--dir",
            "/tmp/project",
            "--no-install",
            "--no-verify",
            "--inbox-route",
            "/hooks/dairo",
            "--package-manager",
            "pnpm",
        ]);
        match cli.command {
            Command::Init(InitArgs {
                framework,
                framework_flag,
                dir,
                force,
                no_install,
                package_manager,
                inbox_route,
                no_verify,
            }) => {
                assert_eq!(framework, Some(Framework::Next));
                assert_eq!(framework_flag, None);
                assert_eq!(dir, PathBuf::from("/tmp/project"));
                assert!(!force);
                assert!(no_install);
                assert_eq!(package_manager.as_deref(), Some("pnpm"));
                assert_eq!(inbox_route.as_deref(), Some("/hooks/dairo"));
                assert!(no_verify);
            }
            _ => panic!("expected init command"),
        }
    }

    #[test]
    fn parses_init_with_defaults_and_no_framework() {
        let cli = Cli::parse_from(["dairo", "init"]);
        match cli.command {
            Command::Init(args) => {
                assert_eq!(args.framework, None);
                assert_eq!(args.framework_flag, None);
                assert_eq!(args.dir, PathBuf::from("."));
                assert!(!args.force);
                assert!(!args.no_install);
                assert!(!args.no_verify);
                assert_eq!(args.package_manager, None);
                assert_eq!(args.inbox_route, None);
            }
            _ => panic!("expected init command"),
        }
    }

    #[test]
    fn init_accepts_framework_flag_alias() {
        let cli = Cli::parse_from(["dairo", "init", "--framework", "fastapi"]);
        match cli.command {
            Command::Init(args) => {
                assert_eq!(args.framework, None);
                assert_eq!(args.framework_flag, Some(Framework::Fastapi));
            }
            _ => panic!("expected init command"),
        }
    }

    #[test]
    fn init_rejects_unknown_framework() {
        let error = Cli::try_parse_from(["dairo", "init", "rocket"])
            .expect_err("unknown framework should fail clap validation");
        let message = error.to_string();
        assert!(message.contains("rocket"));
        assert!(message.contains("next"));
        assert!(message.contains("cloudflare-workers"));
        assert!(message.contains("go-http"));
    }

    #[test]
    fn parses_template_create_with_source_and_variables() {
        let cli = Cli::parse_from([
            "dairo",
            "templates",
            "create",
            "--slug",
            "welcome",
            "--name",
            "Welcome",
            "--source",
            "export default () => <p>Hi</p>;",
            "--subject",
            "Hello {{name}}",
            "--variables",
            r#"{"name":"string"}"#,
        ]);
        match cli.command {
            Command::Template {
                command:
                    TemplateCommand::Create {
                        slug,
                        name,
                        description,
                        source,
                        source_file,
                        subject,
                        variables,
                        notes,
                    },
            } => {
                assert_eq!(slug, "welcome");
                assert_eq!(name, "Welcome");
                assert_eq!(description, None);
                assert_eq!(source.as_deref(), Some("export default () => <p>Hi</p>;"));
                assert_eq!(source_file, None);
                assert_eq!(subject.as_deref(), Some("Hello {{name}}"));
                assert_eq!(variables.as_deref(), Some(r#"{"name":"string"}"#));
                assert_eq!(notes, None);
            }
            _ => panic!("expected templates create command"),
        }
    }

    #[test]
    fn template_singular_alias_and_create_source_conflict() {
        // `template` (singular) is an alias of `templates`.
        let cli = Cli::parse_from(["dairo", "template", "list"]);
        assert!(matches!(
            cli.command,
            Command::Template {
                command: TemplateCommand::List
            }
        ));

        // --source and --source-file are mutually exclusive.
        let error = Cli::try_parse_from([
            "dairo",
            "templates",
            "create",
            "--slug",
            "welcome",
            "--name",
            "Welcome",
            "--source",
            "x",
            "--source-file",
            "tpl.tsx",
        ])
        .expect_err("source + source-file should conflict");
        let message = error.to_string();
        assert!(message.contains("--source"));
        assert!(message.contains("--source-file"));
    }

    #[test]
    fn parses_template_get_with_version_and_publish_no_promote() {
        let cli = Cli::parse_from(["dairo", "templates", "get", "welcome", "--version", "3"]);
        match cli.command {
            Command::Template {
                command:
                    TemplateCommand::Get {
                        id_or_slug,
                        version,
                    },
            } => {
                assert_eq!(id_or_slug, "welcome");
                assert_eq!(version, Some(3));
            }
            _ => panic!("expected templates get command"),
        }

        let cli = Cli::parse_from([
            "dairo",
            "templates",
            "publish",
            "welcome",
            "--source",
            "export default () => <p>v2</p>;",
            "--no-promote",
        ]);
        match cli.command {
            Command::Template {
                command:
                    TemplateCommand::Publish {
                        id_or_slug,
                        source,
                        no_promote,
                        ..
                    },
            } => {
                assert_eq!(id_or_slug, "welcome");
                assert_eq!(source.as_deref(), Some("export default () => <p>v2</p>;"));
                assert!(no_promote);
            }
            _ => panic!("expected templates publish command"),
        }
    }

    #[test]
    fn parses_template_version_subcommand() {
        let cli = Cli::parse_from(["dairo", "templates", "version", "welcome", "2"]);
        match cli.command {
            Command::Template {
                command:
                    TemplateCommand::Version {
                        id_or_slug,
                        version,
                    },
            } => {
                assert_eq!(id_or_slug, "welcome");
                assert_eq!(version, 2);
            }
            _ => panic!("expected templates version command"),
        }
    }

    #[test]
    fn parses_events_list_with_all_filters() {
        let cli = Cli::parse_from([
            "dairo",
            "events",
            "list",
            "--limit",
            "50",
            "--cursor",
            "cur_1",
            "--inbox-id",
            "inbox_123",
            "--type",
            "message.received",
            "--wait",
            "10",
            "--tail",
        ]);
        match cli.command {
            Command::Events {
                command:
                    EventsCommand::List {
                        limit,
                        cursor,
                        inbox_id,
                        event_type,
                        wait,
                        tail,
                    },
            } => {
                assert_eq!(limit, Some(50));
                assert_eq!(cursor.as_deref(), Some("cur_1"));
                assert_eq!(inbox_id.as_deref(), Some("inbox_123"));
                assert_eq!(event_type.as_deref(), Some("message.received"));
                assert_eq!(wait, Some(10));
                assert!(tail);
            }
            _ => panic!("expected events list command"),
        }
    }

    #[test]
    fn events_list_rejects_out_of_range_wait() {
        let error = Cli::try_parse_from(["dairo", "events", "list", "--wait", "26"])
            .expect_err("events wait above 25 should fail clap validation");
        assert!(error.to_string().contains("26"));
    }

    #[test]
    fn parses_events_replay_with_bounds_and_types() {
        let cli = Cli::parse_from([
            "dairo",
            "events",
            "replay",
            "--since-seq",
            "42",
            "--inbox-id",
            "inbox_123",
            "--type",
            "message.received",
            "--type",
            "email.delivered",
            "--max-events",
            "500",
        ]);
        match cli.command {
            Command::Events {
                command:
                    EventsCommand::Replay {
                        since,
                        since_seq,
                        inbox_id,
                        types,
                        max_events,
                        ..
                    },
            } => {
                assert_eq!(since, None);
                assert_eq!(since_seq, Some(42));
                assert_eq!(inbox_id.as_deref(), Some("inbox_123"));
                assert_eq!(types, vec!["message.received", "email.delivered"]);
                assert_eq!(max_events, Some(500));
            }
            _ => panic!("expected events replay command"),
        }
    }

    #[test]
    fn parses_agent_list_get_and_singular_alias() {
        let cli = Cli::parse_from(["dairo", "agents", "list"]);
        assert!(matches!(
            cli.command,
            Command::Agent {
                command: AgentCommand::List
            }
        ));

        let cli = Cli::parse_from(["dairo", "agent", "get", "agt_abc"]);
        match cli.command {
            Command::Agent {
                command: AgentCommand::Get { id_or_agent },
            } => assert_eq!(id_or_agent, "agt_abc"),
            _ => panic!("expected agents get command"),
        }
    }

    #[test]
    fn parses_agent_verify_by_id_and_signature_form() {
        let cli = Cli::parse_from(["dairo", "agents", "verify", "--id", "msg_123"]);
        match cli.command {
            Command::Agent {
                command:
                    AgentCommand::Verify {
                        id,
                        agent,
                        kid,
                        sig,
                        ..
                    },
            } => {
                assert_eq!(id.as_deref(), Some("msg_123"));
                assert_eq!(agent, None);
                assert_eq!(kid, None);
                assert_eq!(sig, None);
            }
            _ => panic!("expected agents verify command"),
        }

        let cli = Cli::parse_from([
            "dairo", "agents", "verify", "--agent", "agt_abc", "--kid", "kid_1", "--sig",
            "deadbeef",
        ]);
        match cli.command {
            Command::Agent {
                command:
                    AgentCommand::Verify {
                        agent, kid, sig, ..
                    },
            } => {
                assert_eq!(agent.as_deref(), Some("agt_abc"));
                assert_eq!(kid.as_deref(), Some("kid_1"));
                assert_eq!(sig.as_deref(), Some("deadbeef"));
            }
            _ => panic!("expected agents verify command"),
        }
    }

    #[test]
    fn agent_verify_signature_form_requires_kid_and_sig() {
        // --agent without --kid/--sig must fail clap's `requires`.
        let error = Cli::try_parse_from(["dairo", "agents", "verify", "--agent", "agt_abc"])
            .expect_err("--agent without --kid/--sig should fail clap validation");
        let message = error.to_string();
        assert!(message.contains("--kid") || message.contains("--sig"));

        // --id and --agent are mutually exclusive.
        let error = Cli::try_parse_from([
            "dairo", "agents", "verify", "--id", "msg_1", "--agent", "agt_abc",
        ])
        .expect_err("--id + --agent should conflict");
        assert!(error.to_string().contains("--agent"));
    }

    #[test]
    fn parses_reputation_list() {
        let cli = Cli::parse_from(["dairo", "reputation", "list"]);
        assert!(matches!(
            cli.command,
            Command::Reputation {
                command: ReputationCommand::List
            }
        ));
    }

    #[test]
    fn parses_budget_get_and_set() {
        let cli = Cli::parse_from(["dairo", "budgets", "get", "account"]);
        match cli.command {
            Command::Budget {
                command: BudgetCommand::Get { scope },
            } => assert_eq!(scope, "account"),
            _ => panic!("expected budgets get command"),
        }

        let cli = Cli::parse_from([
            "dairo",
            "budgets",
            "set",
            "--scope",
            "key",
            "--scope-id",
            "key_123",
            "--max-sends-per-day",
            "1000",
            "--hard-stop-on-complaint",
        ]);
        match cli.command {
            Command::Budget {
                command:
                    BudgetCommand::Set {
                        scope,
                        scope_id,
                        disabled,
                        max_sends_per_day,
                        max_new_recipients_per_hour,
                        hard_stop_on_complaint,
                    },
            } => {
                assert_eq!(scope, "key");
                assert_eq!(scope_id.as_deref(), Some("key_123"));
                assert!(!disabled);
                assert_eq!(max_sends_per_day, Some(1000));
                assert_eq!(max_new_recipients_per_hour, None);
                assert!(hard_stop_on_complaint);
            }
            _ => panic!("expected budgets set command"),
        }
    }

    #[test]
    fn parses_compliance_residency_and_erasure_job() {
        let cli = Cli::parse_from(["dairo", "compliance", "residency"]);
        assert!(matches!(
            cli.command,
            Command::Compliance {
                command: ComplianceCommand::Residency
            }
        ));

        let cli = Cli::parse_from(["dairo", "compliance", "erasure-job", "job_123"]);
        match cli.command {
            Command::Compliance {
                command: ComplianceCommand::ErasureJob { job_id },
            } => assert_eq!(job_id, "job_123"),
            _ => panic!("expected compliance erasure-job command"),
        }
    }

    #[test]
    fn parses_a2a_list_and_get() {
        let cli = Cli::parse_from([
            "dairo",
            "a2a",
            "list",
            "--limit",
            "25",
            "--cursor",
            "cur_1",
            "--inbox-id",
            "inbox_123",
        ]);
        match cli.command {
            Command::A2a {
                command:
                    A2aCommand::List {
                        limit,
                        cursor,
                        inbox_id,
                    },
            } => {
                assert_eq!(limit, Some(25));
                assert_eq!(cursor.as_deref(), Some("cur_1"));
                assert_eq!(inbox_id.as_deref(), Some("inbox_123"));
            }
            _ => panic!("expected a2a list command"),
        }

        let cli = Cli::parse_from(["dairo", "a2a", "get", "a2a_123"]);
        match cli.command {
            Command::A2a {
                command: A2aCommand::Get { id },
            } => assert_eq!(id, "a2a_123"),
            _ => panic!("expected a2a get command"),
        }
    }

    #[test]
    fn init_accepts_every_tier1_framework() {
        for value in [
            "next",
            "express",
            "hono",
            "cloudflare-workers",
            "fastapi",
            "flask",
            "go-http",
        ] {
            let cli = Cli::try_parse_from(["dairo", "init", value])
                .unwrap_or_else(|_| panic!("framework {value} should parse"));
            assert!(matches!(cli.command, Command::Init(_)));
        }
    }

    #[test]
    fn parses_budget_list_and_delete_commands() {
        let list = Cli::parse_from(["dairo", "budgets", "list"]);
        assert!(matches!(
            list.command,
            Command::Budget {
                command: BudgetCommand::List
            }
        ));

        let delete = Cli::parse_from(["dairo", "budgets", "delete", "account"]);
        match delete.command {
            Command::Budget {
                command: BudgetCommand::Delete { scope },
            } => assert_eq!(scope, "account"),
            _ => panic!("expected budgets delete command"),
        }
    }

    #[test]
    fn parses_erasure_jobs_commands() {
        let list = Cli::parse_from(["dairo", "erasure-jobs", "list"]);
        assert!(matches!(
            list.command,
            Command::ErasureJobs {
                command: ErasureJobCommand::List
            }
        ));

        let create = Cli::parse_from([
            "dairo",
            "erasure-jobs",
            "create",
            "--subject-email",
            "max@example.com",
        ]);
        match create.command {
            Command::ErasureJobs {
                command:
                    ErasureJobCommand::Create {
                        subject_email,
                        inbox_id,
                    },
            } => {
                assert_eq!(subject_email.as_deref(), Some("max@example.com"));
                assert_eq!(inbox_id, None);
            }
            _ => panic!("expected erasure-jobs create command"),
        }

        let get = Cli::parse_from(["dairo", "erasure-jobs", "get", "job_123"]);
        match get.command {
            Command::ErasureJobs {
                command: ErasureJobCommand::Get { job_id },
            } => assert_eq!(job_id, "job_123"),
            _ => panic!("expected erasure-jobs get command"),
        }
    }

    #[test]
    fn erasure_jobs_create_rejects_both_targets() {
        // --subject-email and --inbox-id are mutually exclusive at the clap layer.
        let error = Cli::try_parse_from([
            "dairo",
            "erasure-jobs",
            "create",
            "--subject-email",
            "max@example.com",
            "--inbox-id",
            "inbox_123",
        ])
        .expect_err("both erasure targets at once should fail clap validation");
        assert!(error.to_string().contains("--inbox-id"));
    }

    #[test]
    fn parses_inbox_schema_commands() {
        let set = Cli::parse_from([
            "dairo",
            "inbox",
            "schema",
            "set",
            "agent@example.com",
            "--schema",
            r#"{"code":{"type":"string","required":true}}"#,
            "--on-validation-error",
            "passthrough",
            "--extraction-hint",
            "Find the one-time code.",
        ]);
        match set.command {
            Command::Inbox {
                command:
                    InboxCommand::Schema {
                        command:
                            InboxSchemaCommand::Set {
                                inbox,
                                schema,
                                schema_file,
                                on_validation_error,
                                extraction_hint,
                            },
                    },
            } => {
                assert_eq!(inbox, "agent@example.com");
                assert_eq!(
                    schema.as_deref(),
                    Some(r#"{"code":{"type":"string","required":true}}"#)
                );
                assert_eq!(schema_file, None);
                assert!(matches!(
                    on_validation_error,
                    Some(InboxSchemaValidationMode::Passthrough)
                ));
                assert_eq!(extraction_hint.as_deref(), Some("Find the one-time code."));
            }
            _ => panic!("expected inbox schema set command"),
        }

        let get = Cli::parse_from(["dairo", "inbox", "schema", "get", "inbox_123"]);
        assert!(matches!(
            get.command,
            Command::Inbox {
                command: InboxCommand::Schema {
                    command: InboxSchemaCommand::Get { .. }
                }
            }
        ));
    }

    #[test]
    fn parses_verification_wait_commands() {
        let register = Cli::parse_from([
            "dairo",
            "inbox",
            "verification-waits",
            "register",
            "inbox_123",
            "--timeout-sec",
            "120",
            "--from-hint",
            "github.com",
            "--pattern",
            r#"code: ([0-9]{6})"#,
            "--idempotency-key",
            "wait-1",
        ]);
        match register.command {
            Command::Inbox {
                command:
                    InboxCommand::VerificationWaits {
                        command:
                            VerificationWaitCommand::Register {
                                inbox,
                                timeout_sec,
                                from_hint,
                                pattern,
                                idempotency_key,
                            },
                    },
            } => {
                assert_eq!(inbox, "inbox_123");
                assert_eq!(timeout_sec, 120);
                assert_eq!(from_hint.as_deref(), Some("github.com"));
                assert_eq!(pattern.as_deref(), Some(r#"code: ([0-9]{6})"#));
                assert_eq!(idempotency_key.as_deref(), Some("wait-1"));
            }
            _ => panic!("expected verification-waits register command"),
        }

        let error = Cli::try_parse_from([
            "dairo",
            "inbox",
            "verification-waits",
            "register",
            "inbox_123",
            "--timeout-sec",
            "10",
        ])
        .expect_err("timeout below backend minimum should fail clap validation");
        assert!(error.to_string().contains("timeout-sec"));
    }

    #[test]
    fn outbound_events_requires_email_id() {
        // Events are now a per-email sub-resource, so --email-id is required.
        let error = Cli::try_parse_from(["dairo", "outbound", "events"])
            .expect_err("outbound events without --email-id should fail clap validation");
        assert!(error.to_string().contains("--email-id"));

        let cli = Cli::parse_from(["dairo", "outbound", "events", "--email-id", "email_123"]);
        match cli.command {
            Command::Outbound {
                command: OutboundCommand::Events { email_id, .. },
            } => assert_eq!(email_id, "email_123"),
            _ => panic!("expected outbound events command"),
        }
    }
}
