use anyhow::Result;

use crate::api::{
    ApiKey, CreateApiKeyResponse, CreateWebhookResponse, DeleteResponse, Domain, Inbox,
    SendEmailResponse, Webhook,
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
    Ok(())
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

    println!("{:<38} {:<44} {:<12} EVENTS", "ID", "URL", "STATUS");
    for webhook in webhooks {
        println!(
            "{:<38} {:<44} {:<12} {}",
            webhook.id,
            webhook.url,
            webhook.status,
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
