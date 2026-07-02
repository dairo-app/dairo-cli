//! `dairo listen` — a local inbound-email sandbox/tunnel.
//!
//! This is the Dairo equivalent of `stripe listen`. It pulls from the durable
//! event ledger (`GET /v1/events`) using the server-side `wait` long-poll, prints
//! each event to the terminal, and — when `--forward-to` is set — re-POSTs each
//! one to a local endpoint with production-compatible `X-Dairo-*` webhook headers
//! and an ephemeral HMAC signature so a handler written against real Dairo
//! webhooks works unchanged locally.
//!
//! ## Why a poll loop and not a websocket
//! The ledger's `(created_at, id)` keyset is the streaming substrate: the cursor
//! lives on the client, every poll is a stateless user-scoped index read, and a
//! reconnect resumes from the persisted cursor with no lost events. This is the
//! scalable shape (see the design doc): 10 listeners or 10M listeners is the same
//! stateless code path.
//!
//! ## Correctness
//! - The cursor advances strictly monotonically (it *is* the ledger keyset), so
//!   paging is forward-only, no-skip, no-overlap.
//! - The cursor is persisted (`0600`, atomic) after each successfully handled
//!   batch, so a crash mid-batch re-reads that batch (at-least-once into the local
//!   handler; the `X-Dairo-Event-Id` header lets the handler dedupe).
//! - `gaps[]` from the server is surfaced as a visible warning rather than
//!   silently swallowed.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use url::Url;

use crate::api::{backoff, ApiClient, EventsQuery, Inbox, LedgerEvent};
use crate::cli::{ListenArgs, PrintMode};
use crate::fsutil::{ensure_parent_private, write_atomic_0600};
use crate::output;
use crate::webhook::sign_body;

mod time;
use time::{format_unix_rfc3339, parse_duration, unix_now};

/// Default event-type set when `--events` is not given: the inbound-sandbox
/// intent (`stripe listen` for received mail). `*` widens to everything,
/// including outbound delivery events.
const DEFAULT_EVENT_GLOBS: &[&str] = &["message.received", "message.quarantined"];

/// Per-page ledger read size. The server caps `limit` at 100; 50 mirrors the
/// `GET /v1/events` default and drains bursts in a few pages.
const PAGE_LIMIT: u32 = 50;

/// Backoff base for failed forwards and failed polls. Doubled per attempt and
/// capped, mirroring `api.rs`'s retry policy.
const FORWARD_BACKOFF_BASE: Duration = Duration::from_millis(250);
const FORWARD_BACKOFF_MAX: Duration = Duration::from_secs(5);

/// Marks a forwarded delivery as coming from the local CLI tunnel rather than the
/// production fan-out, so a handler can tell them apart.
const DELIVERY_HEADER_VALUE: &str = "cli-listen";

/// Resolved, validated configuration for a `dairo listen` run.
struct ListenConfig {
    /// Validated loopback/https forward target, or `None` for print-only mode.
    forward_to: Option<Url>,
    /// Inbox ids to keep (resolved from `--inbox` addresses/ids), empty = all.
    inbox_filter: Vec<String>,
    /// When exactly one inbox is requested, push it to the server `inboxId`
    /// filter; multiple inboxes stream unfiltered and filter client-side.
    server_inbox_id: Option<String>,
    /// Event-type globs to match (client-side); the single exact type, if any, is
    /// also pushed to the server `type` filter.
    event_globs: Vec<String>,
    server_event_type: Option<String>,
    print: PrintMode,
    wait: u8,
    max_forward_retries: u8,
    /// Ephemeral webhook signing secret for this run (`whsec_...`), or `None` when
    /// `--no-sign` / print-only.
    signing_secret: Option<String>,
    state_file: PathBuf,
    no_resume: bool,
    replay: Option<ReplayMode>,
}

/// How to position the start cursor when not resuming a persisted one.
#[derive(Debug, Clone, PartialEq, Eq)]
enum ReplayMode {
    /// Replay from the very beginning of the ledger.
    All,
    /// Replay approximately the last `n` events.
    Count(u32),
    /// Replay events newer than `now - duration`.
    Since(Duration),
}

/// Mutable counters surfaced in the shutdown summary.
#[derive(Default)]
struct Stats {
    received: AtomicU64,
    forwarded: AtomicU64,
    forward_failed: AtomicU64,
    gaps_seen: AtomicU64,
}

/// Entry point invoked from `main`. Runs until SIGINT (Ctrl-C), then flushes the
/// cursor and prints a summary.
pub async fn run_listen(
    client: &ApiClient,
    args: ListenArgs,
    api_key: &str,
    json_banner: bool,
) -> Result<()> {
    let config = resolve_config(client, args, api_key).await?;
    let stats = Stats::default();

    let http = reqwest::Client::builder()
        .user_agent(concat!("dairo-cli/", env!("CARGO_PKG_VERSION"), " listen"))
        .build()
        .context("failed to build local forward HTTP client")?;

    output::print_listen_banner(&config_banner(&config), json_banner);

    // The whole streaming loop races against Ctrl-C; whichever finishes first
    // wins and we then flush + summarize. Using `select!` keeps the SIGINT path
    // from leaving an in-flight long-poll hanging the process.
    let mut cursor = initial_cursor(client, &config).await?;
    tokio::select! {
        result = stream_loop(client, &http, &config, &stats, &mut cursor) => {
            // The loop only returns on an unrecoverable error; a clean stream
            // runs forever until interrupted.
            flush_cursor(&config, cursor.as_deref());
            result?;
        }
        _ = wait_for_interrupt() => {
            flush_cursor(&config, cursor.as_deref());
        }
    }

    output::print_listen_summary(
        stats.received.load(Ordering::Relaxed),
        stats.forwarded.load(Ordering::Relaxed),
        stats.forward_failed.load(Ordering::Relaxed),
        stats.gaps_seen.load(Ordering::Relaxed),
        config.forward_to.is_some(),
        json_banner,
    );
    Ok(())
}

/// Resolves and validates everything the loop needs up front, so the streaming
/// path is failure-free except for transient network errors.
async fn resolve_config(
    client: &ApiClient,
    args: ListenArgs,
    api_key: &str,
) -> Result<ListenConfig> {
    let forward_to = match args.forward_to.as_deref() {
        Some(raw) => Some(validate_forward_target(raw)?),
        None => None,
    };

    // Resolve `--inbox` values (addresses or ids) to inbox ids. We only call the
    // inboxes endpoint if at least one address-looking value is present.
    let inbox_filter = resolve_inbox_filter(client, &args.inbox).await?;
    let server_inbox_id = if inbox_filter.len() == 1 {
        inbox_filter.first().cloned()
    } else {
        None
    };

    let event_globs = if args.events.is_empty() {
        DEFAULT_EVENT_GLOBS.iter().map(|s| s.to_string()).collect()
    } else {
        args.events.clone()
    };
    // Only push the server `type` filter when there is exactly one exact
    // (non-glob) type; otherwise stream broadly and match client-side.
    let server_event_type = match event_globs.as_slice() {
        [single] if !is_glob(single) => Some(single.clone()),
        _ => None,
    };

    let replay = match args.replay.as_deref() {
        Some(raw) => Some(parse_replay(raw)?),
        None => None,
    };

    // Signing: mint a fresh ephemeral secret per run (unless --no-sign or
    // print-only). Printed once so a handler can verify with it.
    let signing_secret = if forward_to.is_some() && !args.no_sign {
        Some(generate_signing_secret())
    } else {
        None
    };

    let state_file = match args.state_file {
        Some(path) => path,
        None => default_state_file(api_key, &inbox_filter, &event_globs)?,
    };

    Ok(ListenConfig {
        forward_to,
        inbox_filter,
        server_inbox_id,
        event_globs,
        server_event_type,
        print: args.print,
        wait: args.wait,
        max_forward_retries: args.max_forward_retries,
        signing_secret,
        state_file,
        no_resume: args.no_resume,
        replay,
    })
}

/// Resolves the start cursor before streaming: a persisted cursor (unless
/// `--no-resume`), else `--replay`'s position, else the tail head cursor.
async fn initial_cursor(client: &ApiClient, config: &ListenConfig) -> Result<Option<String>> {
    if !config.no_resume {
        if let Some(saved) = load_cursor(&config.state_file) {
            return Ok(Some(saved));
        }
    }
    match &config.replay {
        Some(ReplayMode::All) => Ok(None), // start from the beginning
        Some(ReplayMode::Count(n)) => count_replay_cursor(client, config, *n).await,
        Some(ReplayMode::Since(duration)) => {
            duration_replay_cursor(client, config, *duration).await
        }
        None => tail_cursor(client, config).await,
    }
}

/// Fetches the head cursor "as of now" so the loop streams only *new* events.
async fn tail_cursor(client: &ApiClient, config: &ListenConfig) -> Result<Option<String>> {
    let response = client
        .list_events(&EventsQuery {
            tail: true,
            limit: Some(1),
            inbox_id: config.server_inbox_id.clone(),
            event_type: config.server_event_type.clone(),
            ..Default::default()
        })
        .await
        .context("failed to read the head cursor (GET /v1/events?tail=true)")?;
    Ok(response.pagination.next_cursor)
}

/// Finds the resume cursor that replays approximately the last `n` events. The
/// public read exposes a *page-granular* cursor (not per-event), so we page
/// forward keeping a sliding window of `(cursor_before_page, page_event_count)`
/// and return the cursor of the earliest page boundary that still leaves at least
/// `n` events ahead. The replay is therefore page-aligned (it may replay up to
/// `PAGE_LIMIT - 1` extra older events), which is the precise behavior achievable
/// without per-event cursors. Returns `None` (= from the beginning) when the
/// ledger holds fewer than `n` events.
async fn count_replay_cursor(
    client: &ApiClient,
    config: &ListenConfig,
    n: u32,
) -> Result<Option<String>> {
    // Each entry: the cursor *before* a page, and how many events that page held.
    // `None` cursor == ledger start.
    let mut pages: Vec<(Option<String>, usize)> = Vec::new();
    let mut cursor: Option<String> = None;
    loop {
        let before_page = cursor.clone();
        let response = client
            .list_events(&EventsQuery {
                since: cursor.clone(),
                limit: Some(PAGE_LIMIT),
                inbox_id: config.server_inbox_id.clone(),
                event_type: config.server_event_type.clone(),
                ..Default::default()
            })
            .await
            .context("failed to scan ledger for --replay <N>")?;
        if !response.events.is_empty() {
            pages.push((before_page, response.events.len()));
        }
        if !response.pagination.has_more {
            break;
        }
        cursor = response.pagination.next_cursor;
        if cursor.is_none() {
            break;
        }
    }
    // Walk pages newest→oldest, accumulating events until we have at least `n`.
    // The chosen page's `before_page` cursor is the resume point.
    let mut accumulated = 0usize;
    let mut resume: Option<String> = None;
    for (before_page, count) in pages.iter().rev() {
        resume = before_page.clone();
        accumulated += count;
        if accumulated >= n as usize {
            break;
        }
    }
    Ok(resume)
}

/// Finds the resume cursor for a duration lower bound (`--replay 1h`). The public
/// `GET /v1/events` `since` param only accepts a real opaque keyset cursor — it
/// cannot be synthesized from a timestamp — so we page forward from the start and
/// return the cursor *just before* the first event whose `createdAt` is within
/// the window. Streaming then resumes from there, replaying everything in the
/// window. Returns `None` (= from the beginning) if every event is in-window.
async fn duration_replay_cursor(
    client: &ApiClient,
    config: &ListenConfig,
    duration: Duration,
) -> Result<Option<String>> {
    let lower_bound = format_unix_rfc3339(unix_now().saturating_sub(duration.as_secs() as i64));
    // Cursor strictly before the current page; advanced page by page. The first
    // page whose events cross the lower bound gives us the resume point.
    let mut before_page: Option<String> = None;
    let mut cursor: Option<String> = None;
    loop {
        let response = client
            .list_events(&EventsQuery {
                since: cursor.clone(),
                limit: Some(PAGE_LIMIT),
                inbox_id: config.server_inbox_id.clone(),
                event_type: config.server_event_type.clone(),
                ..Default::default()
            })
            .await
            .context("failed to scan ledger for --replay <duration>")?;
        // If any event on this page is at/after the lower bound, the resume
        // cursor is the one before this page (so the whole window is replayed).
        let crosses = response
            .events
            .iter()
            .any(|event| created_at_at_or_after(event, &lower_bound));
        if crosses {
            return Ok(before_page);
        }
        if !response.pagination.has_more {
            // No event is within the window; resume after the last page so the
            // stream waits for new events rather than replaying old ones.
            return Ok(response.pagination.next_cursor.or(before_page));
        }
        before_page = response.pagination.next_cursor.clone();
        cursor = response.pagination.next_cursor;
        if cursor.is_none() {
            return Ok(before_page);
        }
    }
}

/// Whether an event's `createdAt` (or `occurredAt` fallback) is lexicographically
/// at or after `lower_bound`. RFC3339 UTC strings sort chronologically, so a
/// string comparison is correct for the `...Z` timestamps the ledger emits.
fn created_at_at_or_after(event: &LedgerEvent, lower_bound: &str) -> bool {
    let ts = event
        .created_at
        .as_deref()
        .or(event.occurred_at.as_deref())
        .unwrap_or("");
    ts >= lower_bound
}

/// The core poll loop. Returns only on an unrecoverable error; transient poll
/// failures are retried with bounded backoff.
async fn stream_loop(
    client: &ApiClient,
    http: &reqwest::Client,
    config: &ListenConfig,
    stats: &Stats,
    cursor: &mut Option<String>,
) -> Result<()> {
    let mut poll_failures: u32 = 0;
    loop {
        let response = match client
            .list_events(&EventsQuery {
                since: cursor.clone(),
                limit: Some(PAGE_LIMIT),
                inbox_id: config.server_inbox_id.clone(),
                event_type: config.server_event_type.clone(),
                wait: Some(config.wait),
                tail: false,
            })
            .await
        {
            Ok(response) => {
                poll_failures = 0;
                response
            }
            Err(error) => {
                // A transient blip (laptop sleep, network drop) should not kill
                // the stream; back off and resume from the same cursor. The
                // ledger is durable, so nothing between now and the retry is lost.
                poll_failures += 1;
                output::print_listen_poll_error(&error.to_string());
                backoff(poll_failures, FORWARD_BACKOFF_BASE, FORWARD_BACKOFF_MAX).await;
                continue;
            }
        };

        surface_gaps(&response.gaps, stats);

        for event in &response.events {
            stats.received.fetch_add(1, Ordering::Relaxed);
            // Client-side filtering: the inbox set and glob event types not
            // narrowed by the single-valued server filters.
            if !event_matches(event, config) {
                continue;
            }
            output::print_listen_event(event, config.print);
            if config.forward_to.is_some() {
                forward_event(http, config, event, stats).await;
            }
        }

        // Persist the page cursor after each fully handled batch so a restart
        // resumes exactly here. The public read exposes a page-granular cursor
        // (not per-event), so a crash mid-batch re-reads the whole page — at
        // least once into the handler, which dedupes on X-Dairo-Event-Id.
        if !response.events.is_empty() {
            if let Some(next) = &response.pagination.next_cursor {
                *cursor = Some(next.clone());
            }
            flush_cursor(config, cursor.as_deref());
        }

        // Drain a burst promptly: when more is buffered the next iteration's
        // `since=<advanced cursor>` re-polls immediately because `has_more` means
        // the long-poll path returns the next page without hanging. When the
        // stream is caught up, the `wait` long-poll parks until new events arrive.
        let _ = response.pagination.has_more;
    }
}

/// POSTs one event to the local endpoint with production-compatible headers and
/// an ephemeral signature. Retries with bounded backoff; on exhaustion it
/// logs-and-continues so a bad handler cannot wedge the stream forever.
async fn forward_event(
    http: &reqwest::Client,
    config: &ListenConfig,
    event: &LedgerEvent,
    stats: &Stats,
) {
    let Some(target) = &config.forward_to else {
        return;
    };
    let body = webhook_body(event);
    let body_bytes = match serde_json::to_vec(&body) {
        Ok(bytes) => bytes,
        Err(error) => {
            output::print_listen_forward_result(&event.event_id, Err(&error.to_string()));
            stats.forward_failed.fetch_add(1, Ordering::Relaxed);
            return;
        }
    };
    let timestamp = unix_now().to_string();
    let signature = config
        .signing_secret
        .as_deref()
        .map(|secret| sign_body(secret, &body_bytes));

    let mut attempt: u32 = 0;
    loop {
        let mut request = http
            .post(target.clone())
            .header("content-type", "application/json")
            .header("X-Dairo-Event", &event.event_type)
            .header("X-Dairo-Event-Id", &event.event_id)
            .header("X-Dairo-Timestamp", &timestamp)
            .header("X-Dairo-Delivery", DELIVERY_HEADER_VALUE)
            .body(body_bytes.clone());
        if let Some(signature) = &signature {
            request = request.header("X-Dairo-Signature", signature);
        }

        match request.send().await {
            Ok(response) if response.status().is_success() => {
                output::print_listen_forward_result(
                    &event.event_id,
                    Ok(response.status().as_u16()),
                );
                stats.forwarded.fetch_add(1, Ordering::Relaxed);
                return;
            }
            Ok(response) => {
                let status = response.status().as_u16();
                if attempt >= u32::from(config.max_forward_retries) {
                    output::print_listen_forward_result(
                        &event.event_id,
                        Err(&format!("HTTP {status} after {} attempts", attempt + 1)),
                    );
                    stats.forward_failed.fetch_add(1, Ordering::Relaxed);
                    return;
                }
            }
            Err(error) => {
                if attempt >= u32::from(config.max_forward_retries) {
                    output::print_listen_forward_result(
                        &event.event_id,
                        Err(&format!("{error} after {} attempts", attempt + 1)),
                    );
                    stats.forward_failed.fetch_add(1, Ordering::Relaxed);
                    return;
                }
            }
        }
        attempt += 1;
        backoff(attempt, FORWARD_BACKOFF_BASE, FORWARD_BACKOFF_MAX).await;
    }
}

/// Reconstructs the production `WebhookEvent` JSON (camelCase, byte-compatible
/// with `webhooks::WebhookEvent`) from a ledger row, exactly as `events/replay`
/// does: same event id, type, occurred-at, user (when present), and payload.
fn webhook_body(event: &LedgerEvent) -> serde_json::Value {
    // `occurredAt` is the event's logical time; the production replay path uses it
    // for `createdAt`, falling back to the ledger row's `createdAt`.
    let created_at = event
        .occurred_at
        .clone()
        .or_else(|| event.created_at.clone())
        .unwrap_or_default();
    // `userId` is part of the production `WebhookEvent` shape but is not exposed
    // on the public ledger read, so it is omitted here. A handler keys on
    // id/type/createdAt/data — all byte-compatible with the live fan-out.
    serde_json::json!({
        "id": event.event_id,
        "type": event.event_type,
        "createdAt": created_at,
        "data": event.data,
    })
}

// ---------------------------------------------------------------------------
// Filtering
// ---------------------------------------------------------------------------

fn event_matches(event: &LedgerEvent, config: &ListenConfig) -> bool {
    matches_inbox(event, &config.inbox_filter) && matches_event_type(event, &config.event_globs)
}

fn matches_inbox(event: &LedgerEvent, inbox_filter: &[String]) -> bool {
    if inbox_filter.is_empty() {
        return true;
    }
    match &event.inbox_id {
        Some(inbox_id) => inbox_filter.iter().any(|id| id == inbox_id),
        // Events without an inbox (e.g. some outbound/account events) are only
        // shown in the unfiltered ("all inboxes") mode.
        None => false,
    }
}

fn matches_event_type(event: &LedgerEvent, globs: &[String]) -> bool {
    globs
        .iter()
        .any(|glob| glob_matches(glob, &event.event_type))
}

/// Returns whether `value` contains a `*` wildcard.
fn is_glob(value: &str) -> bool {
    value.contains('*')
}

/// Minimal glob: `*` matches any run of characters. Supports the documented
/// forms `*`, `message.*`, `*.received`, exact types. No `?` or character
/// classes (the design only specifies `*`).
fn glob_matches(glob: &str, value: &str) -> bool {
    if glob == "*" || glob == "all" {
        return true;
    }
    if !glob.contains('*') {
        return glob == value;
    }
    // Split on `*` and ensure each literal segment appears in order, anchored at
    // the ends when the glob does not start/end with `*`.
    let parts: Vec<&str> = glob.split('*').collect();
    let mut pos = 0usize;
    for (index, part) in parts.iter().enumerate() {
        if part.is_empty() {
            continue;
        }
        if index == 0 {
            if !value[pos..].starts_with(part) {
                return false;
            }
            pos += part.len();
        } else if index == parts.len() - 1 {
            // Last anchored segment must match the suffix.
            return value[pos..].ends_with(part);
        } else {
            match value[pos..].find(part) {
                Some(found) => pos += found + part.len(),
                None => return false,
            }
        }
    }
    // Trailing `*` (last part empty) matches the rest.
    true
}

fn surface_gaps(gaps: &[serde_json::Value], stats: &Stats) {
    if gaps.is_empty() {
        return;
    }
    stats
        .gaps_seen
        .fetch_add(gaps.len() as u64, Ordering::Relaxed);
    output::print_listen_gaps(gaps);
}

// ---------------------------------------------------------------------------
// Forward-target validation
// ---------------------------------------------------------------------------

/// Validates `--forward-to`: loopback (`http://localhost`, `127.0.0.1`, `[::1]`)
/// is allowed because it is the developer's own machine; non-loopback `http://`
/// is rejected to avoid accidental cleartext to a remote host; `https://`
/// non-loopback is allowed (forward to a staging server).
fn validate_forward_target(raw: &str) -> Result<Url> {
    let url = Url::parse(raw).with_context(|| format!("invalid --forward-to URL: {raw}"))?;
    match url.scheme() {
        "https" => Ok(url),
        "http" if is_loopback_url(&url) => Ok(url),
        "http" => anyhow::bail!(
            "refusing to forward over plain http to a non-loopback host: {raw}. \
             Use a loopback URL (http://localhost, http://127.0.0.1) or https:// for a remote target."
        ),
        scheme => anyhow::bail!("--forward-to must be an http(s) URL, got scheme '{scheme}'"),
    }
}

fn is_loopback_url(url: &Url) -> bool {
    match url.host() {
        Some(url::Host::Domain(host)) => {
            let host = host.to_ascii_lowercase();
            host == "localhost" || host == "localhost." || host.ends_with(".localhost")
        }
        Some(url::Host::Ipv4(ip)) => ip.is_loopback(),
        Some(url::Host::Ipv6(ip)) => ip.is_loopback(),
        None => false,
    }
}

// ---------------------------------------------------------------------------
// Inbox resolution
// ---------------------------------------------------------------------------

/// Maps `--inbox` values (which may be inbox ids or email addresses) to inbox
/// ids. Only fetches the inbox list when at least one value looks like an
/// address (contains `@`).
async fn resolve_inbox_filter(client: &ApiClient, requested: &[String]) -> Result<Vec<String>> {
    if requested.is_empty() {
        return Ok(Vec::new());
    }
    let needs_lookup = requested.iter().any(|value| value.contains('@'));
    let inboxes: Vec<Inbox> = if needs_lookup {
        client
            .list_inboxes()
            .await
            .context("failed to resolve --inbox address to an inbox id")?
            .data
    } else {
        Vec::new()
    };
    let mut resolved = Vec::with_capacity(requested.len());
    for value in requested {
        if value.contains('@') {
            let target = value.to_ascii_lowercase();
            let inbox = inboxes
                .iter()
                .find(|inbox| inbox.address.eq_ignore_ascii_case(&target))
                .with_context(|| format!("no inbox found for address {value}"))?;
            resolved.push(inbox.id.clone());
        } else {
            resolved.push(value.clone());
        }
    }
    resolved.sort();
    resolved.dedup();
    Ok(resolved)
}

// ---------------------------------------------------------------------------
// Cursor-file persistence
// ---------------------------------------------------------------------------

/// On-disk resume state. The fingerprint + filters tie a cursor file to the
/// `(API key, filter set)` it was written for, so one listen session cannot
/// resume another's cursor.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct CursorFile {
    cursor: Option<String>,
    #[serde(rename = "apiKeyFingerprint")]
    api_key_fingerprint: String,
    filters: CursorFilters,
    #[serde(rename = "updatedAt")]
    updated_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct CursorFilters {
    inbox: Vec<String>,
    events: Vec<String>,
}

/// Default state-file path: `~/.config/dairo/listen-<keyhash>.cursor`, keyed by
/// the API key fingerprint + filter set so concurrent listens don't collide.
fn default_state_file(api_key: &str, inbox: &[String], events: &[String]) -> Result<PathBuf> {
    let base = dirs::config_dir().context("could not determine user config directory")?;
    let fingerprint = filter_fingerprint(api_key, inbox, events);
    Ok(base
        .join("dairo")
        .join(format!("listen-{fingerprint}.cursor")))
}

/// A short, stable hash over the API key and the filter set. Used both in the
/// default filename and the file body to detect a mismatched resume.
fn filter_fingerprint(api_key: &str, inbox: &[String], events: &[String]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(api_key.as_bytes());
    hasher.update([0u8]);
    let mut inbox_sorted: Vec<&String> = inbox.iter().collect();
    inbox_sorted.sort();
    for value in inbox_sorted {
        hasher.update(value.as_bytes());
        hasher.update([0u8]);
    }
    hasher.update([1u8]);
    let mut events_sorted: Vec<&String> = events.iter().collect();
    events_sorted.sort();
    for value in events_sorted {
        hasher.update(value.as_bytes());
        hasher.update([0u8]);
    }
    let digest = hasher.finalize();
    // 16 hex chars (8 bytes) is collision-safe for per-machine cursor files.
    digest[..8].iter().map(|b| format!("{b:02x}")).collect()
}

/// Loads a persisted cursor, ignoring an unreadable/corrupt file (we simply fall
/// back to tail rather than fail the whole run).
fn load_cursor(path: &Path) -> Option<String> {
    let contents = std::fs::read_to_string(path).ok()?;
    let parsed: CursorFile = serde_json::from_str(&contents).ok()?;
    parsed.cursor
}

/// Writes the cursor file atomically with `0600` permissions. Best-effort: a
/// write failure is logged but does not crash the stream (the next batch will
/// retry the write). The cursor is not secret, but it lives next to the config
/// file so we keep the same private-file policy.
fn flush_cursor(config: &ListenConfig, cursor: Option<&str>) {
    let fingerprint = filter_fingerprint_from_config(config);
    let file = CursorFile {
        cursor: cursor.map(|c| c.to_string()),
        api_key_fingerprint: fingerprint,
        filters: CursorFilters {
            inbox: config.inbox_filter.clone(),
            events: config.event_globs.clone(),
        },
        updated_at: format_unix_rfc3339(unix_now()),
    };
    let serialized = match serde_json::to_vec_pretty(&file) {
        Ok(bytes) => bytes,
        Err(_) => return,
    };
    if ensure_parent_private(&config.state_file).is_err() {
        return;
    }
    if let Err(error) = write_atomic_0600(&config.state_file, &serialized) {
        output::print_listen_poll_error(&format!(
            "could not persist cursor to {}: {error}",
            config.state_file.display()
        ));
    }
}

/// Recomputes the fingerprint for the cursor-file body. We don't keep the API key
/// on the config, so we re-derive from the filters only here; the filename hash
/// already binds the key. The body field is informational.
fn filter_fingerprint_from_config(config: &ListenConfig) -> String {
    // Fingerprint the filters (without the key) for the body; the key is already
    // bound into the default filename.
    filter_fingerprint("", &config.inbox_filter, &config.event_globs)
}

// ---------------------------------------------------------------------------
// Replay parsing
// ---------------------------------------------------------------------------

/// Parses `--replay <N|all|duration>` into a [`ReplayMode`]. Durations accept a
/// single `s`/`m`/`h`/`d` suffix (e.g. `30m`, `2h`, `1d`).
fn parse_replay(raw: &str) -> Result<ReplayMode> {
    let trimmed = raw.trim();
    if trimmed.eq_ignore_ascii_case("all") {
        return Ok(ReplayMode::All);
    }
    if let Ok(count) = trimmed.parse::<u32>() {
        anyhow::ensure!(count > 0, "--replay <N> must be a positive count");
        return Ok(ReplayMode::Count(count));
    }
    if let Some(duration) = parse_duration(trimmed) {
        return Ok(ReplayMode::Since(duration));
    }
    anyhow::bail!(
        "invalid --replay value '{raw}'. Use a count (e.g. 50), 'all', or a duration (e.g. 1h, 30m, 2d)."
    )
}

// ---------------------------------------------------------------------------
// Small utilities
// ---------------------------------------------------------------------------

/// Generates a fresh ephemeral webhook signing secret (`whsec_<hex>`) for this
/// run. Derived from process entropy via a UUID v4 (already a CLI dependency);
/// it never touches disk and is printed to the operator exactly once.
fn generate_signing_secret() -> String {
    let a = uuid::Uuid::new_v4().simple().to_string();
    let b = uuid::Uuid::new_v4().simple().to_string();
    format!("whsec_{a}{b}")
}

async fn wait_for_interrupt() {
    // A failure to install the handler should not crash the stream; if Ctrl-C is
    // unavailable we simply never resolve and rely on the loop running forever.
    if tokio::signal::ctrl_c().await.is_err() {
        std::future::pending::<()>().await;
    }
}

/// A human-readable banner summary of the run configuration.
fn config_banner(config: &ListenConfig) -> output::ListenBanner {
    output::ListenBanner {
        forward_to: config.forward_to.as_ref().map(|u| u.to_string()),
        inboxes: config.inbox_filter.clone(),
        events: config.event_globs.clone(),
        print: config.print,
        wait: config.wait,
        replay: config.replay.as_ref().map(describe_replay),
        state_file: config.state_file.display().to_string(),
        signing_secret: config.signing_secret.clone(),
    }
}

fn describe_replay(mode: &ReplayMode) -> String {
    match mode {
        ReplayMode::All => "all history".to_string(),
        ReplayMode::Count(n) => format!("last {n} events"),
        ReplayMode::Since(d) => format!("last {}s", d.as_secs()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn event(event_type: &str, inbox: Option<&str>) -> LedgerEvent {
        LedgerEvent {
            event_id: "evt_1".to_string(),
            event_type: event_type.to_string(),
            seq: Some(1),
            partition_key: inbox.map(|i| format!("inbox:{i}")),
            inbox_id: inbox.map(|i| i.to_string()),
            thread_id: None,
            idempotency_key: None,
            outbound_email_id: None,
            message_id: Some("msg_1".to_string()),
            provider_message_id: None,
            occurred_at: Some("2026-06-11T00:00:00Z".to_string()),
            created_at: Some("2026-06-11T00:00:01Z".to_string()),
            data: serde_json::json!({ "subject": "Hi" }),
        }
    }

    #[test]
    fn loopback_http_forward_targets_are_allowed() {
        assert!(validate_forward_target("http://localhost:3000/webhook").is_ok());
        assert!(validate_forward_target("http://127.0.0.1:3000/hook").is_ok());
        assert!(validate_forward_target("http://[::1]:3000/hook").is_ok());
        assert!(validate_forward_target("http://dev.localhost/hook").is_ok());
    }

    #[test]
    fn non_loopback_http_forward_targets_are_rejected() {
        let error = validate_forward_target("http://example.com/hook")
            .expect_err("plain http to a remote host must be rejected");
        assert!(error.to_string().contains("non-loopback"));
    }

    #[test]
    fn https_non_loopback_forward_targets_are_allowed() {
        assert!(validate_forward_target("https://staging.example.com/hook").is_ok());
    }

    #[test]
    fn non_http_schemes_are_rejected() {
        assert!(validate_forward_target("ftp://localhost/x").is_err());
        assert!(validate_forward_target("file:///etc/passwd").is_err());
    }

    #[test]
    fn glob_matching_supports_wildcards_and_exact() {
        assert!(glob_matches("*", "anything.here"));
        assert!(glob_matches("all", "message.received"));
        assert!(glob_matches("message.*", "message.received"));
        assert!(glob_matches("message.*", "message.quarantined"));
        assert!(!glob_matches("message.*", "domain.verified"));
        assert!(glob_matches("*.received", "message.received"));
        assert!(!glob_matches("*.received", "message.bounced"));
        assert!(glob_matches("message.delivered", "message.delivered"));
        assert!(!glob_matches("message.delivered", "message.bounced"));
    }

    #[test]
    fn event_type_matching_uses_default_globs() {
        let config_globs: Vec<String> = DEFAULT_EVENT_GLOBS.iter().map(|s| s.to_string()).collect();
        assert!(matches_event_type(
            &event("message.received", Some("a")),
            &config_globs
        ));
        assert!(matches_event_type(
            &event("message.quarantined", Some("a")),
            &config_globs
        ));
        assert!(!matches_event_type(
            &event("message.delivered", None),
            &config_globs
        ));
    }

    #[test]
    fn inbox_filter_matches_only_requested_ids() {
        let filter = vec!["inbox_a".to_string(), "inbox_b".to_string()];
        assert!(matches_inbox(
            &event("message.received", Some("inbox_a")),
            &filter
        ));
        assert!(matches_inbox(
            &event("message.received", Some("inbox_b")),
            &filter
        ));
        assert!(!matches_inbox(
            &event("message.received", Some("inbox_c")),
            &filter
        ));
        // Empty filter matches everything.
        assert!(matches_inbox(
            &event("message.received", Some("inbox_c")),
            &[]
        ));
        // No-inbox events only show in unfiltered mode.
        assert!(!matches_inbox(&event("message.delivered", None), &filter));
    }

    #[test]
    fn parses_replay_count_all_and_durations() {
        assert_eq!(parse_replay("all").unwrap(), ReplayMode::All);
        assert_eq!(parse_replay("ALL").unwrap(), ReplayMode::All);
        assert_eq!(parse_replay("50").unwrap(), ReplayMode::Count(50));
        assert_eq!(
            parse_replay("1h").unwrap(),
            ReplayMode::Since(Duration::from_secs(3_600))
        );
        assert_eq!(
            parse_replay("30m").unwrap(),
            ReplayMode::Since(Duration::from_secs(1_800))
        );
        assert_eq!(
            parse_replay("2d").unwrap(),
            ReplayMode::Since(Duration::from_secs(172_800))
        );
        assert!(parse_replay("0").is_err());
        assert!(parse_replay("bogus").is_err());
        assert!(parse_replay("1y").is_err());
    }

    #[test]
    fn signing_secret_has_whsec_prefix_and_is_unique() {
        let a = generate_signing_secret();
        let b = generate_signing_secret();
        assert!(a.starts_with("whsec_"));
        assert!(b.starts_with("whsec_"));
        assert_ne!(a, b);
    }

    #[test]
    fn webhook_body_is_byte_compatible_with_production_shape() {
        let body = webhook_body(&event("message.received", Some("inbox_a")));
        // id/type/createdAt/data are the always-present production fields and use
        // the camelCase keys the backend WebhookEvent serializes.
        assert_eq!(body["id"], "evt_1");
        assert_eq!(body["type"], "message.received");
        // occurredAt is preferred for createdAt (mirrors events/replay).
        assert_eq!(body["createdAt"], "2026-06-11T00:00:00Z");
        assert_eq!(body["data"]["subject"], "Hi");
    }

    #[test]
    fn webhook_body_falls_back_to_created_at_when_no_occurred_at() {
        let mut e = event("message.delivered", None);
        e.occurred_at = None;
        let body = webhook_body(&e);
        assert_eq!(body["createdAt"], "2026-06-11T00:00:01Z");
    }

    #[test]
    fn fingerprint_is_stable_and_filter_sensitive() {
        let a = filter_fingerprint("key1", &["inbox_a".to_string()], &["message.*".to_string()]);
        let a_again =
            filter_fingerprint("key1", &["inbox_a".to_string()], &["message.*".to_string()]);
        assert_eq!(a, a_again);
        // Order-independent for the inbox/event sets.
        let reordered =
            filter_fingerprint("key1", &["inbox_a".to_string()], &["message.*".to_string()]);
        assert_eq!(a, reordered);
        // Different key or filters => different fingerprint.
        let different_key =
            filter_fingerprint("key2", &["inbox_a".to_string()], &["message.*".to_string()]);
        assert_ne!(a, different_key);
        let different_filter =
            filter_fingerprint("key1", &["inbox_b".to_string()], &["message.*".to_string()]);
        assert_ne!(a, different_filter);
        assert_eq!(a.len(), 16);
    }

    #[test]
    fn cursor_file_round_trips_and_persists_privately() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("nested").join("listen.cursor");
        let config = ListenConfig {
            forward_to: None,
            inbox_filter: vec!["inbox_a".to_string()],
            server_inbox_id: Some("inbox_a".to_string()),
            event_globs: vec!["message.*".to_string()],
            server_event_type: None,
            print: PrintMode::Compact,
            wait: 25,
            max_forward_retries: 5,
            signing_secret: None,
            state_file: path.clone(),
            no_resume: false,
            replay: None,
        };
        flush_cursor(&config, Some("cursor_xyz"));
        assert_eq!(load_cursor(&path).as_deref(), Some("cursor_xyz"));

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
            assert_eq!(mode, 0o600);
        }
    }

    #[test]
    fn load_cursor_ignores_missing_or_corrupt_file() {
        let dir = tempfile::tempdir().unwrap();
        let missing = dir.path().join("nope.cursor");
        assert_eq!(load_cursor(&missing), None);

        let corrupt = dir.path().join("corrupt.cursor");
        std::fs::write(&corrupt, b"not json").unwrap();
        assert_eq!(load_cursor(&corrupt), None);
    }

    #[test]
    fn format_unix_rfc3339_matches_known_instants() {
        assert_eq!(format_unix_rfc3339(0), "1970-01-01T00:00:00Z");
        // 2026-06-11T00:00:00Z
        assert_eq!(format_unix_rfc3339(1_781_136_000), "2026-06-11T00:00:00Z");
        // A non-midnight instant exercises the time-of-day path.
        assert_eq!(format_unix_rfc3339(1_781_178_645), "2026-06-11T11:50:45Z");
    }

    #[test]
    fn default_state_file_lives_under_dairo_config_dir() {
        // Only assert the shape that does not depend on the host config dir.
        let path = default_state_file(
            "dairo_key_123",
            &["inbox_a".to_string()],
            &["message.*".to_string()],
        )
        .unwrap();
        let name = path.file_name().unwrap().to_string_lossy();
        assert!(name.starts_with("listen-"));
        assert!(name.ends_with(".cursor"));
        assert!(path.parent().unwrap().ends_with("dairo"));
    }
}
