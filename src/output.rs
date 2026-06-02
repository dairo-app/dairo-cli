use anyhow::Result;

use crate::api::{
    ApiKey, AttachmentDownloadUrlResponse, CreateApiKeyResponse, CreateWebhookResponse,
    DeleteResponse, Domain, Inbox, Message, SendEmailResponse, SendEmailWarning, Thread, Webhook,
    WhoamiResponse,
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
        "{:<38} {:<24} {:<18} {:<10} SCOPES",
        "ID", "NAME", "PREFIX", "STATUS"
    );
    for api_key in api_keys {
        println!(
            "{:<38} {:<24} {:<18} {:<10} {}",
            api_key.id,
            api_key.name,
            api_key.prefix,
            api_key.status,
            api_key.scopes.join(",")
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

pub fn print_delete_response(
    response: &DeleteResponse,
    resource: &str,
    format: OutputFormat,
) -> Result<()> {
    if format == OutputFormat::Json {
        println!("{}", serde_json::to_string_pretty(response)?);
        return Ok(());
    }

    if response.deleted {
        println!("Deleted {resource}.");
    } else {
        println!("{resource} was not deleted.");
    }
    Ok(())
}
