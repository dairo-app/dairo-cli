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
    /// List outbound delivery events (delivered, bounced, complained, ...).
    Events {
        #[arg(long = "email-id")]
        email_id: Option<String>,
        #[arg(long)]
        limit: Option<u32>,
    },
    /// List only bounce events.
    Bounces {
        #[arg(long = "email-id")]
        email_id: Option<String>,
        #[arg(long)]
        limit: Option<u32>,
    },
    /// List only complaint events (recipients who reported spam).
    Complaints {
        #[arg(long = "email-id")]
        email_id: Option<String>,
        #[arg(long)]
        limit: Option<u32>,
    },
}

#[derive(Debug, Args)]
#[command(group(
    ArgGroup::new("body")
        .required(true)
        .multiple(true)
        .args(["text", "html", "react_source"])
))]
pub struct SendArgs {
    #[arg(long = "inbox-id")]
    pub inbox_id: String,
    #[arg(long, required = true, action = clap::ArgAction::Append)]
    pub to: Vec<String>,
    #[arg(long, default_value = "")]
    pub subject: String,
    #[arg(long)]
    pub text: Option<String>,
    #[arg(long)]
    pub html: Option<String>,
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
}

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
            }) => {
                assert_eq!(inbox_id, "inbox_123");
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
            }) => {
                assert_eq!(inbox_id, "inbox_123");
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
}
