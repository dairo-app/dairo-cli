mod api;
mod cli;
mod config;
mod output;

use anyhow::{Context, Result};
use api::{ApiClient, CreateDomainRequest, CreateInboxRequest, SendEmailRequest};
use clap::Parser;
use cli::{AuthCommand, Cli, Command, DomainCommand, InboxCommand};
use config::Config;
use output::OutputFormat;

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();
    run(cli).await
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
                Command::Send {
                    inbox_id,
                    to,
                    subject,
                    text,
                } => {
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
