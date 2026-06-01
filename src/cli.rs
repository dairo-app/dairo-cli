use anyhow::{Context, Result};
use clap::{Args, Parser, Subcommand};
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
    /// Send an email from a Dairo inbox.
    Send {
        #[arg(long = "inbox-id")]
        inbox_id: String,
        #[arg(long, required = true, action = clap::ArgAction::Append)]
        to: Vec<String>,
        #[arg(long, default_value = "")]
        subject: String,
        #[arg(long)]
        text: String,
    },
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
    /// Print a short-lived signed download URL.
    Url { attachment_id: String },
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
        #[arg(long = "event", required = true)]
        events: Vec<String>,
    },
    /// Delete a webhook by ID or URL.
    Delete { webhook: String },
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
        let cli = Cli::parse_from([
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
        ]);

        match cli.command {
            Command::Send {
                inbox_id,
                to,
                subject,
                text,
            } => {
                assert_eq!(inbox_id, "inbox_123");
                assert_eq!(to, vec!["max@example.com"]);
                assert_eq!(subject, "Hello");
                assert_eq!(text, "Body");
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
                assert_eq!(events, vec!["message.received", "email.delivered"]);
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
                command: ApiKeyCommand::Create { name, scopes },
            } => {
                assert_eq!(name, "CI");
                assert_eq!(scopes, vec!["mail:send", "mail:read"]);
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
}
