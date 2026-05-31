mod api;
mod cli;
mod config;
mod output;

use anyhow::{Context, Result};
use api::{
    ApiClient, CreateApiKeyRequest, CreateDomainRequest, CreateInboxRequest, CreateWebhookRequest,
    SendEmailRequest,
};
use clap::Parser;
use cli::{ApiKeyCommand, AuthCommand, Cli, Command, DomainCommand, InboxCommand, WebhookCommand};
use config::Config;
use output::OutputFormat;
use serde_json::json;
use std::{ffi::OsString, process::ExitCode};

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
                } => {
                    normalize_recipients(&mut to)?;
                    let response = client
                        .send_email(&SendEmailRequest {
                            inbox_id,
                            to,
                            cc: None,
                            bcc: None,
                            subject,
                            text,
                            html: None,
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
