use anyhow::Result;

use crate::cli::PrintMode;
use crate::mcp_install::McpInstallReport;

use crate::api::{
    ApiKey, AttachmentDownloadUrlResponse, BatchDeleteResult, CreateApiKeyResponse,
    CreateWebhookResponse, Domain, EmailList, EmailListDetailResponse, EmailListImportResponse,
    EmailListSendResponse, Inbox, LedgerEvent, Message, SendEmailResponse, SendEmailWarning,
    Thread, Webhook, WhoamiResponse,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OutputFormat {
    Human,
    Json,
}

impl OutputFormat {
    pub fn from_json_flag(json: bool) -> Self {
        if json {
            Self::Json
        } else {
            Self::Human
        }
    }
}

/// Pretty-prints a raw JSON value. Used for outbound history/events, whose
/// shape is a thin pass-through of the API response.
pub fn print_json(value: &serde_json::Value, _format: OutputFormat) -> Result<()> {
    println!("{}", serde_json::to_string_pretty(value)?);
    Ok(())
}

/// Returns a copy of a unified list envelope (`{ "object": "list", "data": [...] }`)
/// keeping only events whose `type` matches `kind` ("bounce"/"complaint"),
/// case-insensitively (stored SES types are capitalized) plus the webhook form
/// (email.bounced/complained). The per-email events endpoint
/// (`GET /v1/emails/{id}/events`) now wraps its rows under `data`.
pub fn filter_events_of_type(mut value: serde_json::Value, kind: &str) -> serde_json::Value {
    let webhook_form = match kind {
        "bounce" => "email.bounced",
        "complaint" => "email.complained",
        _ => "",
    };
    if let Some(events) = value.get_mut("data").and_then(|v| v.as_array_mut()) {
        events.retain(|event| {
            event
                .get("type")
                .and_then(|t| t.as_str())
                .map(|t| {
                    t.eq_ignore_ascii_case(kind)
                        || (!webhook_form.is_empty() && t.eq_ignore_ascii_case(webhook_form))
                })
                .unwrap_or(false)
        });
    }
    value
}

pub fn print_mcp_install(reports: &[McpInstallReport], format: OutputFormat) -> Result<()> {
    if format == OutputFormat::Json {
        println!(
            "{}",
            serde_json::to_string_pretty(&reports_to_json(reports))?
        );
        return Ok(());
    }
    println!("Dairo MCP install complete. No API key was printed.");
    for report in reports {
        println!(
            "- {}: {} ({})",
            report.client,
            report.action,
            report.path.display()
        );
        println!("  verify: {}", report.verify);
    }
    Ok(())
}

fn reports_to_json(reports: &[McpInstallReport]) -> serde_json::Value {
    serde_json::json!({
        "servers": reports.iter().map(|report| serde_json::json!({
            "client": report.client,
            "path": report.path.display().to_string(),
            "action": report.action,
            "verify": report.verify
        })).collect::<Vec<_>>()
    })
}

/// Human-readable report for `dairo init`. The JSON form is emitted by
/// `init::run` itself (it owns the full manifest shape); this renders the
/// terminal output: the per-file create/skip/merge plan, install status, the
/// optional `whoami` line, and a "Next steps" block. Modeled on
/// [`print_mcp_install`].
pub fn print_init(
    framework: &str,
    dir: &str,
    reports: &[crate::init::InitReport],
    install_summary: &str,
    verify: Option<&str>,
    next_steps: &[String],
) {
    println!("Dairo {framework} starter scaffolded in {dir}");
    for report in reports {
        println!("  {:<14} {}", report.action, report.rel_path);
    }
    println!("Dependencies: {install_summary}");
    if let Some(verify) = verify {
        println!("Connectivity: {verify}");
    }
    if !next_steps.is_empty() {
        println!("Next steps:");
        for (index, step) in next_steps.iter().enumerate() {
            println!("  {}. {step}", index + 1);
        }
    }
}

pub fn print_whoami(response: &WhoamiResponse, format: OutputFormat) -> Result<()> {
    if format == OutputFormat::Json {
        println!("{}", serde_json::to_string_pretty(response)?);
        return Ok(());
    }

    println!("User: {}", response.user_id);
    if let Some(workspace_id) = &response.workspace_id {
        println!("Workspace: {workspace_id}");
    }
    println!("API key: {}", response.api_key.id);
    println!("Scopes: {}", response.api_key.scopes.join(","));
    if !response.api_key.allowed_ips.is_empty() {
        println!("Allowed IPs: {}", response.api_key.allowed_ips.join(","));
    }
    println!("Plan: {}", response.plan);
    println!(
        "Storage: {} used / {} limit ({} left)",
        format_bytes(response.storage.used_bytes),
        format_bytes(response.storage.limit_bytes),
        format_bytes(response.storage.remaining_bytes)
    );
    if let Some(breakdown) = response.storage.breakdown.as_object() {
        let mail_body = breakdown
            .get("mailBodyBytes")
            .and_then(serde_json::Value::as_i64)
            .unwrap_or(0);
        let attachments = breakdown
            .get("attachmentBytes")
            .and_then(serde_json::Value::as_i64)
            .unwrap_or(0);
        let files = breakdown
            .get("fileBytes")
            .and_then(serde_json::Value::as_i64)
            .unwrap_or(0);
        let expiring = breakdown
            .get("expiringFileBytes")
            .and_then(serde_json::Value::as_i64)
            .unwrap_or(0);
        let objects = breakdown
            .get("activeFileObjects")
            .and_then(serde_json::Value::as_i64)
            .unwrap_or(0);
        println!(
            "Breakdown: bodies {}, attachments {}, files {}, expiring {}, active file objects {}",
            format_bytes(mail_body),
            format_bytes(attachments),
            format_bytes(files),
            format_bytes(expiring),
            objects
        );
    }
    Ok(())
}

fn format_bytes(bytes: i64) -> String {
    const UNITS: [&str; 5] = ["B", "KB", "MB", "GB", "TB"];
    let mut value = bytes.max(0) as f64;
    let mut unit = 0usize;
    while value >= 1024.0 && unit < UNITS.len() - 1 {
        value /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{} {}", bytes.max(0), UNITS[unit])
    } else {
        format!("{value:.2} {}", UNITS[unit])
    }
}

pub fn print_domains(domains: &[Domain], format: OutputFormat) -> Result<()> {
    if format == OutputFormat::Json {
        println!("{}", serde_json::to_string_pretty(domains)?);
        return Ok(());
    }

    if domains.is_empty() {
        println!("No domains found.");
        return Ok(());
    }

    println!(
        "{:<32} {:<12} {:<14} DNS RECORDS",
        "DOMAIN", "STATUS", "REGION"
    );
    for domain in domains {
        println!(
            "{:<32} {:<12} {:<14} {}",
            domain.domain,
            domain.status,
            domain.region,
            domain.records.len()
        );
    }

    Ok(())
}

pub fn print_inboxes(inboxes: &[Inbox], format: OutputFormat) -> Result<()> {
    if format == OutputFormat::Json {
        println!("{}", serde_json::to_string_pretty(inboxes)?);
        return Ok(());
    }

    if inboxes.is_empty() {
        println!("No inboxes found.");
        return Ok(());
    }

    println!("{:<38} {:<32} {:<14} MODE", "ID", "ADDRESS", "STATUS");
    for inbox in inboxes {
        println!(
            "{:<38} {:<32} {:<14} {}",
            inbox.id, inbox.address, inbox.status, inbox.mode
        );
    }

    Ok(())
}

pub fn print_inbox(inbox: &Inbox, format: OutputFormat) -> Result<()> {
    if format == OutputFormat::Json {
        println!("{}", serde_json::to_string_pretty(inbox)?);
        return Ok(());
    }

    println!("Created inbox:");
    println!("  id: {}", inbox.id);
    println!("  address: {}", inbox.address);
    println!("  status: {}", inbox.status);
    println!("  mode: {}", inbox.mode);
    Ok(())
}

pub fn print_send_result(response: &SendEmailResponse, format: OutputFormat) -> Result<()> {
    if format == OutputFormat::Json {
        println!("{}", serde_json::to_string_pretty(response)?);
        return Ok(());
    }

    println!("Email {}: {}", response.status, response.id);
    if let Some(error) = &response.error {
        println!("Error: {error}");
    }
    print_send_warnings(&response.warnings);
    Ok(())
}

pub fn print_email_lists(lists: &[EmailList], format: OutputFormat) -> Result<()> {
    if format == OutputFormat::Json {
        println!("{}", serde_json::to_string_pretty(lists)?);
        return Ok(());
    }
    if lists.is_empty() {
        println!("No email lists found.");
        return Ok(());
    }
    println!("{:<38} {:<28} {:<10} MEMBERS", "ID", "NAME", "STATUS");
    for list in lists {
        println!(
            "{:<38} {:<28} {:<10} {}",
            list.id,
            list.name,
            list.status,
            list.member_count.unwrap_or(0)
        );
    }
    Ok(())
}

pub fn print_email_list_detail(
    response: &EmailListDetailResponse,
    format: OutputFormat,
) -> Result<()> {
    if format == OutputFormat::Json {
        println!("{}", serde_json::to_string_pretty(response)?);
        return Ok(());
    }
    println!("List: {} ({})", response.list.name, response.list.id);
    println!("Members: {}", response.members.len());
    if response.members.is_empty() {
        println!("No members yet. Add one with `dairo lists add {}` or import CSV with `dairo lists import-csv {}`.", response.list.id, response.list.id);
        return Ok(());
    }
    println!("{:<36} {:<28} STATUS", "EMAIL", "NAME");
    for member in &response.members {
        println!(
            "{:<36} {:<28} {}",
            member.email,
            member.name.as_deref().unwrap_or(""),
            member.status
        );
    }
    Ok(())
}

pub fn print_email_list_import(
    response: &EmailListImportResponse,
    format: OutputFormat,
) -> Result<()> {
    if format == OutputFormat::Json {
        println!("{}", serde_json::to_string_pretty(response)?);
        return Ok(());
    }
    println!(
        "Imported {} recipient(s) into list {}.",
        response.imported, response.list_id
    );
    Ok(())
}

pub fn print_email_list_send(response: &EmailListSendResponse, format: OutputFormat) -> Result<()> {
    if format == OutputFormat::Json {
        println!("{}", serde_json::to_string_pretty(response)?);
        return Ok(());
    }
    println!(
        "Sent list '{}' to {} recipient(s) in {} batch(es).",
        response.list_name, response.recipient_count, response.batch_count
    );
    for email in &response.emails {
        println!("  - {}: {}", email.status, email.id);
        print_send_warnings(&email.warnings);
    }
    Ok(())
}

fn print_send_warnings(warnings: &[SendEmailWarning]) {
    if warnings.is_empty() {
        return;
    }

    println!("Warnings:");
    for warning in warnings {
        let recipient = warning.recipient.as_deref().unwrap_or("recipient");
        let message = warning.message.as_deref().unwrap_or_else(|| {
            if is_complaint_warning(warning) {
                "Recipient previously complained; do not contact again unless you are sure."
            } else {
                "Dairo returned a send warning."
            }
        });
        println!("  - {recipient}: {message}");

        if is_complaint_warning(warning) {
            println!("    Suggestion: do not contact this recipient again unless you are sure. Review outbound delivery events in Dairo before sending follow-up mail.");
        }
        if let Some(source_email_id) = &warning.source_outbound_email_id {
            println!("    source email id: {source_email_id}");
        }
        if let Some(provider_message_id) = &warning.provider_message_id {
            println!("    provider message id: {provider_message_id}");
        }
        if let Some(last_event_at) = &warning.last_event_at {
            println!("    last event: {last_event_at}");
        }
    }
}

fn is_complaint_warning(warning: &SendEmailWarning) -> bool {
    warning
        .reason
        .as_deref()
        .is_some_and(|reason| reason.eq_ignore_ascii_case("complaint"))
        || warning.complaint_feedback_type.is_some()
        || warning.complaint_user_agent.is_some()
}

pub fn print_webhooks(webhooks: &[Webhook], format: OutputFormat) -> Result<()> {
    if format == OutputFormat::Json {
        println!("{}", serde_json::to_string_pretty(webhooks)?);
        return Ok(());
    }

    if webhooks.is_empty() {
        println!("No webhooks found.");
        return Ok(());
    }

    println!(
        "{:<38} {:<44} {:<12} {:<24} EVENTS",
        "ID", "URL", "STATUS", "LAST DELIVERY"
    );
    for webhook in webhooks {
        println!(
            "{:<38} {:<44} {:<12} {:<24} {}",
            webhook.id,
            webhook.url,
            webhook.status,
            webhook.last_delivery_at.as_deref().unwrap_or("-"),
            webhook.events.join(",")
        );
    }
    Ok(())
}

pub fn print_created_webhook(response: &CreateWebhookResponse, format: OutputFormat) -> Result<()> {
    if format == OutputFormat::Json {
        println!("{}", serde_json::to_string_pretty(response)?);
        return Ok(());
    }

    println!("Created webhook:");
    println!("  id: {}", response.webhook.id);
    println!("  url: {}", response.webhook.url);
    println!("  status: {}", response.webhook.status);
    println!("  events: {}", response.webhook.events.join(","));
    if let Some(last_delivery_at) = &response.webhook.last_delivery_at {
        println!("  last delivery: {last_delivery_at}");
    }
    println!("  signing secret: {}", response.secret);
    println!("Store this secret now. Dairo will not show it again.");
    Ok(())
}

pub fn print_api_keys(api_keys: &[ApiKey], format: OutputFormat) -> Result<()> {
    if format == OutputFormat::Json {
        println!("{}", serde_json::to_string_pretty(api_keys)?);
        return Ok(());
    }

    if api_keys.is_empty() {
        println!("No API keys found.");
        return Ok(());
    }

    println!(
        "{:<38} {:<24} {:<18} {:<10} {:<28} ALLOWED IPS",
        "ID", "NAME", "PREFIX", "STATUS", "SCOPES"
    );
    for api_key in api_keys {
        let allowed_ips = if api_key.allowed_ips.is_empty() {
            "any".to_string()
        } else {
            api_key.allowed_ips.join(",")
        };
        println!(
            "{:<38} {:<24} {:<18} {:<10} {:<28} {}",
            api_key.id,
            api_key.name,
            api_key.prefix,
            api_key.status,
            api_key.scopes.join(","),
            allowed_ips
        );
    }
    Ok(())
}

pub fn print_created_api_key(response: &CreateApiKeyResponse, format: OutputFormat) -> Result<()> {
    if format == OutputFormat::Json {
        println!("{}", serde_json::to_string_pretty(response)?);
        return Ok(());
    }

    println!("Created API key:");
    println!("  id: {}", response.api_key.id);
    println!("  name: {}", response.api_key.name);
    println!("  prefix: {}", response.api_key.prefix);
    println!("  scopes: {}", response.api_key.scopes.join(","));
    if response.api_key.allowed_ips.is_empty() {
        println!("  allowed IPs: any");
    } else {
        println!("  allowed IPs: {}", response.api_key.allowed_ips.join(","));
    }
    println!("  secret: {}", response.secret);
    println!("Store this secret now. Dairo will not show it again.");
    Ok(())
}

pub fn print_threads(threads: &[Thread], format: OutputFormat) -> Result<()> {
    if format == OutputFormat::Json {
        println!("{}", serde_json::to_string_pretty(threads)?);
        return Ok(());
    }
    if threads.is_empty() {
        println!("No threads found.");
        return Ok(());
    }
    println!("{:<38} {:<38} {:<10} SUBJECT", "ID", "INBOX", "STATUS");
    for thread in threads {
        println!(
            "{:<38} {:<38} {:<10} {}",
            thread.id, thread.inbox_id, thread.status, thread.subject
        );
    }
    Ok(())
}

pub fn print_thread(thread: &Thread, format: OutputFormat) -> Result<()> {
    if format == OutputFormat::Json {
        println!("{}", serde_json::to_string_pretty(thread)?);
        return Ok(());
    }
    println!("Thread {}: {}", thread.id, thread.subject);
    Ok(())
}

pub fn print_messages(messages: &[Message], format: OutputFormat) -> Result<()> {
    if format == OutputFormat::Json {
        println!("{}", serde_json::to_string_pretty(messages)?);
        return Ok(());
    }
    if messages.is_empty() {
        println!("No messages found.");
        return Ok(());
    }
    println!(
        "{:<38} {:<38} {:<10} {:<12} SUBJECT",
        "ID", "INBOX", "STATUS", "ATTACHMENTS"
    );
    for message in messages {
        let attachments = if message.has_attachments { "yes" } else { "-" };
        println!(
            "{:<38} {:<38} {:<10} {:<12} {}",
            message.id, message.inbox_id, message.status, attachments, message.subject
        );
    }
    Ok(())
}

pub fn print_message(message: &Message, format: OutputFormat) -> Result<()> {
    if format == OutputFormat::Json {
        println!("{}", serde_json::to_string_pretty(message)?);
        return Ok(());
    }
    println!("Message {}: {}", message.id, message.subject);
    println!("From: {}", message.from.address);
    if !message.to.is_empty() {
        println!("To: {}", message.to.join(", "));
    }
    if let Some(received_at) = &message.received_at {
        println!("Received: {received_at}");
    }
    if let Some(text_body) = message
        .text_body
        .as_deref()
        .map(str::trim)
        .filter(|body| !body.is_empty())
    {
        println!("Body:\n{text_body}");
    } else if let Some(html_body) = message
        .html_body
        .as_deref()
        .map(str::trim)
        .filter(|body| !body.is_empty())
    {
        println!("HTML Body:\n{html_body}");
    } else if !message.text_preview.trim().is_empty() {
        println!("Preview:\n{}", message.text_preview.trim());
    } else {
        println!("Body: <empty>");
    }
    if message.attachments.is_empty() {
        if message.has_attachments {
            println!("Attachments: present; run `dairo messages get {}` for metadata if this response was from a list view.", message.id);
        } else {
            println!("Attachments: none");
        }
    } else {
        println!("Attachments:");
        for attachment in &message.attachments {
            println!(
                "  - {}  {}  {}  {} bytes",
                attachment.id,
                attachment.filename.as_deref().unwrap_or("attachment"),
                attachment
                    .content_type
                    .as_deref()
                    .unwrap_or("application/octet-stream"),
                attachment.size_bytes.unwrap_or_default()
            );
        }
    }
    Ok(())
}

pub fn print_attachment_url(
    response: &AttachmentDownloadUrlResponse,
    format: OutputFormat,
) -> Result<()> {
    if format == OutputFormat::Json {
        println!("{}", serde_json::to_string_pretty(response)?);
        return Ok(());
    }
    println!("{}", response.download_url);
    Ok(())
}

pub fn print_attachment_share_url(
    response: &AttachmentDownloadUrlResponse,
    format: OutputFormat,
) -> Result<()> {
    if format == OutputFormat::Json {
        println!("{}", serde_json::to_string_pretty(response)?);
        return Ok(());
    }
    if let Some(share_url) = &response.share_url {
        println!("{share_url}");
    } else {
        println!("{}", response.download_url);
    }
    Ok(())
}

/// Reports a successful delete. The redesign answers deletes with `204 No
/// Content` (no body), so success is implied by the call returning `Ok(())`. In
/// JSON mode we still emit a stable `{ "deleted": true, "resource": ... }`
/// acknowledgement so scripts that parsed the old `DeleteResponse` keep working.
pub fn print_deleted(resource: &str, format: OutputFormat) -> Result<()> {
    if format == OutputFormat::Json {
        println!(
            "{}",
            serde_json::to_string_pretty(&serde_json::json!({
                "deleted": true,
                "resource": resource,
            }))?
        );
        return Ok(());
    }

    println!("Deleted {resource}.");
    Ok(())
}

/// Renders the partial-success result of a batch-delete call. In JSON mode the
/// raw `batch_delete_result` envelope is echoed; in text mode a one-line summary
/// plus per-id failures are printed.
pub fn print_batch_delete_result(
    resource: &str,
    result: &BatchDeleteResult,
    format: OutputFormat,
) -> Result<()> {
    if format == OutputFormat::Json {
        println!("{}", serde_json::to_string_pretty(result)?);
        return Ok(());
    }

    println!(
        "Deleted {} {resource}(s); {} failed.",
        result.deleted.len(),
        result.failed.len()
    );
    for failure in &result.failed {
        println!("  failed {}: {}", failure.id, failure.error);
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// `dairo listen` rendering
// ---------------------------------------------------------------------------

/// Startup-banner summary of a `dairo listen` run, rendered once before the
/// stream begins.
pub struct ListenBanner {
    pub forward_to: Option<String>,
    pub inboxes: Vec<String>,
    pub events: Vec<String>,
    pub print: PrintMode,
    pub wait: u8,
    pub replay: Option<String>,
    pub state_file: String,
    /// The ephemeral signing secret to print once (if signing is enabled).
    pub signing_secret: Option<String>,
}

/// Prints the startup banner. The banner always goes to stderr so it never
/// pollutes a `--print json` stdout stream meant for piping into `jq`.
pub fn print_listen_banner(banner: &ListenBanner, json_banner: bool) {
    if json_banner {
        let payload = serde_json::json!({
            "listen": {
                "forwardTo": banner.forward_to,
                "inboxes": banner.inboxes,
                "events": banner.events,
                "print": banner.print.to_string(),
                "wait": banner.wait,
                "replay": banner.replay,
                "stateFile": banner.state_file,
                "signing": banner.signing_secret.is_some(),
            }
        });
        eprintln!(
            "{}",
            serde_json::to_string_pretty(&payload).unwrap_or_default()
        );
    } else {
        eprintln!("dairo listen — streaming events from the durable ledger. Ctrl-C to stop.");
        match &banner.forward_to {
            Some(url) => eprintln!("  forwarding to: {url}"),
            None => eprintln!("  mode: print-only (no --forward-to)"),
        }
        let inboxes = if banner.inboxes.is_empty() {
            "all inboxes".to_string()
        } else {
            banner.inboxes.join(", ")
        };
        eprintln!("  inboxes: {inboxes}");
        eprintln!("  events: {}", banner.events.join(", "));
        if let Some(replay) = &banner.replay {
            eprintln!("  replay: {replay}");
        }
        eprintln!("  long-poll: {}s", banner.wait);
        eprintln!("  state file: {}", banner.state_file);
    }
    // The signing secret is printed (once) regardless of banner format so the
    // operator can point their handler's DAIRO_WEBHOOK_SECRET at it.
    if let Some(secret) = &banner.signing_secret {
        eprintln!("  signing secret (set DAIRO_WEBHOOK_SECRET to verify): {secret}");
    }
}

/// Renders one ledger event per `--print` mode. `json` goes to stdout (pipeable);
/// `compact`/`pretty` go to stdout as the human stream.
pub fn print_listen_event(event: &LedgerEvent, mode: PrintMode) {
    match mode {
        PrintMode::Json => {
            // One raw event per line for `| jq`.
            match serde_json::to_string(event) {
                Ok(line) => println!("{line}"),
                Err(error) => eprintln!("(failed to serialize event {}: {error})", event.event_id),
            }
        }
        PrintMode::Compact => {
            let when = event
                .occurred_at
                .as_deref()
                .or(event.created_at.as_deref())
                .unwrap_or("-");
            let from = event_from(event);
            let subject = event_subject(event);
            let inbox = event.inbox_id.as_deref().unwrap_or("-");
            println!(
                "{when}  {:<22}  inbox={inbox}  {from}{subject}",
                event.event_type
            );
        }
        PrintMode::Pretty => {
            println!("event {}  ({})", event.event_type, event.event_id);
            if let Some(when) = event.occurred_at.as_deref().or(event.created_at.as_deref()) {
                println!("  at: {when}");
            }
            if let Some(inbox) = &event.inbox_id {
                println!("  inbox: {inbox}");
            }
            if let Some(message_id) = &event.message_id {
                println!("  message: {message_id}");
            }
            if let Some(thread_id) = &event.thread_id {
                println!("  thread: {thread_id}");
            }
            let from = event_from(event);
            if !from.is_empty() {
                println!("  {}", from.trim_end());
            }
            let subject = event_subject(event);
            if !subject.is_empty() {
                println!("  {}", subject.trim_start());
            }
            if let Some(seq) = event.seq {
                if let Some(partition) = &event.partition_key {
                    println!("  ledger: {partition} #{seq}");
                }
            }
        }
    }
}

/// Extracts a `from=<addr> ` fragment from the event payload when present
/// (inbound message events carry it under `data.from`).
fn event_from(event: &LedgerEvent) -> String {
    let from = event
        .data
        .get("from")
        .and_then(|value| match value {
            serde_json::Value::String(s) => Some(s.clone()),
            serde_json::Value::Object(map) => map
                .get("address")
                .and_then(|a| a.as_str())
                .map(str::to_string),
            _ => None,
        })
        .unwrap_or_default();
    if from.is_empty() {
        String::new()
    } else {
        format!("from={from} ")
    }
}

/// Extracts a `subject="..."` fragment (or the messageId fallback) from the
/// event payload for the compact log line.
fn event_subject(event: &LedgerEvent) -> String {
    if let Some(subject) = event.data.get("subject").and_then(|v| v.as_str()) {
        if !subject.is_empty() {
            return format!("subject=\"{subject}\"");
        }
    }
    match &event.message_id {
        Some(message_id) => format!("messageId={message_id}"),
        None => String::new(),
    }
}

/// Prints a forward result line for one event. `Ok(status)` is the HTTP status of
/// a successful forward; `Err(reason)` is a final failure after retries.
pub fn print_listen_forward_result(event_id: &str, result: std::result::Result<u16, &str>) {
    match result {
        Ok(status) => eprintln!("  → forwarded {event_id} (HTTP {status})"),
        Err(reason) => eprintln!("  → forward FAILED {event_id}: {reason}"),
    }
}

/// Surfaces server-reported ledger gaps (missing per-partition `seq`) as a
/// visible warning. The stream keeps going — this is a reliability signal, not a
/// fatal error.
pub fn print_listen_gaps(gaps: &[serde_json::Value]) {
    for gap in gaps {
        let partition = gap
            .get("partitionKey")
            .and_then(|v| v.as_str())
            .unwrap_or("?");
        let missing = gap
            .get("missingSeq")
            .map(|v| v.to_string())
            .unwrap_or_else(|| "[]".to_string());
        eprintln!("  ! gap detected in partition {partition}: missing seq {missing}");
    }
}

/// Prints a transient poll/persist error without tearing down the stream.
pub fn print_listen_poll_error(message: &str) {
    eprintln!("  ! {message}");
}

/// Prints the shutdown summary after Ctrl-C.
pub fn print_listen_summary(
    received: u64,
    forwarded: u64,
    forward_failed: u64,
    gaps_seen: u64,
    forwarding: bool,
    json_banner: bool,
) {
    if json_banner {
        let payload = serde_json::json!({
            "summary": {
                "received": received,
                "forwarded": forwarded,
                "forwardFailed": forward_failed,
                "gapsSeen": gaps_seen,
            }
        });
        eprintln!(
            "{}",
            serde_json::to_string_pretty(&payload).unwrap_or_default()
        );
        return;
    }
    eprintln!();
    if forwarding {
        eprintln!(
            "Stopped. {received} event(s) received, {forwarded} forwarded, {forward_failed} failed.",
        );
    } else {
        eprintln!("Stopped. {received} event(s) received.");
    }
    if gaps_seen > 0 {
        eprintln!("{gaps_seen} ledger gap(s) were detected during this run.");
    }
}
