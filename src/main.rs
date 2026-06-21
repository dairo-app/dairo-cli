mod api;
mod auth;
mod cli;
mod config;
mod doctor;
mod fsutil;
mod init;
mod listen;
mod mcp_catalog;
mod mcp_install;
mod output;
mod update;
mod webhook;

use anyhow::{Context, Result};
use api::{
    A2aMessageQuery, ApiClient, AuditLogQuery, CreateApiKeyRequest, CreateDomainRequest,
    CreateEmailListRequest, CreateInboxRequest, CreateWebhookRequest, EmailListMemberInput,
    EmailListMembersRequest, EventsQuery, MessageListQuery, SendEmailAttachment, SendEmailReact,
    SendEmailRequest, ThreadListQuery, VerifyAgentQuery,
};
use base64::{engine::general_purpose::STANDARD as BASE64_STANDARD, Engine as _};
use clap::CommandFactory;
use clap::Parser;
use cli::{
    A2aCommand, AgentCommand, ApiKeyCommand, AttachmentCommand, AttachmentDelivery,
    AuditLogCommand, AuthCommand, BudgetCommand, Cli, Command, ComplianceCommand,
    DedicatedIpCommand, DomainCommand, EmailListCommand, ErasureJobCommand, EventsCommand,
    InboxCommand, InboxSchemaCommand, InboxSchemaValidationMode, LoginArgs, McpCommand,
    MessageCommand, OutboundCommand, ReputationCommand, TemplateCommand, ThreadCommand,
    VerificationWaitCommand, WebhookCommand,
};
use config::{Config, StorageMode};
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

    // Select the credential-storage policy for the rest of the process: the OS
    // keychain by default, or the legacy `0600` file when `--insecure-storage`
    // is set. This must happen before any config load/save.
    config::set_storage_mode(if cli.insecure_storage {
        StorageMode::FileOnly
    } else {
        StorageMode::Auto
    });

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
        // Offline webhook verification: no API key or network required, so it is
        // handled before constructing the API client.
        Command::Webhook {
            command:
                WebhookCommand::Verify {
                    secret,
                    signature,
                    timestamp,
                    tolerance_seconds,
                },
        } => {
            verify_webhook_from_stdin(&secret, &signature, &timestamp, tolerance_seconds, cli.json)
        }
        // Scaffolding is a client-only operation: it writes embedded templates
        // and only optionally calls `whoami` (handled inside `init::run`, which
        // resolves its own config and degrades gracefully when no key is set).
        // So it is handled before the generic API-client construction that would
        // otherwise hard-require a key.
        Command::Init(args) => init::run(args, cli.json).await,
        // Completion-script generation is a pure client-only operation: no API
        // key or network is involved, so it is handled before the key-required
        // generic arm.
        Command::Completion { shell } => {
            let mut command = Cli::command();
            let bin_name = command.get_name().to_string();
            clap_complete::generate(
                clap_complete::Shell::from(shell),
                &mut command,
                bin_name,
                &mut std::io::stdout(),
            );
            Ok(())
        }
        // `update` only talks to the public GitHub releases API (or degrades
        // gracefully offline); it never needs a Dairo token.
        Command::Update => update::run(OutputFormat::from_json_flag(cli.json)).await,
        // `doctor` must run even when no token is configured (that is one of the
        // things it diagnoses), so it resolves config itself and degrades
        // gracefully rather than going through the key-required generic arm.
        Command::Doctor => {
            let config = Config::load_from_path(&config_path)?;
            let base_url = resolve_base_url(cli.api_url.as_deref(), &config);
            // An absent/blank token is reported by `doctor`, never an error here.
            let api_key = config.resolve_api_key().unwrap_or_default();
            doctor::run(
                &config,
                &base_url,
                &api_key,
                &config_path,
                OutputFormat::from_json_flag(cli.json),
            )
            .await
        }
        // Browser OAuth login does not need an existing API key (it mints one),
        // so it is handled before the generic API-client construction that would
        // otherwise hard-require a key.
        Command::Login(args) => run_login(args, &cli.api_url, &config_path).await,
        // Logout clears the local credential and best-effort revokes it
        // server-side. It must succeed even when the stored token is invalid or
        // the server is unreachable, so it builds its own client and degrades
        // gracefully rather than going through the key-required generic arm.
        Command::Logout => run_logout(&cli.api_url, &config_path).await,
        command => {
            let config = Config::load_from_path(&config_path)?;
            let api_key = config.resolve_api_key()?;
            let base_url = cli
                .api_url
                .or_else(|| std::env::var("DAIRO_API_URL").ok())
                .or(config.api_url)
                .unwrap_or_else(|| api::DEFAULT_BASE_URL.to_string());
            let client = ApiClient::new(&base_url, &api_key)?;
            let format = OutputFormat::from_json_flag(cli.json);

            match command {
                Command::Whoami => {
                    let response = client.whoami().await?;
                    output::print_whoami(&response, format)
                }
                Command::Domain { command } => match command {
                    DomainCommand::List => {
                        let response = client.list_domains().await?;
                        output::print_domains(&response.data, format)
                    }
                    DomainCommand::Add { domain } => {
                        let domain = client
                            .create_domain(&CreateDomainRequest { domain })
                            .await?;
                        output::print_domains(std::slice::from_ref(&domain), format)
                    }
                    DomainCommand::Recheck { domain } => {
                        let domain = client.recheck_domain(&domain).await?;
                        output::print_domains(std::slice::from_ref(&domain), format)
                    }
                    DomainCommand::Delete { domain } => {
                        client.delete_domain(&domain).await?;
                        output::print_deleted("domain", format)
                    }
                },
                Command::Inbox { command } => match command {
                    InboxCommand::List => {
                        let response = client.list_inboxes().await?;
                        output::print_inboxes(&response.data, format)
                    }
                    InboxCommand::Create { username, domain } => {
                        let inbox = client
                            .create_inbox(&CreateInboxRequest {
                                username,
                                domain,
                                agent: None,
                                mode: None,
                            })
                            .await?;
                        output::print_inbox(&inbox, format)
                    }
                    InboxCommand::Delete { inbox_id } => {
                        client.delete_inbox(&inbox_id).await?;
                        output::print_deleted("inbox", format)
                    }
                    InboxCommand::Schema { command } => {
                        run_inbox_schema(&client, command, format).await
                    }
                    InboxCommand::VerificationWaits { command } => {
                        run_verification_waits(&client, command, format).await
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
                        output::print_messages(&response.data, format)
                    }
                    MessageCommand::Get { message_id } => {
                        let message = client.get_message(&message_id).await?;
                        output::print_message(&message, format)
                    }
                    MessageCommand::DownloadAttachments { message_id, out } => {
                        let message = client.get_message(&message_id).await?;
                        if message.attachments.is_empty() {
                            println!("No attachments found for message {message_id}.");
                            Ok(())
                        } else {
                            std::fs::create_dir_all(&out).with_context(|| {
                                format!("creating output directory {}", out.display())
                            })?;
                            let mut used_paths = HashSet::new();
                            for attachment in message.attachments {
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
                    AttachmentCommand::Url {
                        attachment_id,
                        expiry_hours,
                    } => {
                        let response = client
                            .get_attachment_url(&attachment_id, expiry_hours)
                            .await?;
                        output::print_attachment_url(&response, format)
                    }
                    AttachmentCommand::Share {
                        attachment_id,
                        expiry_hours,
                    } => {
                        // Branded share page: must hit /link (which returns a
                        // Dairo shareUrl), not /url (a raw signed S3 URL).
                        let response = client
                            .get_attachment_link(&attachment_id, expiry_hours)
                            .await?;
                        output::print_attachment_share_url(&response, format)
                    }
                    AttachmentCommand::Download { attachment_id, out } => {
                        let metadata = client.get_attachment_url(&attachment_id, None).await?;
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
                        output::print_threads(&response.data, format)
                    }
                    ThreadCommand::Get { thread_id } => {
                        let response = client.get_thread(&thread_id).await?;
                        output::print_thread(&response.thread, format)
                    }
                },
                Command::Webhook { command } => match command {
                    WebhookCommand::List => {
                        let response = client.list_webhooks().await?;
                        output::print_webhooks(&response.data, format)
                    }
                    WebhookCommand::Create { url, events } => {
                        let events = events
                            .into_iter()
                            .map(|event| event.as_str().to_string())
                            .collect();
                        let response = client
                            .create_webhook(&CreateWebhookRequest { url, events })
                            .await?;
                        output::print_created_webhook(&response, format)
                    }
                    WebhookCommand::Delete { webhook } => {
                        client.delete_webhook(&webhook).await?;
                        output::print_deleted("webhook", format)
                    }
                    WebhookCommand::Verify { .. } => {
                        unreachable!("webhook verify is handled before API client construction")
                    }
                },
                Command::ApiKey { command } => match command {
                    ApiKeyCommand::List => {
                        let response = client.list_api_keys().await?;
                        output::print_api_keys(&response.data, format)
                    }
                    ApiKeyCommand::Create {
                        name,
                        scopes,
                        allowed_ips,
                    } => {
                        let allowed_ips = if allowed_ips.is_empty() {
                            None
                        } else {
                            Some(allowed_ips)
                        };
                        let response = client
                            .create_api_key(&CreateApiKeyRequest {
                                name,
                                scopes,
                                allowed_ips,
                            })
                            .await?;
                        output::print_created_api_key(&response, format)
                    }
                    ApiKeyCommand::Revoke { api_key_id } => {
                        client.revoke_api_key(&api_key_id).await?;
                        output::print_deleted("API key", format)
                    }
                },
                Command::Mcp { command } => match command {
                    McpCommand::Install { client, name } => {
                        let reports = mcp_install::install(client, &name, &base_url, &api_key)?;
                        output::print_mcp_install(&reports, format)
                    }
                    McpCommand::Catalog {
                        json,
                        for_me,
                        family,
                    } => {
                        let catalog = client.mcp_catalog(for_me).await?;
                        let catalog_format =
                            OutputFormat::from_json_flag(format == OutputFormat::Json || json);
                        mcp_catalog::render(&catalog, catalog_format, for_me, family.as_deref())
                    }
                },
                Command::Send(args) => {
                    let dry_run = args.dry_run;
                    let request = build_send_request(&client, args, true).await?;
                    if dry_run {
                        print_dry_run_request(&request)
                    } else {
                        let response = client.send_email(&request).await?;
                        output::print_send_result(&response, format)
                    }
                }
                Command::Outbound { command } => match command {
                    OutboundCommand::List { limit } => {
                        let response = client.list_outbound_emails(limit).await?;
                        output::print_json(&response, format)
                    }
                    OutboundCommand::Get { email_id } => {
                        let response = client.get_outbound_email(&email_id).await?;
                        output::print_json(&response, format)
                    }
                    OutboundCommand::Cancel { email_id } => {
                        let response = client.cancel_outbound_email(&email_id).await?;
                        output::print_json(&response, format)
                    }
                    OutboundCommand::Events { email_id, limit } => {
                        let response = client.list_outbound_events(&email_id, limit).await?;
                        output::print_json(&response, format)
                    }
                    OutboundCommand::Bounces { email_id, limit } => {
                        let response = client.list_outbound_events(&email_id, limit).await?;
                        output::print_json(
                            &output::filter_events_of_type(response, "bounce"),
                            format,
                        )
                    }
                    OutboundCommand::Complaints { email_id, limit } => {
                        let response = client.list_outbound_events(&email_id, limit).await?;
                        output::print_json(
                            &output::filter_events_of_type(response, "complaint"),
                            format,
                        )
                    }
                },
                Command::EmailList { command } => match command {
                    EmailListCommand::List => {
                        let response = client.list_email_lists().await?;
                        output::print_email_lists(&response.data, format)
                    }
                    EmailListCommand::Create { name, description } => {
                        let list = client
                            .create_email_list(&CreateEmailListRequest { name, description })
                            .await?;
                        output::print_email_lists(std::slice::from_ref(&list), format)
                    }
                    EmailListCommand::Get { list_id } => {
                        let response = client.get_email_list(&list_id).await?;
                        output::print_email_list_detail(&response, format)
                    }
                    EmailListCommand::Delete { list_id } => {
                        client.delete_email_list(&list_id).await?;
                        output::print_deleted("email list", format)
                    }
                    EmailListCommand::Add {
                        list_id,
                        email,
                        name,
                    } => {
                        let response = client
                            .add_email_list_members(
                                &list_id,
                                &EmailListMembersRequest {
                                    members: vec![EmailListMemberInput { email, name }],
                                },
                            )
                            .await?;
                        output::print_email_list_import(&response, format)
                    }
                    EmailListCommand::ImportCsv { list_id, file } => {
                        let members = read_email_list_csv(&file)?;
                        // The /members/import alias was removed in the redesign; the
                        // canonical /members endpoint upserts and accepts the same
                        // payload, so CSV import now posts there too.
                        let response = client
                            .add_email_list_members(
                                &list_id,
                                &EmailListMembersRequest { members },
                            )
                            .await?;
                        output::print_email_list_import(&response, format)
                    }
                    EmailListCommand::Send { list_id, send } => {
                        let dry_run = send.dry_run;
                        let request = build_send_request(&client, send, false).await?;
                        if dry_run {
                            print_dry_run_request(&request)
                        } else {
                            let response =
                                client.send_email_list(&list_id, &request).await?;
                            output::print_email_list_send(&response, format)
                        }
                    }
                },
                Command::AuditLog { command } => match command {
                    AuditLogCommand::List { limit, cursor } => {
                        let response = client
                            .list_audit_logs(&AuditLogQuery { limit, cursor })
                            .await?;
                        output::print_json(&response, format)
                    }
                },
                Command::DedicatedIp { command } => match command {
                    DedicatedIpCommand::Status => {
                        let response = client.list_dedicated_ips().await?;
                        output::print_json(&response, format)
                    }
                },
                Command::Template { command } => run_template(&client, command, format).await,
                Command::Events { command } => match command {
                    EventsCommand::List {
                        limit,
                        cursor,
                        inbox_id,
                        event_type,
                        wait,
                        tail,
                    } => {
                        let response = client
                            .list_events(&EventsQuery {
                                since: cursor,
                                limit,
                                inbox_id,
                                event_type,
                                wait,
                                tail,
                            })
                            .await?;
                        output::print_json(&serde_json::to_value(response)?, format)
                    }
                    EventsCommand::Replay {
                        since,
                        since_seq,
                        since_timestamp,
                        inbox_id,
                        until,
                        types,
                        webhook_id,
                        max_events,
                    } => {
                        let body = build_replay_request(
                            since,
                            since_seq,
                            since_timestamp,
                            inbox_id,
                            until,
                            types,
                            webhook_id,
                            max_events,
                        );
                        let response = client.replay_events(&body).await?;
                        output::print_json(&response, format)
                    }
                },
                Command::Agent { command } => match command {
                    AgentCommand::List => {
                        let response = client.list_agents().await?;
                        output::print_json(&response, format)
                    }
                    AgentCommand::Get { id_or_agent } => {
                        let response = client.get_agent(&id_or_agent).await?;
                        output::print_json(&response, format)
                    }
                    AgentCommand::Verify {
                        id,
                        agent,
                        kid,
                        sig,
                        from,
                        to,
                        subject,
                        ts,
                    } => {
                        let query = VerifyAgentQuery {
                            id,
                            agent,
                            kid,
                            sig,
                            from,
                            to,
                            subject,
                            ts,
                        };
                        anyhow::ensure!(
                            query.id.is_some() || query.agent.is_some(),
                            "agents verify requires either --id or the signature form (--agent --kid --sig)"
                        );
                        let response = client.verify_agent(&query).await?;
                        output::print_json(&response, format)
                    }
                },
                Command::Reputation { command } => match command {
                    ReputationCommand::List => {
                        let response = client.list_reputation().await?;
                        output::print_json(&response, format)
                    }
                },
                Command::Budget { command } => match command {
                    BudgetCommand::List => {
                        let response = client.list_budgets().await?;
                        output::print_json(&response, format)
                    }
                    BudgetCommand::Get { scope } => {
                        let response = client.get_budget(&scope).await?;
                        output::print_json(&response, format)
                    }
                    BudgetCommand::Set {
                        scope,
                        scope_id,
                        disabled,
                        max_sends_per_day,
                        max_new_recipients_per_hour,
                        hard_stop_on_complaint,
                    } => {
                        let body = build_set_budget_request(
                            scope,
                            scope_id,
                            disabled,
                            max_sends_per_day,
                            max_new_recipients_per_hour,
                            hard_stop_on_complaint,
                        )?;
                        let response = client.set_budget(&body).await?;
                        output::print_json(&response, format)
                    }
                    BudgetCommand::Delete { scope } => {
                        client.delete_budget(&scope).await?;
                        output::print_deleted("budget", format)
                    }
                },
                Command::Compliance { command } => match command {
                    ComplianceCommand::Residency => {
                        let response = client.compliance_residency().await?;
                        output::print_json(&response, format)
                    }
                    ComplianceCommand::ErasureJob { job_id } => {
                        let response = client.get_erasure_job(&job_id).await?;
                        output::print_json(&response, format)
                    }
                },
                Command::ErasureJobs { command } => match command {
                    ErasureJobCommand::List => {
                        let response = client.list_erasure_jobs().await?;
                        output::print_json(&response, format)
                    }
                    ErasureJobCommand::Create {
                        subject_email,
                        inbox_id,
                    } => {
                        let body = build_erasure_job_request(subject_email, inbox_id)?;
                        let response = client.create_erasure_job(&body).await?;
                        output::print_json(&response, format)
                    }
                    ErasureJobCommand::Get { job_id } => {
                        let response = client.get_erasure_job(&job_id).await?;
                        output::print_json(&response, format)
                    }
                },
                Command::A2a { command } => match command {
                    A2aCommand::List {
                        limit,
                        cursor,
                        inbox_id,
                    } => {
                        let response = client
                            .list_a2a_messages(&A2aMessageQuery {
                                limit,
                                cursor,
                                inbox_id,
                            })
                            .await?;
                        output::print_json(&response, format)
                    }
                    A2aCommand::Get { id } => {
                        let response = client.get_a2a_message(&id).await?;
                        output::print_json(&response, format)
                    }
                },
                Command::Listen(args) => {
                    // `listen` does its own rendering and its errors are already
                    // descriptive, so it bypasses the generic "failed to print
                    // command output" context the other arms share.
                    return listen::run_listen(&client, args, &api_key, cli.json).await;
                }
                Command::Auth { .. } => unreachable!("auth handled before API client construction"),
                Command::Init(_) => {
                    unreachable!("init is handled before API client construction")
                }
                Command::Login(_) => {
                    unreachable!("login is handled before API client construction")
                }
                Command::Logout => {
                    unreachable!("logout is handled before API client construction")
                }
                Command::Completion { .. } => {
                    unreachable!("completion is handled before API client construction")
                }
                Command::Doctor => {
                    unreachable!("doctor is handled before API client construction")
                }
                Command::Update => {
                    unreachable!("update is handled before API client construction")
                }
            }
            .context("failed to print command output")
        }
    }
}

/// Resolves the API base URL using the same precedence the generic command arm
/// uses: an explicit override, then `DAIRO_API_URL`, then the configured base,
/// then the public default.
fn resolve_base_url(explicit: Option<&str>, config: &Config) -> String {
    explicit
        .map(str::to_string)
        .or_else(|| std::env::var("DAIRO_API_URL").ok())
        .or_else(|| config.api_url.clone())
        .unwrap_or_else(|| api::DEFAULT_BASE_URL.to_string())
}

/// Handles `dairo login`: runs the browser OAuth (PKCE) flow and persists the
/// resulting token. The `--api-url` on the subcommand takes precedence over the
/// global `--api-url`/env/config base.
async fn run_login(
    args: LoginArgs,
    global_api_url: &Option<String>,
    config_path: &Path,
) -> Result<()> {
    let config = Config::load_from_path(config_path)?;
    let base_url = resolve_base_url(
        args.api_url.as_deref().or(global_api_url.as_deref()),
        &config,
    );
    let outcome = auth::login(&base_url, &args.scope, config_path).await?;
    // Never print the token; only the granted scopes and where it was stored.
    println!(
        "Signed in. Token saved to {}.",
        outcome.config_path.display()
    );
    if outcome.scopes.is_empty() {
        println!("Granted scopes: (none reported)");
    } else {
        println!("Granted scopes: {}", outcome.scopes.join(", "));
    }
    println!("Verify your session with `dairo whoami`.");
    Ok(())
}

/// Handles `dairo logout`: best-effort server-side revocation of the stored
/// token, then clears the credential from the local config. Always clears the
/// local config even if revocation is not addressable, and tells the user when
/// they should revoke in the dashboard.
async fn run_logout(global_api_url: &Option<String>, config_path: &Path) -> Result<()> {
    let mut config = Config::load_from_path(config_path)?;
    let Some(token) = config
        .api_key
        .clone()
        .filter(|token| !token.trim().is_empty())
    else {
        println!("No stored Dairo token to clear.");
        return Ok(());
    };
    let base_url = resolve_base_url(global_api_url.as_deref(), &config);

    // Best-effort server-side revocation. A failure here (invalid token, missing
    // keys:* scope, network error, non-https/non-loopback base) must not block
    // clearing the local credential.
    let mut revoked_server_side = false;
    let mut revoke_note: Option<String> = None;
    match api::ApiClient::new(&base_url, &token) {
        Ok(client) => match client.revoke_token_by_prefix(&token).await {
            Ok(true) => revoked_server_side = true,
            Ok(false) => {
                revoke_note = Some(
                    "could not match the stored token to an active key server-side".to_string(),
                );
            }
            Err(error) => {
                revoke_note = Some(format!("server-side revocation failed: {error}"));
            }
        },
        Err(error) => {
            revoke_note = Some(format!("server-side revocation was skipped: {error}"));
        }
    }

    config.clear_credentials();
    config.save_to_path(config_path)?;

    if revoked_server_side {
        println!(
            "Logged out. Token revoked server-side and cleared from {}.",
            config_path.display()
        );
    } else {
        println!("Cleared the stored token from {}.", config_path.display());
        if let Some(note) = revoke_note {
            println!("Note: {note}.");
        }
        println!(
            "If the token may still be active, revoke it in the Dairo dashboard \
             (https://dairo.app/app) to be safe."
        );
    }
    Ok(())
}

const MAX_INLINE_ATTACHMENT_BYTES: usize = 8 * 1024 * 1024;

async fn build_send_request(
    client: &ApiClient,
    mut args: cli::SendArgs,
    require_to: bool,
) -> Result<SendEmailRequest> {
    // Resolve the sending inbox to its id: either the given --inbox-id, or the
    // --from address looked up against the account's inboxes.
    let inbox_id = resolve_inbox_id(client, &args).await?;
    if require_to {
        normalize_recipients(&mut args.to)?;
    } else {
        args.to.clear();
    }
    let mut cc = std::mem::take(&mut args.cc);
    normalize_optional_recipients(&mut cc);
    let mut bcc = std::mem::take(&mut args.bcc);
    normalize_optional_recipients(&mut bcc);
    // Body: an inline value or a file (`-` = stdin); inline and file are mutually
    // exclusive per channel, enforced in resolve_body_source.
    let text = resolve_body_source(args.text, args.text_file, "text")?;
    let html = resolve_body_source(args.html, args.html_file, "html")?;
    let attachments = read_send_attachments(
        &args.attachments,
        args.attachment_delivery,
        args.attachment_link_expiry_hours,
    )?;
    let react = build_react_request(args.react_source, args.react_props)?;
    let reply_to = args.reply_to.and_then(non_empty_trimmed);
    let headers = parse_key_value_pairs(&args.headers, "--headers")?;
    let tags = parse_key_value_pairs(&args.tags, "--tags")?;
    // `--send-at` accepts RFC3339 (passed through) or natural language, resolved
    // to an RFC3339 string with offset relative to now.
    let send_at = match args.send_at.and_then(non_empty_trimmed) {
        Some(raw) => Some(resolve_send_at(&raw)?),
        None => None,
    };
    Ok(SendEmailRequest {
        inbox_id,
        to: args.to,
        cc: (!cc.is_empty()).then_some(cc),
        bcc: (!bcc.is_empty()).then_some(bcc),
        subject: args.subject,
        text,
        html,
        react,
        attachments,
        idempotency_key: None,
        send_at,
        ignore_complaints: args.ignore_complaints,
        reply_to,
        headers,
        tags,
    })
}

/// Parses repeated `KEY=VALUE` flag values into a sorted map, rejecting any
/// malformed pair (missing `=`, or an empty key). Returns `None` when no pairs
/// were given so the field is omitted from the wire request entirely.
fn parse_key_value_pairs(
    raw: &[String],
    flag: &str,
) -> Result<Option<std::collections::BTreeMap<String, String>>> {
    if raw.is_empty() {
        return Ok(None);
    }
    let mut map = std::collections::BTreeMap::new();
    for entry in raw {
        let (key, value) = entry
            .split_once('=')
            .with_context(|| format!("{flag} expects KEY=VALUE, got '{entry}'"))?;
        let key = key.trim();
        anyhow::ensure!(!key.is_empty(), "{flag} entry '{entry}' has an empty key");
        map.insert(key.to_string(), value.trim().to_string());
    }
    Ok(Some(map))
}

/// Resolves a `--send-at` value to an RFC3339 string with an explicit offset.
///
/// RFC3339 input is passed through unchanged (re-serialized to canonical form);
/// otherwise the value is parsed as natural language relative to the current
/// local time (e.g. "in 1 hour", "tomorrow at 9am", "next monday") and converted
/// to RFC3339.
fn resolve_send_at(raw: &str) -> Result<String> {
    // RFC3339 with an explicit offset: pass through (canonicalized).
    if let Ok(dt) = chrono::DateTime::parse_from_rfc3339(raw) {
        return Ok(dt.to_rfc3339());
    }
    // Natural language relative to now, in the local timezone.
    let now = chrono::Local::now();
    let normalized = normalize_natural_time(raw);
    let parsed =
        interim::parse_date_string(&normalized, now, interim::Dialect::Us).with_context(|| {
            format!(
                "could not parse --send-at '{raw}' as RFC3339 or a natural-language time \
             (try e.g. \"in 1 hour\", \"tomorrow at 9am\", or \"2026-06-11T09:00:00Z\")"
            )
        })?;
    Ok(parsed.to_rfc3339())
}

/// Light normalization so common English phrasings the `interim` grammar does
/// not accept verbatim still work: a leading "in " ("in 1 hour" -> "1 hour")
/// and an " at " connector ("tomorrow at 9am" -> "tomorrow 9am").
fn normalize_natural_time(raw: &str) -> String {
    let trimmed = raw.trim();
    let without_in = trimmed
        .strip_prefix("in ")
        .or_else(|| trimmed.strip_prefix("IN "))
        .or_else(|| trimmed.strip_prefix("In "))
        .unwrap_or(trimmed);
    without_in.replace(" at ", " ")
}

/// Renders a built [`SendEmailRequest`] as pretty JSON for `--dry-run`, without
/// ever emitting attachment bytes: each attachment's `contentBase64` is replaced
/// by a `byteLength` (the decoded size) so the operator sees what would be sent
/// without dumping base64 to the terminal. Nothing is sent to the API.
fn print_dry_run_request(request: &SendEmailRequest) -> Result<()> {
    let mut value = serde_json::to_value(request).context("failed to serialize send request")?;
    if let Some(attachments) = value.get_mut("attachments").and_then(|v| v.as_array_mut()) {
        for attachment in attachments {
            if let Some(obj) = attachment.as_object_mut() {
                let byte_length = obj
                    .remove("contentBase64")
                    .and_then(|v| v.as_str().map(decoded_base64_len))
                    .unwrap_or(0);
                obj.insert(
                    "byteLength".to_string(),
                    serde_json::Value::from(byte_length),
                );
            }
        }
    }
    println!("{}", serde_json::to_string_pretty(&value)?);
    Ok(())
}

/// Decoded byte length of a standard base64 string, computed from its length and
/// padding so we never have to allocate/echo the decoded bytes.
fn decoded_base64_len(b64: &str) -> usize {
    let trimmed = b64.trim();
    let len = trimmed.len();
    if len == 0 {
        return 0;
    }
    let padding = trimmed.bytes().rev().take_while(|&b| b == b'=').count();
    (len / 4) * 3 - padding
}

/// Resolves the sending inbox id from `--inbox-id` (used directly) or `--from`
/// (an address looked up against the account's inboxes). The clap `source` group
/// guarantees exactly one is present.
async fn resolve_inbox_id(client: &ApiClient, args: &cli::SendArgs) -> Result<String> {
    if let Some(id) = args
        .inbox_id
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        return Ok(id.to_string());
    }
    let from = args
        .from
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .context("provide a sending inbox with --from <address> or --inbox-id <uuid>")?;
    let address = extract_email_address(from);
    let inboxes = client.list_inboxes().await.context(
        "could not list inboxes to resolve --from (the token needs the inboxes:read scope, \
         or pass --inbox-id <uuid> directly)",
    )?;
    let mut matches = inboxes
        .data
        .into_iter()
        .filter(|inbox| inbox.address.eq_ignore_ascii_case(&address));
    match (matches.next(), matches.next()) {
        (Some(inbox), None) => Ok(inbox.id),
        (Some(_), Some(_)) => anyhow::bail!(
            "multiple inboxes match address '{address}'; use --inbox-id <uuid> to disambiguate"
        ),
        (None, _) => anyhow::bail!(
            "no inbox found for address '{address}'. Create one with \
             `dairo inbox create --domain <domain> <username>`, or list them with `dairo inbox list`."
        ),
    }
}

/// Extracts the bare email address from a `Display Name <addr>` form (else the
/// trimmed input), lowercased for case-insensitive inbox matching.
fn extract_email_address(input: &str) -> String {
    let s = input.trim();
    if let (Some(lt), Some(gt)) = (s.find('<'), s.rfind('>')) {
        if lt < gt {
            return s[lt + 1..gt].trim().to_ascii_lowercase();
        }
    }
    s.to_ascii_lowercase()
}

/// Trims and drops empties from an optional recipient list (cc/bcc), without the
/// "at least one" requirement [`normalize_recipients`] enforces for `--to`.
fn normalize_optional_recipients(recipients: &mut Vec<String>) {
    for recipient in recipients.iter_mut() {
        *recipient = recipient.trim().to_string();
    }
    recipients.retain(|recipient| !recipient.is_empty());
}

/// Resolves a body channel from an inline value or a file path (`-` = stdin).
/// Errors if both an inline value and a file are given for the same channel.
fn resolve_body_source(
    inline: Option<String>,
    file: Option<PathBuf>,
    label: &str,
) -> Result<Option<String>> {
    match (inline, file) {
        (Some(_), Some(_)) => {
            anyhow::bail!("provide either --{label} or --{label}-file, not both")
        }
        (Some(value), None) => Ok(Some(value)),
        (None, Some(path)) => Ok(Some(read_body_file(&path, label)?)),
        (None, None) => Ok(None),
    }
}

/// Reads a body file, or stdin when the path is `-`.
fn read_body_file(path: &std::path::Path, label: &str) -> Result<String> {
    if path.as_os_str() == "-" {
        let mut buf = String::new();
        std::io::Read::read_to_string(&mut std::io::stdin(), &mut buf)
            .with_context(|| format!("failed to read --{label}-file from stdin"))?;
        Ok(buf)
    } else {
        std::fs::read_to_string(path)
            .with_context(|| format!("failed to read --{label}-file: {}", path.display()))
    }
}

async fn run_inbox_schema(
    client: &ApiClient,
    command: InboxSchemaCommand,
    format: OutputFormat,
) -> Result<()> {
    match command {
        InboxSchemaCommand::Get { inbox } => {
            let response = client.get_inbox_schema(&inbox).await?;
            output::print_json(&response, format)
        }
        InboxSchemaCommand::Set {
            inbox,
            schema,
            schema_file,
            on_validation_error,
            extraction_hint,
        } => {
            let body = build_inbox_schema_request(
                schema,
                schema_file,
                on_validation_error,
                extraction_hint,
            )?;
            let response = client.set_inbox_schema(&inbox, &body).await?;
            output::print_json(&response, format)
        }
        InboxSchemaCommand::Delete { inbox } => {
            client.delete_inbox_schema(&inbox).await?;
            output::print_deleted("inbox schema", format)
        }
    }
}

async fn run_verification_waits(
    client: &ApiClient,
    command: VerificationWaitCommand,
    format: OutputFormat,
) -> Result<()> {
    match command {
        VerificationWaitCommand::Register {
            inbox,
            timeout_sec,
            from_hint,
            pattern,
            idempotency_key,
        } => {
            let body =
                build_verification_wait_request(timeout_sec, from_hint, pattern, idempotency_key);
            let response = client.register_verification_wait(&inbox, &body).await?;
            output::print_json(&response, format)
        }
        VerificationWaitCommand::List { inbox } => {
            let response = client.list_verification_waits(&inbox).await?;
            output::print_json(&response, format)
        }
        VerificationWaitCommand::Get { inbox, wait_id } => {
            let response = client.get_verification_wait(&inbox, &wait_id).await?;
            output::print_json(&response, format)
        }
        VerificationWaitCommand::Cancel { inbox, wait_id } => {
            let response = client.cancel_verification_wait(&inbox, &wait_id).await?;
            output::print_json(&response, format)
        }
    }
}

/// Dispatches the `templates` subcommands. Template bodies carry free-form
/// `source`/`variables`, so requests are assembled as `serde_json::Value` and
/// responses pass through `print_json` verbatim — matching the
/// outbound/audit-logs convention for the newer resource families.
async fn run_template(
    client: &ApiClient,
    command: TemplateCommand,
    format: OutputFormat,
) -> Result<()> {
    match command {
        TemplateCommand::List => {
            let response = client.list_templates().await?;
            output::print_json(&response, format)
        }
        TemplateCommand::Create {
            slug,
            name,
            description,
            source,
            source_file,
            subject,
            variables,
            notes,
        } => {
            let source = resolve_template_source(source, source_file)?;
            let mut body = json!({
                "slug": slug,
                "name": name,
                "source": source,
            });
            insert_opt_str(&mut body, "description", description);
            insert_opt_str(&mut body, "subject", subject);
            insert_opt_str(&mut body, "notes", notes);
            insert_opt_variables(&mut body, variables)?;
            let response = client.create_template(&body).await?;
            output::print_json(&response, format)
        }
        TemplateCommand::Get {
            id_or_slug,
            version,
        } => {
            let response = client.get_template(&id_or_slug, version).await?;
            output::print_json(&response, format)
        }
        TemplateCommand::Update {
            id_or_slug,
            name,
            description,
            current_version,
        } => {
            let mut body = json!({});
            insert_opt_str(&mut body, "name", name);
            // `description` is nullable: an explicit empty string clears it,
            // matching the SDK's `description: string | null` contract.
            if let Some(description) = description {
                body["description"] = serde_json::Value::String(description);
            }
            if let Some(current_version) = current_version {
                body["currentVersion"] = serde_json::Value::from(current_version);
            }
            let response = client.update_template(&id_or_slug, &body).await?;
            output::print_json(&response, format)
        }
        TemplateCommand::Delete { id_or_slug } => {
            let response = client.delete_template(&id_or_slug).await?;
            output::print_json(&response, format)
        }
        TemplateCommand::Versions { id_or_slug } => {
            let response = client.list_template_versions(&id_or_slug).await?;
            output::print_json(&response, format)
        }
        TemplateCommand::Version {
            id_or_slug,
            version,
        } => {
            let response = client.get_template_version(&id_or_slug, version).await?;
            output::print_json(&response, format)
        }
        TemplateCommand::Publish {
            id_or_slug,
            source,
            source_file,
            subject,
            variables,
            no_promote,
            notes,
        } => {
            let source = resolve_template_source(source, source_file)?;
            let mut body = json!({ "source": source });
            insert_opt_str(&mut body, "subject", subject);
            insert_opt_str(&mut body, "notes", notes);
            insert_opt_variables(&mut body, variables)?;
            // `promote` defaults to true server-side; only send it when opting out.
            if no_promote {
                body["promote"] = serde_json::Value::Bool(false);
            }
            let response = client.publish_template_version(&id_or_slug, &body).await?;
            output::print_json(&response, format)
        }
    }
}

/// Resolves a template/version source from either `--source` or `--source-file`.
/// Exactly one must be provided (the two flags are `conflicts_with` at the clap
/// layer, so this only needs to reject the both-absent case).
fn resolve_template_source(source: Option<String>, source_file: Option<PathBuf>) -> Result<String> {
    match (source, source_file) {
        (Some(source), _) => Ok(source),
        (None, Some(path)) => std::fs::read_to_string(&path)
            .with_context(|| format!("failed to read template source {}", path.display())),
        (None, None) => {
            anyhow::bail!("provide the template source with --source or --source-file")
        }
    }
}

/// Inserts a string field into a JSON object body only when present, mirroring
/// the SDKs' `skip_serializing_if`-style optional fields.
fn insert_opt_str(body: &mut serde_json::Value, key: &str, value: Option<String>) {
    if let Some(value) = value {
        body[key] = serde_json::Value::String(value);
    }
}

/// Parses `--variables` as a JSON object and inserts it under `variables`. The
/// backend's `variables_schema` is a JSON-Schema-lite object, so a non-object
/// (array/scalar) is rejected before the request goes out.
fn insert_opt_variables(body: &mut serde_json::Value, variables: Option<String>) -> Result<()> {
    if let Some(variables) = variables {
        let value: serde_json::Value =
            serde_json::from_str(&variables).context("--variables must be valid JSON")?;
        anyhow::ensure!(value.is_object(), "--variables must be a JSON object");
        body["variables"] = value;
    }
    Ok(())
}

fn build_inbox_schema_request(
    schema: Option<String>,
    schema_file: Option<PathBuf>,
    on_validation_error: Option<InboxSchemaValidationMode>,
    extraction_hint: Option<String>,
) -> Result<serde_json::Value> {
    let mut body = json!({});
    if let Some(schema) = resolve_json_object_arg("--schema", schema, schema_file)? {
        body["schema"] = schema;
    }
    if let Some(mode) = on_validation_error {
        body["onValidationError"] = serde_json::Value::String(
            match mode {
                InboxSchemaValidationMode::Quarantine => "quarantine",
                InboxSchemaValidationMode::Passthrough => "passthrough",
            }
            .to_string(),
        );
    }
    insert_opt_str(&mut body, "extractionHint", extraction_hint);
    Ok(body)
}

fn resolve_json_object_arg(
    flag_name: &str,
    inline: Option<String>,
    file: Option<PathBuf>,
) -> Result<Option<serde_json::Value>> {
    let Some(raw) = inline
        .map(Ok)
        .or_else(|| {
            file.map(|path| {
                std::fs::read_to_string(&path)
                    .with_context(|| format!("failed to read JSON object {}", path.display()))
            })
        })
        .transpose()?
    else {
        return Ok(None);
    };
    let value: serde_json::Value =
        serde_json::from_str(&raw).with_context(|| format!("{flag_name} must be valid JSON"))?;
    anyhow::ensure!(value.is_object(), "{flag_name} must be a JSON object");
    Ok(Some(value))
}

fn build_verification_wait_request(
    timeout_sec: u32,
    from_hint: Option<String>,
    pattern: Option<String>,
    idempotency_key: Option<String>,
) -> serde_json::Value {
    let mut body = json!({ "timeoutSec": timeout_sec });
    insert_opt_str(&mut body, "fromHint", from_hint);
    insert_opt_str(&mut body, "pattern", pattern);
    insert_opt_str(&mut body, "idempotencyKey", idempotency_key);
    body
}

/// Assembles the `POST /v1/events/replay` body. The backend requires exactly one
/// lower bound; this only sets the fields the caller supplied (the server
/// enforces the one-bound rule), matching the SDK request shape.
#[allow(clippy::too_many_arguments)]
fn build_replay_request(
    since: Option<String>,
    since_seq: Option<i64>,
    since_timestamp: Option<String>,
    inbox_id: Option<String>,
    until: Option<String>,
    types: Vec<String>,
    webhook_id: Option<String>,
    max_events: Option<u32>,
) -> serde_json::Value {
    let mut body = json!({});
    insert_opt_str(&mut body, "since", since);
    if let Some(since_seq) = since_seq {
        body["sinceSeq"] = serde_json::Value::from(since_seq);
    }
    insert_opt_str(&mut body, "sinceTimestamp", since_timestamp);
    insert_opt_str(&mut body, "inboxId", inbox_id);
    insert_opt_str(&mut body, "until", until);
    if !types.is_empty() {
        body["types"] = serde_json::Value::from(types);
    }
    insert_opt_str(&mut body, "webhookId", webhook_id);
    if let Some(max_events) = max_events {
        body["maxEvents"] = serde_json::Value::from(max_events);
    }
    body
}

/// Assembles the `PUT /v1/budgets` body. The server requires at least one
/// enforceable limit, so a `set` with no limit flags is rejected client-side
/// rather than sending an empty `limits` object the backend would refuse.
fn build_set_budget_request(
    scope: String,
    scope_id: Option<String>,
    disabled: bool,
    max_sends_per_day: Option<u64>,
    max_new_recipients_per_hour: Option<u64>,
    hard_stop_on_complaint: bool,
) -> Result<serde_json::Value> {
    let mut limits = serde_json::Map::new();
    if let Some(value) = max_sends_per_day {
        limits.insert("maxSendsPerDay".to_string(), serde_json::Value::from(value));
    }
    if let Some(value) = max_new_recipients_per_hour {
        limits.insert(
            "maxNewRecipientsPerHour".to_string(),
            serde_json::Value::from(value),
        );
    }
    if hard_stop_on_complaint {
        limits.insert(
            "hardStopOnComplaint".to_string(),
            serde_json::Value::Bool(true),
        );
    }
    anyhow::ensure!(
        !limits.is_empty(),
        "budgets set requires at least one limit (--max-sends-per-day, --max-new-recipients-per-hour, or --hard-stop-on-complaint)"
    );
    let mut body = json!({ "scope": scope, "limits": limits });
    insert_opt_str(&mut body, "scopeId", scope_id);
    // `enabled` defaults to true server-side; only send it when disabling.
    if disabled {
        body["enabled"] = serde_json::Value::Bool(false);
    }
    Ok(body)
}

/// Assembles the `POST /v1/erasure-jobs` body. The redesign merged the two
/// `/compliance/erase` + `/compliance/purge-inbox` verbs into one typed job
/// resource: provide exactly one of `subjectEmail` or `inboxId`. The CLI enforces
/// the exactly-one rule client-side so a malformed request never goes out.
fn build_erasure_job_request(
    subject_email: Option<String>,
    inbox_id: Option<String>,
) -> Result<serde_json::Value> {
    let subject_email = subject_email.and_then(non_empty_trimmed);
    let inbox_id = inbox_id.and_then(non_empty_trimmed);
    match (subject_email, inbox_id) {
        (Some(subject_email), None) => Ok(json!({ "subjectEmail": subject_email })),
        (None, Some(inbox_id)) => Ok(json!({ "inboxId": inbox_id })),
        (Some(_), Some(_)) => {
            anyhow::bail!("erasure-jobs create takes exactly one of --subject-email or --inbox-id")
        }
        (None, None) => {
            anyhow::bail!("erasure-jobs create requires either --subject-email or --inbox-id")
        }
    }
}

/// Trims a string and returns `None` if it is empty, so blank flag values
/// (e.g. `--send-at ""`) are treated as "not provided" rather than sent as an
/// empty string the backend would reject.
fn non_empty_trimmed(value: String) -> Option<String> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed.to_string())
    }
}

fn read_email_list_csv(path: &Path) -> Result<Vec<EmailListMemberInput>> {
    let content = std::fs::read_to_string(path)
        .with_context(|| format!("failed to read CSV {}", path.display()))?;
    let mut members = Vec::new();
    for (index, line) in content.lines().enumerate() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        let parts: Vec<&str> = trimmed.split(',').map(str::trim).collect();
        let email = parts.first().copied().unwrap_or_default().trim_matches('"');
        if index == 0 && email.eq_ignore_ascii_case("email") {
            continue;
        }
        anyhow::ensure!(!email.is_empty(), "CSV line {} has no email", index + 1);
        let name = parts
            .get(1)
            .map(|value| value.trim_matches('"').trim().to_string())
            .filter(|value| !value.is_empty());
        members.push(EmailListMemberInput {
            email: email.to_string(),
            name,
        });
    }
    anyhow::ensure!(!members.is_empty(), "CSV contains no recipients");
    Ok(members)
}

fn build_react_request(
    source: Option<String>,
    props_json: Option<String>,
) -> Result<Option<SendEmailReact>> {
    let Some(source) = source else {
        anyhow::ensure!(
            props_json.is_none(),
            "--react-props requires --react-source"
        );
        return Ok(None);
    };

    let props = match props_json {
        Some(props_json) => {
            let value: serde_json::Value =
                serde_json::from_str(&props_json).context("--react-props must be valid JSON")?;
            let object = value
                .as_object()
                .cloned()
                .context("--react-props must be a JSON object")?;
            Some(object)
        }
        None => None,
    };

    Ok(Some(SendEmailReact { source, props }))
}

fn read_send_attachments(
    paths: &[PathBuf],
    delivery: AttachmentDelivery,
    link_expiry_hours: Option<u32>,
) -> Result<Option<Vec<SendEmailAttachment>>> {
    if paths.is_empty() {
        return Ok(None);
    }
    if delivery == AttachmentDelivery::Link {
        anyhow::bail!(
            "delivery='link' for local --attachment files requires a standalone Dairo file upload/link API, which is not available in this CLI contract yet. Dairo will not attach files or edit the email body implicitly. To share an existing persisted email attachment, run `dairo attachments share <attachment-id> --expiry-hours {}` and place the printed URL deliberately in --text/--html, then send without --attachment",
            link_expiry_hours.unwrap_or(1)
        );
    }
    let mut attachments = Vec::with_capacity(paths.len());
    let mut total_bytes = 0usize;
    for path in paths {
        let bytes = std::fs::read(path)
            .with_context(|| format!("failed to read attachment {}", path.display()))?;
        anyhow::ensure!(!bytes.is_empty(), "attachment {} is empty", path.display());
        total_bytes += bytes.len();
        anyhow::ensure!(
            bytes.len() <= MAX_INLINE_ATTACHMENT_BYTES
                && total_bytes <= MAX_INLINE_ATTACHMENT_BYTES,
            "{}",
            oversized_attachment_message(delivery, link_expiry_hours)
        );
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
            delivery: None,
        });
    }
    Ok(Some(attachments))
}

fn oversized_attachment_message(
    delivery: AttachmentDelivery,
    link_expiry_hours: Option<u32>,
) -> String {
    let expiry_hours = link_expiry_hours.unwrap_or(1);
    let mode_context = match delivery {
        AttachmentDelivery::Attachment => {
            "file too big for email attachment delivery"
        }
        AttachmentDelivery::Auto => {
            "auto delivery would need link mode because the file is too big for inline attachment delivery"
        }
        AttachmentDelivery::Link => unreachable!("link mode is handled before reading files"),
    };
    format!(
        "{mode_context}; standalone local file-link upload is not available in this CLI contract yet. Dairo will not modify --text/--html automatically. To share an existing persisted email attachment, run `dairo attachments share <attachment-id> --expiry-hours {expiry_hours}` and place the printed URL deliberately in the email body, then send without the oversized --attachment. Dairo inline attachment limit is {MAX_INLINE_ATTACHMENT_BYTES} bytes to stay below API Gateway's 10 MB JSON/base64 envelope and SES v2's 40 MB message limit"
    )
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

fn verify_webhook_from_stdin(
    secret: &str,
    signature: &str,
    timestamp: &str,
    tolerance_seconds: u64,
    json_output: bool,
) -> Result<()> {
    use std::io::Read;

    let mut raw_body = Vec::new();
    std::io::stdin()
        .read_to_end(&mut raw_body)
        .context("failed to read webhook body from stdin")?;

    match webhook::verify_webhook(secret, &raw_body, signature, timestamp, tolerance_seconds) {
        Ok(()) => {
            if json_output {
                println!("{}", json!({ "verified": true }));
            } else {
                println!("Webhook signature is valid.");
            }
            Ok(())
        }
        // Surface a structured reason without ever echoing the secret or the
        // computed signature.
        Err(reason) => Err(anyhow::anyhow!("webhook verification failed: {reason}")),
    }
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extract_email_address_handles_bare_and_display_name_forms() {
        assert_eq!(extract_email_address("agent@dairo.app"), "agent@dairo.app");
        assert_eq!(
            extract_email_address("  Agent@Dairo.App "),
            "agent@dairo.app"
        );
        assert_eq!(
            extract_email_address("Support <support@dairo.app>"),
            "support@dairo.app"
        );
        assert_eq!(
            extract_email_address("Two Words < hi@dairo.app >"),
            "hi@dairo.app"
        );
        // Malformed angle brackets fall back to the trimmed, lowercased input.
        assert_eq!(extract_email_address(">weird<"), ">weird<");
    }

    #[test]
    fn normalize_optional_recipients_trims_and_drops_blanks() {
        let mut cc = vec![
            " a@x.com ".to_string(),
            "".to_string(),
            "b@x.com".to_string(),
        ];
        normalize_optional_recipients(&mut cc);
        assert_eq!(cc, vec!["a@x.com", "b@x.com"]);
        let mut empty: Vec<String> = vec!["  ".to_string()];
        normalize_optional_recipients(&mut empty);
        assert!(empty.is_empty());
    }

    #[test]
    fn resolve_body_source_rejects_both_inline_and_file() {
        let err = resolve_body_source(
            Some("hi".to_string()),
            Some(PathBuf::from("body.txt")),
            "text",
        )
        .unwrap_err();
        assert!(err.to_string().contains("--text or --text-file"));
        assert_eq!(
            resolve_body_source(Some("hi".to_string()), None, "text").unwrap(),
            Some("hi".to_string())
        );
        assert_eq!(resolve_body_source(None, None, "html").unwrap(), None);
    }

    #[test]
    fn builds_react_request_with_object_props() {
        let react = build_react_request(
            Some("export default function Email() { return <p>Hello</p>; }".to_string()),
            Some(r#"{"name":"Max"}"#.to_string()),
        )
        .unwrap()
        .unwrap();

        assert_eq!(
            react.source,
            "export default function Email() { return <p>Hello</p>; }"
        );
        assert_eq!(react.props.unwrap()["name"], "Max");
    }

    #[test]
    fn rejects_react_props_that_are_not_an_object() {
        let error = build_react_request(
            Some("export default function Email() { return <p>Hello</p>; }".to_string()),
            Some(r#"["Max"]"#.to_string()),
        )
        .expect_err("array props should be rejected");

        assert!(error.to_string().contains("JSON object"));
    }

    #[test]
    fn builds_inbox_schema_request_with_camel_case_fields() {
        let body = build_inbox_schema_request(
            Some(r#"{"code":{"type":"string","required":true}}"#.to_string()),
            None,
            Some(InboxSchemaValidationMode::Passthrough),
            Some("Find the OTP.".to_string()),
        )
        .unwrap();

        assert_eq!(body["schema"]["code"]["type"], "string");
        assert_eq!(body["schema"]["code"]["required"], true);
        assert_eq!(body["onValidationError"], "passthrough");
        assert_eq!(body["extractionHint"], "Find the OTP.");
    }

    #[test]
    fn rejects_non_object_inbox_schema() {
        let error = build_inbox_schema_request(Some(r#"["code"]"#.to_string()), None, None, None)
            .expect_err("array schema should be rejected");

        assert!(error.to_string().contains("--schema must be a JSON object"));
    }

    #[test]
    fn builds_verification_wait_request_with_camel_case_fields() {
        let body = build_verification_wait_request(
            120,
            Some("github.com".to_string()),
            Some(r#"code: ([0-9]{6})"#.to_string()),
            Some("wait-1".to_string()),
        );

        assert_eq!(body["timeoutSec"], 120);
        assert_eq!(body["fromHint"], "github.com");
        assert_eq!(body["pattern"], r#"code: ([0-9]{6})"#);
        assert_eq!(body["idempotencyKey"], "wait-1");
    }

    #[test]
    fn auto_delivery_keeps_small_attachments_inline() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("invoice.txt");
        std::fs::write(&path, b"hello").unwrap();

        let attachments =
            read_send_attachments(&[path], AttachmentDelivery::Auto, Some(24)).unwrap();
        let attachments = attachments.unwrap();

        assert_eq!(attachments.len(), 1);
        assert_eq!(attachments[0].filename, "invoice.txt");
        assert_eq!(attachments[0].delivery, None);
    }

    #[test]
    fn parse_key_value_pairs_builds_a_map_and_rejects_malformed() {
        let map = parse_key_value_pairs(
            &["X-Campaign=spring".to_string(), "env = prod".to_string()],
            "--headers",
        )
        .unwrap()
        .unwrap();
        assert_eq!(map.get("X-Campaign").map(String::as_str), Some("spring"));
        // Keys/values are trimmed.
        assert_eq!(map.get("env").map(String::as_str), Some("prod"));

        // No pairs -> omitted entirely.
        assert!(parse_key_value_pairs(&[], "--tags").unwrap().is_none());

        // Missing `=` is rejected.
        let err = parse_key_value_pairs(&["nope".to_string()], "--headers").unwrap_err();
        assert!(err.to_string().contains("KEY=VALUE"));

        // Empty key is rejected.
        let err = parse_key_value_pairs(&["=value".to_string()], "--tags").unwrap_err();
        assert!(err.to_string().contains("empty key"));
    }

    #[test]
    fn resolve_send_at_passes_through_rfc3339() {
        // RFC3339 with an offset is canonicalized but preserved.
        let out = resolve_send_at("2026-06-11T09:00:00Z").unwrap();
        let parsed = chrono::DateTime::parse_from_rfc3339(&out).unwrap();
        assert_eq!(
            parsed.with_timezone(&chrono::Utc),
            chrono::DateTime::parse_from_rfc3339("2026-06-11T09:00:00Z")
                .unwrap()
                .with_timezone(&chrono::Utc)
        );
    }

    #[test]
    fn resolve_send_at_parses_natural_language() {
        // "in 1 hour" should resolve to ~1h from now and be valid RFC3339.
        let out = resolve_send_at("in 1 hour").unwrap();
        let parsed = chrono::DateTime::parse_from_rfc3339(&out)
            .expect("natural language must resolve to RFC3339");
        let delta = parsed.with_timezone(&chrono::Utc) - chrono::Utc::now();
        assert!(
            delta.num_minutes() >= 55 && delta.num_minutes() <= 65,
            "expected ~1h ahead, got {} minutes",
            delta.num_minutes()
        );

        // Garbage is rejected with a helpful message.
        let err = resolve_send_at("definitely not a time").unwrap_err();
        assert!(err.to_string().contains("--send-at"));
    }

    #[test]
    fn decoded_base64_len_matches_actual_decoded_size() {
        // "JVBERi0xLjQ=" decodes to 8 bytes ("%PDF-1.4").
        assert_eq!(decoded_base64_len("JVBERi0xLjQ="), 8);
        assert_eq!(decoded_base64_len(""), 0);
        // No padding.
        assert_eq!(decoded_base64_len("aGVsbG8h"), 6); // "hello!"
    }

    #[test]
    fn dry_run_redacts_attachment_bytes() {
        let request = SendEmailRequest {
            inbox_id: "inbox_1".to_string(),
            to: vec!["max@example.com".to_string()],
            cc: None,
            bcc: None,
            subject: "Hi".to_string(),
            text: Some("Body".to_string()),
            html: None,
            react: None,
            attachments: Some(vec![SendEmailAttachment {
                filename: "invoice.pdf".to_string(),
                content_type: "application/pdf".to_string(),
                content_base64: "JVBERi0xLjQ=".to_string(),
                delivery: None,
            }]),
            idempotency_key: None,
            send_at: None,
            ignore_complaints: false,
            reply_to: Some("support@dairo.app".to_string()),
            headers: None,
            tags: None,
        };
        let mut value = serde_json::to_value(&request).unwrap();
        // Mirror the redaction the dry-run printer performs.
        if let Some(attachments) = value.get_mut("attachments").and_then(|v| v.as_array_mut()) {
            for attachment in attachments {
                if let Some(obj) = attachment.as_object_mut() {
                    let byte_length = obj
                        .remove("contentBase64")
                        .and_then(|v| v.as_str().map(decoded_base64_len))
                        .unwrap_or(0);
                    obj.insert(
                        "byteLength".to_string(),
                        serde_json::Value::from(byte_length),
                    );
                }
            }
        }
        assert!(value["attachments"][0].get("contentBase64").is_none());
        assert_eq!(value["attachments"][0]["byteLength"], 8);
        assert_eq!(value["attachments"][0]["filename"], "invoice.pdf");
        assert_eq!(value["replyTo"], "support@dairo.app");
    }

    #[test]
    fn link_delivery_reports_missing_standalone_upload_contract() {
        let path = PathBuf::from("invoice.pdf");
        let error = read_send_attachments(&[path], AttachmentDelivery::Link, Some(24))
            .expect_err("link delivery cannot pretend local file-link upload exists");

        let message = error.to_string();
        assert!(message.contains("standalone Dairo file upload/link API"));
        assert!(message.contains("dairo attachments share <attachment-id> --expiry-hours 24"));
    }
}
