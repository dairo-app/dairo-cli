use anyhow::Result;

use crate::api::{Domain, Inbox, SendEmailResponse};

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
