mod api;
mod cli;
mod config;
mod output;

use anyhow::{Context, Result};
use api::{
    ApiClient, CreateApiKeyRequest, CreateDomainRequest, CreateInboxRequest, CreateWebhookRequest,
    MessageListQuery, SendEmailAttachment, SendEmailRequest, ThreadListQuery,
};
use base64::{engine::general_purpose::STANDARD as BASE64_STANDARD, Engine as _};
use clap::Parser;
use cli::{
    ApiKeyCommand, AttachmentCommand, AuthCommand, Cli, Command, DomainCommand, InboxCommand,
    MessageCommand, ThreadCommand, WebhookCommand,
};
use config::Config;
use output::OutputFormat;
use serde_json::json;
use std::{
    collections::HashSet,
    ffi::OsString,
    path::{Path, PathBuf},
    process::ExitCode,
};

#[tokio::main]
async fn main() -> ExitCode {
    let raw_args: Vec<OsString> = std::env::args_os().collect();
    let json_output = args_request_json(&raw_args);

    if rejects_positional_token(&raw_args) {
        print_error_message(
            "token must be provided on stdin; run `printf '%s' \"$DAIRO_API_KEY\" | dairo auth token set`",
            json_output,
            "usage_error",
        );
        return ExitCode::FAILURE;
    }

    let cli = match Cli::try_parse_from(&raw_args) {
        Ok(cli) => cli,
        Err(error) => {
            if error.exit_code() == 0 {
                let _ = error.print();
                return ExitCode::SUCCESS;
            }
            if json_output {
                print_error_message(&error.to_string(), true, "usage_error");
            } else {
                let _ = error.print();
            }
            return ExitCode::FAILURE;
        }
    };
    let json_output = cli.json;

    match run(cli).await {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            print_error(&error, json_output);
            ExitCode::FAILURE
        }
    }
}

async fn run(cli: Cli) -> Result<()> {
    let config_path = Config::path()?;

    match cli.command {
        Command::Auth { command } => match command {
            AuthCommand::Token(command) => {
                let token = command.token_value()?;
                let mut config = Config::load_from_path(&config_path)?;
                config.api_key = Some(token);
                config.save_to_path(&config_path)?;
                println!("Dairo API token saved to {}.", config_path.display());
                Ok(())
            }
        },
        command => {
            let config = Config::load_from_path(&config_path)?;
            let api_key = config.resolve_api_key()?;
            let base_url = cli
                .api_url
                .or_else(|| std::env::var("DAIRO_API_URL").ok())
                .or(config.api_url)
                .unwrap_or_else(|| api::DEFAULT_BASE_URL.to_string());
            let client = ApiClient::new(base_url, api_key)?;
            let format = OutputFormat::from_json_flag(cli.json);

            match command {
                Command::Domain { command } => match command {
                    DomainCommand::List => {
                        let response = client.list_domains().await?;
                        output::print_domains(&response.domains, format)
                    }
                    DomainCommand::Add { domain } => {
                        let response = client
                            .create_domain(&CreateDomainRequest { domain })
                            .await?;
                        output::print_domains(&response.domains, format)
                    }
                    DomainCommand::Recheck { domain } => {
                        let response = client.recheck_domain(&domain).await?;
                        output::print_domains(&response.domains, format)
                    }
                    DomainCommand::Delete { domain } => {
                        let response = client.delete_domain(&domain).await?;
                        output::print_domains(&response.domains, format)
                    }
                },
                Command::Inbox { command } => match command {
                    InboxCommand::List => {
                        let response = client.list_inboxes().await?;
                        output::print_inboxes(&response.inboxes, format)
                    }
                    InboxCommand::Create { username, domain } => {
                        let response = client
                            .create_inbox(&CreateInboxRequest {
                                username,
                                domain,
                                agent: None,
                                mode: None,
                            })
                            .await?;
                        output::print_inbox(&response.inbox, format)
                    }
                    InboxCommand::Delete { inbox_id } => {
                        let response = client.delete_inbox(&inbox_id).await?;
                        output::print_delete_response(&response, "inbox", format)
                    }
                },
                Command::Message { command } => match command {
                    MessageCommand::List {
                        inbox_id,
                        thread_id,
                        direction,
                        limit,
                        cursor,
                    } => {
                        let response = client
                            .list_messages(&MessageListQuery {
                                inbox_id,
                                thread_id,
                                direction,
                                limit,
                                cursor,
                            })
                            .await?;
                        output::print_messages(&response.messages, format)
                    }
                    MessageCommand::Get { message_id } => {
                        let response = client.get_message(&message_id).await?;
                        output::print_message(&response.message, format)
                    }
                    MessageCommand::DownloadAttachments { message_id, out } => {
                        let response = client.get_message(&message_id).await?;
                        if response.message.attachments.is_empty() {
                            println!("No attachments found for message {message_id}.");
                            Ok(())
                        } else {
                            std::fs::create_dir_all(&out).with_context(|| {
                                format!("creating output directory {}", out.display())
                            })?;
                            let mut used_paths = HashSet::new();
                            for attachment in response.message.attachments {
                                let bytes =
                                    client.download_attachment_bytes(&attachment.id).await?;
                                let path = unique_attachment_output_path(
                                    &out,
                                    attachment.filename.as_deref(),
                                    &attachment.id,
                                    &mut used_paths,
                                )?;
                                write_download(&path, &bytes)?;
                                println!("Downloaded {} bytes to {}", bytes.len(), path.display());
                            }
                            Ok(())
                        }
                    }
                },
                Command::Attachment { command } => match command {
                    AttachmentCommand::Url { attachment_id } => {
                        let response = client.get_attachment_url(&attachment_id).await?;
                        output::print_attachment_url(&response, format)
                    }
                    AttachmentCommand::Download { attachment_id, out } => {
                        let metadata = client.get_attachment_url(&attachment_id).await?;
                        let bytes = client.download_attachment_bytes(&attachment_id).await?;
                        let path = attachment_output_path(
                            &out,
                            metadata.attachment.filename.as_deref(),
                            &attachment_id,
                        )?;
                        write_download(&path, &bytes)?;
                        println!("Downloaded {} bytes to {}", bytes.len(), path.display());
                        Ok(())
                    }
                },
                Command::Thread { command } => match command {
                    ThreadCommand::List {
                        inbox_id,
                        limit,
                        cursor,
                    } => {
                        let response = client
                            .list_threads(&ThreadListQuery {
                                inbox_id,
                                limit,
                                cursor,
                            })
                            .await?;
                        output::print_threads(&response.threads, format)
                    }
                    ThreadCommand::Get { thread_id } => {
                        let response = client.get_thread(&thread_id).await?;
                        output::print_thread(&response.thread, format)
                    }
                },
                Command::Webhook { command } => match command {
                    WebhookCommand::List => {
                        let response = client.list_webhooks().await?;
                        output::print_webhooks(&response.webhooks, format)
                    }
                    WebhookCommand::Create { url, events } => {
                        let response = client
                            .create_webhook(&CreateWebhookRequest { url, events })
                            .await?;
                        output::print_created_webhook(&response, format)
                    }
                    WebhookCommand::Delete { webhook } => {
                        let response = client.delete_webhook(&webhook).await?;
                        output::print_delete_response(&response, "webhook", format)
                    }
                },
                Command::ApiKey { command } => match command {
                    ApiKeyCommand::List => {
                        let response = client.list_api_keys().await?;
                        output::print_api_keys(&response.api_keys, format)
                    }
                    ApiKeyCommand::Create { name, scopes } => {
                        let response = client
                            .create_api_key(&CreateApiKeyRequest { name, scopes })
                            .await?;
                        output::print_created_api_key(&response, format)
                    }
                    ApiKeyCommand::Revoke { api_key_id } => {
                        let response = client.revoke_api_key(&api_key_id).await?;
                        output::print_delete_response(&response, "API key", format)
                    }
                },
                Command::Send {
                    inbox_id,
                    mut to,
                    subject,
                    text,
                    attachments,
                } => {
                    normalize_recipients(&mut to)?;
                    let attachments = read_send_attachments(&attachments)?;
                    let response = client
                        .send_email(&SendEmailRequest {
                            inbox_id,
                            to,
                            cc: None,
                            bcc: None,
                            subject,
                            text,
                            html: None,
                            attachments,
                            idempotency_key: None,
                        })
                        .await?;
                    output::print_send_result(&response, format)
                }
                Command::Auth { .. } => unreachable!("auth handled before API client construction"),
            }
            .context("failed to print command output")
        }
    }
}

fn read_send_attachments(paths: &[PathBuf]) -> Result<Option<Vec<SendEmailAttachment>>> {
    if paths.is_empty() {
        return Ok(None);
    }
    let mut attachments = Vec::with_capacity(paths.len());
    for path in paths {
        let bytes = std::fs::read(path)
            .with_context(|| format!("failed to read attachment {}", path.display()))?;
        anyhow::ensure!(!bytes.is_empty(), "attachment {} is empty", path.display());
        let filename = path
            .file_name()
            .and_then(|value| value.to_str())
            .filter(|value| !value.trim().is_empty())
            .map(str::to_string)
            .with_context(|| format!("attachment {} has no valid filename", path.display()))?;
        attachments.push(SendEmailAttachment {
            content_type: guess_content_type(path),
            filename,
            content_base64: BASE64_STANDARD.encode(bytes),
        });
    }
    Ok(Some(attachments))
}

fn guess_content_type(path: &Path) -> String {
    match path
        .extension()
        .and_then(|value| value.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase()
        .as_str()
    {
        "pdf" => "application/pdf",
        "txt" => "text/plain",
        "html" | "htm" => "text/html",
        "csv" => "text/csv",
        "json" => "application/json",
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        _ => "application/octet-stream",
    }
    .to_string()
}

fn normalize_recipients(recipients: &mut Vec<String>) -> Result<()> {
    for recipient in recipients.iter_mut() {
        *recipient = recipient.trim().to_string();
    }
    recipients.retain(|recipient| !recipient.is_empty());
    anyhow::ensure!(
        !recipients.is_empty(),
        "send requires at least one non-empty --to recipient"
    );
    Ok(())
}

fn print_error(error: &anyhow::Error, json_output: bool) {
    if json_output {
        print_error_message(&error.to_string(), true, "command_failed");
    } else {
        eprintln!("Error: {error:#}");
    }
}

fn print_error_message(message: &str, json_output: bool, code: &str) {
    if json_output {
        let payload = json!({
            "error": {
                "message": message,
                "code": code,
                "status": null,
            }
        });
        eprintln!(
            "{}",
            serde_json::to_string_pretty(&payload).unwrap_or_else(|_| {
                r#"{"error":{"message":"command failed","code":"command_failed","status":null}}"#
                    .to_string()
            })
        );
    } else {
        eprintln!("Error: {message}");
    }
}

fn attachment_output_path(
    out: &Path,
    filename: Option<&str>,
    attachment_id: &str,
) -> Result<PathBuf> {
    if out.extension().is_some() || (out.exists() && out.is_file()) {
        return Ok(out.to_path_buf());
    }
    let name = filename
        .map(safe_filename)
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| format!("{attachment_id}.bin"));
    Ok(out.join(name))
}

fn unique_attachment_output_path(
    out: &Path,
    filename: Option<&str>,
    attachment_id: &str,
    used_paths: &mut HashSet<PathBuf>,
) -> Result<PathBuf> {
    let mut path = attachment_output_path(out, filename, attachment_id)?;
    if out.extension().is_some() || (out.exists() && out.is_file()) {
        anyhow::ensure!(
            used_paths.insert(path.clone()),
            "multiple attachments would write to {}; pass an output directory for message downloads",
            path.display()
        );
        return Ok(path);
    }
    if used_paths.insert(path.clone()) && !path.exists() {
        return Ok(path);
    }
    let stem = path
        .file_stem()
        .and_then(|value| value.to_str())
        .filter(|value| !value.is_empty())
        .unwrap_or(attachment_id)
        .to_string();
    let extension = path
        .extension()
        .and_then(|value| value.to_str())
        .map(str::to_string);
    for index in 2..10_000 {
        let mut candidate_name = format!("{stem}-{index}");
        if let Some(extension) = &extension {
            candidate_name.push('.');
            candidate_name.push_str(extension);
        }
        path = out.join(candidate_name);
        if used_paths.insert(path.clone()) && !path.exists() {
            return Ok(path);
        }
    }
    anyhow::bail!("could not choose a unique filename for attachment {attachment_id}")
}

fn safe_filename(filename: &str) -> String {
    filename
        .chars()
        .map(|ch| match ch {
            '/' | '\\' | '\0' | '\r' | '\n' => '_',
            _ => ch,
        })
        .collect::<String>()
        .trim()
        .trim_start_matches('.')
        .to_string()
}

fn write_download(path: &Path, bytes: &[u8]) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("creating output directory {}", parent.display()))?;
    }
    std::fs::write(path, bytes).with_context(|| format!("writing {}", path.display()))
}

fn args_request_json(args: &[OsString]) -> bool {
    args.iter().skip(1).any(|arg| {
        let arg = arg.to_string_lossy();
        arg == "--json" || arg.starts_with("--json=")
    })
}

fn rejects_positional_token(args: &[OsString]) -> bool {
    let mut command_words = Vec::new();
    let mut iter = args.iter().skip(1);

    while let Some(arg) = iter.next() {
        if arg == "--json" {
            continue;
        }
        if arg == "--api-url" {
            let _ = iter.next();
            continue;
        }
        let Some(arg_str) = arg.to_str() else {
            continue;
        };
        if arg_str.starts_with("--json=") || arg_str.starts_with("--api-url=") {
            continue;
        }
        command_words.push(arg_str);
    }

    matches!(command_words.as_slice(), ["auth", "token", "set", next, ..] if *next != "--help" && *next != "-h")
}
