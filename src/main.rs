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
    A2aMessageQuery, ApiClient, AudienceMemberInput, AudienceMembersRequest, AuditLogQuery,
    BucketObjectListQuery, CreateApiKeyRequest, CreateAudienceRequest, CreateBucketRequest,
    CreateDomainRequest, CreateInboxRequest, CreateLetterRequest, CreateWebhookRequest,
    EventsQuery, LetterCreditor, LetterDebtor, LetterFileRef, LetterListQuery, LetterPayment,
    LetterPriceRequest, LetterPrintOptions, MessageListQuery, PostalAddress, SendMessageAttachment,
    SendMessageReact, SendMessageRequest, ThreadListQuery, VerifyAgentQuery,
};
use base64::{engine::general_purpose::STANDARD as BASE64_STANDARD, Engine as _};
use clap::CommandFactory;
use clap::Parser;
use cli::{
    A2aCommand, AgentCommand, ApiKeyCommand, AttachmentCommand, AttachmentDelivery,
    AudienceCommand, AuditLogCommand, AuthCommand, BucketCommand, BudgetCommand, Cli, Command,
    ComplianceCommand, DedicatedIpCommand, DomainCommand, ErasureJobCommand, EventsCommand,
    InboxCommand, InboxSchemaCommand, InboxSchemaValidationMode, LetterCommand, LetterPaymentArgs,
    LetterPriceArgs, LetterPrintArgs, LetterSendArgs, LoginArgs, McpCommand, MessageCommand,
    OutboundCommand, RecipientArgs, ReputationCommand, SenderArgs, TemplateCommand, ThreadCommand,
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
        // `update` only talks to public release/download endpoints and never
        // needs a Dairo token.
        Command::Update(args) => update::run(args, OutputFormat::from_json_flag(cli.json)).await,
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
                        channel,
                        limit,
                        cursor,
                    } => {
                        let response = client
                            .list_messages(&MessageListQuery {
                                inbox_id,
                                thread_id,
                                direction,
                                channel,
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
                    MessageCommand::BatchDelete { message_ids } => {
                        let result = client.batch_delete_messages(message_ids).await?;
                        output::print_batch_delete_result("message", &result, format)
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
                        let response = client.send(&request).await?;
                        output::print_send_result(&response, format)
                    }
                }
                Command::Letter { command } => run_letter(&client, command, format).await,
                Command::Bucket { command } => run_bucket(&client, command, format).await,
                Command::Outbound { command } => match command {
                    OutboundCommand::List { limit } => {
                        let response = client.list_outbound_emails(limit).await?;
                        output::print_json(&response, format)
                    }
                    OutboundCommand::Get { message_id } => {
                        let response = client.get_outbound_email(&message_id).await?;
                        output::print_json(&response, format)
                    }
                    OutboundCommand::Cancel { message_id } => {
                        let response = client.cancel_outbound_email(&message_id).await?;
                        output::print_json(&response, format)
                    }
                    OutboundCommand::Events { message_id, limit } => {
                        let response = client.list_outbound_events(&message_id, limit).await?;
                        output::print_json(&response, format)
                    }
                    OutboundCommand::Bounces { message_id, limit } => {
                        let response = client.list_outbound_events(&message_id, limit).await?;
                        output::print_json(
                            &output::filter_events_of_type(response, "bounce"),
                            format,
                        )
                    }
                    OutboundCommand::Complaints { message_id, limit } => {
                        let response = client.list_outbound_events(&message_id, limit).await?;
                        output::print_json(
                            &output::filter_events_of_type(response, "complaint"),
                            format,
                        )
                    }
                },
                Command::Audience { command } => match command {
                    AudienceCommand::List => {
                        let response = client.list_audiences().await?;
                        output::print_audiences(&response.data, format)
                    }
                    AudienceCommand::Create { name, description } => {
                        let list = client
                            .create_audience(&CreateAudienceRequest { name, description })
                            .await?;
                        output::print_audiences(std::slice::from_ref(&list), format)
                    }
                    AudienceCommand::Get { list_id } => {
                        let response = client.get_audience(&list_id).await?;
                        output::print_audience_detail(&response, format)
                    }
                    AudienceCommand::Delete { list_id } => {
                        client.delete_audience(&list_id).await?;
                        output::print_deleted("email list", format)
                    }
                    AudienceCommand::Add {
                        list_id,
                        email,
                        name,
                    } => {
                        let response = client
                            .add_audience_members(
                                &list_id,
                                &AudienceMembersRequest {
                                    members: vec![AudienceMemberInput { email, name }],
                                },
                            )
                            .await?;
                        output::print_audience_import(&response, format)
                    }
                    AudienceCommand::ImportCsv { list_id, file } => {
                        let members = read_audience_csv(&file)?;
                        // The /members/import alias was removed in the redesign; the
                        // canonical /members endpoint upserts and accepts the same
                        // payload, so CSV import now posts there too.
                        let response = client
                            .add_audience_members(
                                &list_id,
                                &AudienceMembersRequest { members },
                            )
                            .await?;
                        output::print_audience_import(&response, format)
                    }
                    AudienceCommand::Send { list_id, send } => {
                        let dry_run = send.dry_run;
                        let request = build_send_request(&client, send, false).await?;
                        if dry_run {
                            print_dry_run_request(&request)
                        } else {
                            let response =
                                client.send_audience(&list_id, &request).await?;
                            output::print_audience_send(&response, format)
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
                Command::Update(_) => {
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

// ---------------------------------------------------------------------------
// Letters (physical-mail surface)
// ---------------------------------------------------------------------------

/// Max decoded PDF size the CLI will base64-encode and send inline, mirroring
/// the backend's `DAIRO_LETTER_MAX_PDF_BYTES` (default 25 MB). A larger file is
/// rejected client-side so the request never trips the backend's `413`.
const MAX_LETTER_PDF_BYTES: usize = 25 * 1024 * 1024;

/// Dispatches the `letter` subcommands. Like the outbound/templates families,
/// responses pass through `print_json` verbatim (the unified envelope). The two
/// POST mutations (send, cancel) ride the shared body-inclusive default
/// idempotency-key path in the API client automatically.
async fn run_letter(
    client: &ApiClient,
    command: LetterCommand,
    format: OutputFormat,
) -> Result<()> {
    match command {
        LetterCommand::Send(args) => {
            let dry_run = args.dry_run;
            let request = build_create_letter_request(&args)?;
            if dry_run {
                print_letter_dry_run_request(&request)
            } else {
                let response = client.create_letter(&request).await?;
                output::print_json(&response, format)
            }
        }
        LetterCommand::List {
            limit,
            cursor,
            status,
            country,
        } => {
            let query = LetterListQuery {
                limit,
                cursor: cursor.and_then(non_empty_trimmed),
                status: status.map(|s| s.as_str().to_string()),
                country: country.and_then(non_empty_trimmed),
            };
            let response = client.list_letters(&query).await?;
            output::print_json(&response, format)
        }
        LetterCommand::Get { id } => {
            let response = client.get_letter(id.trim()).await?;
            output::print_json(&response, format)
        }
        LetterCommand::Cancel { id } => {
            let response = client.cancel_letter(id.trim()).await?;
            output::print_json(&response, format)
        }
        LetterCommand::Events { id, limit, cursor } => {
            let cursor = cursor.and_then(non_empty_trimmed);
            let response = client
                .list_letter_events(id.trim(), limit, cursor.as_deref())
                .await?;
            output::print_json(&response, format)
        }
        LetterCommand::Price(args) => {
            let request = build_letter_price_request(&args)?;
            let response = client.price_letter(&request).await?;
            output::print_json(&response, format)
        }
    }
}

// ---------------------------------------------------------------------------
// Storage buckets (/v1/buckets)
// ---------------------------------------------------------------------------

/// Dispatches the `bucket` subcommands. Bucket/object CRUD responses pass
/// through `print_json` verbatim (the unified envelope); upload and download
/// drive the three-step presigned flow and the local-file IO in the API client.
async fn run_bucket(
    client: &ApiClient,
    command: BucketCommand,
    format: OutputFormat,
) -> Result<()> {
    match command {
        BucketCommand::Create {
            name,
            display_name,
            description,
        } => {
            let name = name.trim().to_string();
            anyhow::ensure!(!name.is_empty(), "bucket name must not be empty");
            let response = client
                .create_bucket(&CreateBucketRequest {
                    name,
                    display_name: display_name.and_then(non_empty_trimmed),
                    description: description.and_then(non_empty_trimmed),
                })
                .await?;
            output::print_json(&response, format)
        }
        BucketCommand::List => {
            let response = client.list_buckets().await?;
            output::print_json(&response, format)
        }
        BucketCommand::Get { bucket_id } => {
            let response = client.get_bucket(bucket_id.trim()).await?;
            output::print_json(&response, format)
        }
        BucketCommand::Delete { bucket_id } => {
            client.delete_bucket(bucket_id.trim()).await?;
            output::print_deleted("bucket", format)
        }
        BucketCommand::Ls {
            bucket_id,
            limit,
            cursor,
        } => {
            let response = client
                .list_bucket_objects(
                    bucket_id.trim(),
                    &BucketObjectListQuery {
                        limit,
                        cursor: cursor.and_then(non_empty_trimmed),
                    },
                )
                .await?;
            output::print_json(&response, format)
        }
        BucketCommand::Upload {
            bucket_id,
            file,
            name,
        } => {
            let bytes = std::fs::read(&file)
                .with_context(|| format!("failed to read {}", file.display()))?;
            anyhow::ensure!(!bytes.is_empty(), "{} is empty", file.display());
            let filename = name
                .and_then(non_empty_trimmed)
                .or_else(|| {
                    file.file_name()
                        .and_then(|value| value.to_str())
                        .map(str::to_string)
                })
                .context("could not determine an object name; pass --name")?;
            let content_type = mime_guess_from_path(&file);
            let response = client
                .upload_file(bucket_id.trim(), &filename, &content_type, bytes)
                .await?;
            output::print_json(&response, format)
        }
        BucketCommand::Download {
            bucket_id,
            object_id,
            out,
        } => {
            let bytes = client
                .download_file(bucket_id.trim(), object_id.trim())
                .await?;
            write_download(&out, &bytes)?;
            println!("Downloaded {} bytes to {}", bytes.len(), out.display());
            Ok(())
        }
        BucketCommand::Rm {
            bucket_id,
            object_id,
        } => {
            client
                .delete_bucket_object(bucket_id.trim(), object_id.trim())
                .await?;
            output::print_deleted("bucket object", format)
        }
        BucketCommand::BatchRm {
            bucket_id,
            object_ids,
        } => {
            let result = client
                .batch_delete_bucket_objects(bucket_id.trim(), object_ids)
                .await?;
            output::print_batch_delete_result("bucket object", &result, format)
        }
    }
}

/// Best-effort content-type guess from a path's extension, defaulting to
/// `application/octet-stream` so the bucket object always carries a valid MIME
/// type for the presigned PUT.
fn mime_guess_from_path(path: &Path) -> String {
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .map(str::to_ascii_lowercase)
        .unwrap_or_default();
    match ext.as_str() {
        "pdf" => "application/pdf",
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "svg" => "image/svg+xml",
        "txt" | "log" => "text/plain",
        "csv" => "text/csv",
        "json" => "application/json",
        "xml" => "application/xml",
        "html" | "htm" => "text/html",
        "zip" => "application/zip",
        "gz" => "application/gzip",
        "mp4" => "video/mp4",
        "mp3" => "audio/mpeg",
        "doc" => "application/msword",
        "docx" => "application/vnd.openxmlformats-officedocument.wordprocessingml.document",
        "xls" => "application/vnd.ms-excel",
        "xlsx" => "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet",
        _ => "application/octet-stream",
    }
    .to_string()
}

/// Assembles the `POST /v1/letters` body from the parsed `send` args. Exactly
/// one PDF source is guaranteed present by the clap `pdf_source` group; this
/// reads/encodes the inline PDF (or builds the attachment reference), resolves
/// the file name, validates the recipient address, and folds in the optional
/// sender, print options, delivery, and metadata. `confirm` toggles `autoSend`:
/// omitted (draft) unless the operator confirmed.
fn build_create_letter_request(args: &LetterSendArgs) -> Result<CreateLetterRequest> {
    let template_id = args.template_id.clone().and_then(non_empty_trimmed);
    let (pdf_base64, file, default_name) = match (&args.pdf, &args.attachment_id, &template_id) {
        (Some(path), _, _) => {
            let bytes = read_letter_pdf(path)?;
            let default_name = path
                .file_name()
                .and_then(|value| value.to_str())
                .filter(|value| !value.trim().is_empty())
                .map(str::to_string);
            (Some(BASE64_STANDARD.encode(bytes)), None, default_name)
        }
        (None, Some(attachment_id), _) => {
            let file = LetterFileRef {
                attachment_id: attachment_id.trim().to_string(),
                message_id: args.message_id.clone().and_then(non_empty_trimmed),
            };
            (None, Some(file), None)
        }
        // The Dairo-render path: the PDF is generated server-side from the
        // template, so no inline bytes or attachment ref accompany it.
        (None, None, Some(_)) => (None, None, None),
        // The clap `letter_source` group guarantees exactly one source is set.
        (None, None, None) => {
            anyhow::bail!(
                "provide a letter source: --pdf <PATH>, --attachment-id, or --template-id"
            )
        }
    };

    let to = build_recipient_address(&args.recipient)?;

    // The structured payment slip is generated by Dairo and composited onto a
    // template-rendered letter, so it is honored only on the --template-id path.
    // Reject it on a pdfBase64 (or attachment) letter with the same message the
    // backend returns, so the error surfaces locally before any request goes out.
    let payment = build_letter_payment(&args.payment, &to)?;
    if payment.is_some() {
        anyhow::ensure!(
            template_id.is_some(),
            "payment slips require a template; pass --template-id (a generated \
             payment slip is only supported on the Dairo-render path, not on a \
             --pdf / --attachment-id letter)"
        );
    }

    // A template-rendered letter needs no client-supplied file name; otherwise
    // it is derived from --pdf or required alongside --attachment-id.
    let file_name = if template_id.is_some() {
        args.file_name
            .clone()
            .and_then(non_empty_trimmed)
            .unwrap_or_else(|| "letter.pdf".to_string())
    } else {
        args.file_name
            .clone()
            .and_then(non_empty_trimmed)
            .or(default_name)
            .context(
                "a file name is required; pass --file-name when using --attachment-id \
                 (it is derived from the --pdf path otherwise)",
            )?
    };

    let from = build_sender_address(&args.sender)?;
    let print = build_letter_print_options(&args.print, args.print.address_placement.is_some());
    let delivery = args.delivery.map(|d| d.as_str().to_string());
    let metadata = match args.metadata.as_deref().map(str::trim) {
        Some(raw) if !raw.is_empty() => {
            let value: serde_json::Value =
                serde_json::from_str(raw).context("--metadata must be valid JSON")?;
            anyhow::ensure!(value.is_object(), "--metadata must be a JSON object");
            Some(value)
        }
        _ => None,
    };

    // The bare bring-your-own-slip flag (--payment-slip) and the structured slip
    // (--payment-type) are mutually exclusive at the CLI; when a structured slip
    // is present the provider flag is derived from its type so the backend picks
    // the right paper for the generated slip.
    let payment_slip = match &payment {
        Some(p) => Some(p.payment_type.clone()),
        None => args.payment_slip.map(|slip| slip.as_str().to_string()),
    };

    Ok(CreateLetterRequest {
        pdf_base64,
        file,
        file_name,
        to,
        from,
        template_id,
        print,
        delivery,
        payment_slip,
        payment,
        notifications: args.notifications,
        // Physical mail is irreversible: only auto-send when the operator
        // confirmed. `autoSend` defaults to true server-side, so a draft must
        // send `false` explicitly; a confirmed send omits the field.
        auto_send: if args.confirm { None } else { Some(false) },
        metadata,
    })
}

/// Builds the optional structured [`LetterPayment`] from the `--payment-*` flags.
/// Returns `None` when `--payment-type` is unset (no generated slip). When set,
/// the creditor block (name/IBAN/country) and amount are required, the amount is
/// validated (> 0, at most two decimals), the currency is defaulted from / checked
/// against the slip kind (`qr`=>CHF, `sepaDe`/`sepaAt`=>EUR), and the debtor
/// defaults to the letter's `to` address when no `--payment-debtor-*` flag is set.
fn build_letter_payment(
    args: &LetterPaymentArgs,
    to: &PostalAddress,
) -> Result<Option<LetterPayment>> {
    let Some(payment_type) = args.payment_type else {
        return Ok(None);
    };

    let amount = args
        .payment_amount
        .context("--payment-amount is required when --payment-type is set")?;
    anyhow::ensure!(
        amount.is_finite() && amount > 0.0,
        "--payment-amount must be a positive number"
    );
    // At most two decimal places: rounding to cents must not change the value.
    let cents = (amount * 100.0).round();
    anyhow::ensure!(
        (cents / 100.0 - amount).abs() < f64::EPSILON,
        "--payment-amount must have at most two decimal places"
    );

    // Default the currency to the one the slip kind requires; if the operator
    // set it explicitly, it must match (qr=>CHF, sepa*=>EUR).
    let required_currency = payment_type.required_currency();
    let currency = args.payment_currency.unwrap_or(required_currency);
    anyhow::ensure!(
        currency == required_currency,
        "--payment-type {} requires --payment-currency {}",
        payment_type.as_str(),
        required_currency.as_str()
    );

    let creditor = LetterCreditor {
        name: args
            .creditor_name
            .clone()
            .and_then(non_empty_trimmed)
            .context("--payment-creditor-name is required when --payment-type is set")?,
        iban: args
            .creditor_iban
            .clone()
            .and_then(non_empty_trimmed)
            .context("--payment-creditor-iban is required when --payment-type is set")?,
        bic: args.creditor_bic.clone().and_then(non_empty_trimmed),
        street: args.creditor_street.clone().and_then(non_empty_trimmed),
        house_number: args
            .creditor_house_number
            .clone()
            .and_then(non_empty_trimmed),
        postal_code: args
            .creditor_postal_code
            .clone()
            .and_then(non_empty_trimmed),
        city: args.creditor_city.clone().and_then(non_empty_trimmed),
        country: args
            .creditor_country
            .clone()
            .and_then(non_empty_trimmed)
            .context("--payment-creditor-country is required when --payment-type is set")?,
    };

    // The debtor defaults to the letter's `to` address; an explicit
    // --payment-debtor-* block overrides it.
    let debtor = if args.has_explicit_debtor() {
        let name = args
            .debtor_name
            .clone()
            .and_then(non_empty_trimmed)
            .context("--payment-debtor-name is required when a debtor block is given")?;
        let country = args
            .debtor_country
            .clone()
            .and_then(non_empty_trimmed)
            .context("--payment-debtor-country is required when a debtor block is given")?;
        LetterDebtor {
            name,
            street: args.debtor_street.clone().and_then(non_empty_trimmed),
            house_number: args.debtor_house_number.clone().and_then(non_empty_trimmed),
            postal_code: args.debtor_postal_code.clone().and_then(non_empty_trimmed),
            city: args.debtor_city.clone().and_then(non_empty_trimmed),
            country,
        }
    } else {
        debtor_from_recipient(to)?
    };

    Ok(Some(LetterPayment {
        payment_type: payment_type.as_str().to_string(),
        creditor,
        amount,
        currency: currency.as_str().to_string(),
        reference: args.payment_reference.clone().and_then(non_empty_trimmed),
        message: args.payment_message.clone().and_then(non_empty_trimmed),
        debtor: Some(debtor),
    }))
}

/// Derives the slip's debtor from the letter's `to` address (the default payer
/// when no explicit `--payment-debtor-*` block is given). The recipient needs a
/// name for the slip; if the `to` address has none, the operator must supply
/// `--payment-debtor-name` (or `--to-name`).
fn debtor_from_recipient(to: &PostalAddress) -> Result<LetterDebtor> {
    let name = to.name.clone().context(
        "the payment-slip debtor defaults to the letter's recipient, but it has no \
         name; pass --to-name or set the debtor explicitly with --payment-debtor-name",
    )?;
    Ok(LetterDebtor {
        name,
        street: to.street.clone(),
        house_number: to.house_number.clone(),
        postal_code: to.postal_code.clone(),
        city: to.city.clone(),
        country: to.country.clone(),
    })
}

/// Assembles the `POST /v1/letters/price` body from the parsed `price` args.
/// Reads/encodes the inline PDF when `--pdf` is given (exact page count); the
/// clap `price_pages` group already guarantees `--page-count`/`--pdf` are not
/// both set.
fn build_letter_price_request(args: &LetterPriceArgs) -> Result<LetterPriceRequest> {
    let country = args.country.trim();
    anyhow::ensure!(!country.is_empty(), "price requires --country");
    let pdf_base64 = match &args.pdf {
        Some(path) => Some(BASE64_STANDARD.encode(read_letter_pdf(path)?)),
        None => None,
    };
    let print = build_letter_print_options(&args.print, args.print.address_placement.is_some());
    let paper_types = if args.paper_types.is_empty() {
        None
    } else {
        Some(
            args.paper_types
                .iter()
                .map(|paper| paper.as_str().to_string())
                .collect(),
        )
    };
    Ok(LetterPriceRequest {
        country: country.to_string(),
        page_count: args.page_count,
        pdf_base64,
        print,
        delivery: args.delivery.map(|d| d.as_str().to_string()),
        paper_types,
    })
}

/// Builds the recipient [`PostalAddress`] from the `--to-*` flags, enforcing the
/// contract's "either street or PO box is required" rule client-side so a
/// malformed request never goes out. `country` is guaranteed present by clap.
fn build_recipient_address(args: &RecipientArgs) -> Result<PostalAddress> {
    let street = args.to_street.clone().and_then(non_empty_trimmed);
    let po_box = args.to_po_box.clone().and_then(non_empty_trimmed);
    anyhow::ensure!(
        street.is_some() || po_box.is_some(),
        "recipient address requires --to-street or --to-po-box"
    );
    let country = args.to_country.trim();
    anyhow::ensure!(
        !country.is_empty(),
        "recipient address requires --to-country"
    );
    Ok(PostalAddress {
        name: args.to_name.clone().and_then(non_empty_trimmed),
        company: args.to_company.clone().and_then(non_empty_trimmed),
        street,
        house_number: args.to_house_number.clone().and_then(non_empty_trimmed),
        po_box,
        address_line2: args.to_address_line2.clone().and_then(non_empty_trimmed),
        postal_code: args.to_postal_code.clone().and_then(non_empty_trimmed),
        city: args.to_city.clone().and_then(non_empty_trimmed),
        country: country.to_string(),
    })
}

/// Builds the optional sender [`PostalAddress`] from the `--from-*` flags.
/// Returns `None` when no sender field was set, so the `from` block is omitted
/// from the request entirely. When any field is set, `--from-country` is
/// required (the contract makes `country` mandatory on a `PostalAddress`).
fn build_sender_address(args: &SenderArgs) -> Result<Option<PostalAddress>> {
    let name = args.from_name.clone().and_then(non_empty_trimmed);
    let company = args.from_company.clone().and_then(non_empty_trimmed);
    let street = args.from_street.clone().and_then(non_empty_trimmed);
    let house_number = args.from_house_number.clone().and_then(non_empty_trimmed);
    let po_box = args.from_po_box.clone().and_then(non_empty_trimmed);
    let address_line2 = args.from_address_line2.clone().and_then(non_empty_trimmed);
    let postal_code = args.from_postal_code.clone().and_then(non_empty_trimmed);
    let city = args.from_city.clone().and_then(non_empty_trimmed);
    let country = args.from_country.clone().and_then(non_empty_trimmed);

    let any_set = name.is_some()
        || company.is_some()
        || street.is_some()
        || house_number.is_some()
        || po_box.is_some()
        || address_line2.is_some()
        || postal_code.is_some()
        || city.is_some()
        || country.is_some();
    if !any_set {
        return Ok(None);
    }
    let country = country.context(
        "a sender address was provided, so --from-country (ISO 3166-1 alpha-2) is required",
    )?;
    Ok(Some(PostalAddress {
        name,
        company,
        street,
        house_number,
        po_box,
        address_line2,
        postal_code,
        city,
        country,
    }))
}

/// Builds the optional [`LetterPrintOptions`] from the resolved print flags.
/// Returns `None` when no print option was set so the field is omitted entirely
/// and the backend applies its defaults. `has_placement` is the caller's check
/// for `--address-placement` so the helper does not re-read the args.
fn build_letter_print_options(
    args: &LetterPrintArgs,
    has_placement: bool,
) -> Option<LetterPrintOptions> {
    let options = LetterPrintOptions {
        mode: args.mode().map(|m| m.as_str().to_string()),
        sides: args.sides().map(|s| s.as_str().to_string()),
        address_placement: has_placement
            .then(|| args.address_placement.map(|p| p.as_str().to_string()))
            .flatten(),
    };
    if options.is_empty() {
        None
    } else {
        Some(options)
    }
}

/// Reads a letter PDF off disk, enforcing the size cap and a `%PDF-` magic-byte
/// check client-side so an obviously-wrong file fails fast before any encode.
fn read_letter_pdf(path: &Path) -> Result<Vec<u8>> {
    let bytes =
        std::fs::read(path).with_context(|| format!("failed to read PDF {}", path.display()))?;
    anyhow::ensure!(!bytes.is_empty(), "PDF {} is empty", path.display());
    anyhow::ensure!(
        bytes.len() <= MAX_LETTER_PDF_BYTES,
        "PDF {} is {} bytes, over the {}-byte limit for inline letter delivery",
        path.display(),
        bytes.len(),
        MAX_LETTER_PDF_BYTES
    );
    anyhow::ensure!(
        bytes.starts_with(b"%PDF-"),
        "{} does not look like a PDF (missing %PDF- header)",
        path.display()
    );
    Ok(bytes)
}

/// Renders a built [`CreateLetterRequest`] as pretty JSON for `--dry-run`,
/// without ever emitting the PDF bytes: the `pdfBase64` field is replaced by its
/// decoded `byteLength` so the operator sees the request shape without dumping
/// base64 to the terminal. Nothing is sent to the API.
fn print_letter_dry_run_request(request: &CreateLetterRequest) -> Result<()> {
    let mut value = serde_json::to_value(request).context("failed to serialize letter request")?;
    if let Some(obj) = value.as_object_mut() {
        if let Some(byte_length) = obj
            .remove("pdfBase64")
            .and_then(|v| v.as_str().map(decoded_base64_len))
        {
            obj.insert(
                "pdfByteLength".to_string(),
                serde_json::Value::from(byte_length),
            );
        }
    }
    println!("{}", serde_json::to_string_pretty(&value)?);
    Ok(())
}

const MAX_INLINE_ATTACHMENT_BYTES: usize = 8 * 1024 * 1024;

async fn build_send_request(
    client: &ApiClient,
    mut args: cli::SendArgs,
    require_to: bool,
) -> Result<SendMessageRequest> {
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
    Ok(SendMessageRequest {
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
        channel: args.channel.and_then(non_empty_trimmed),
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

/// Renders a built [`SendMessageRequest`] as pretty JSON for `--dry-run`, without
/// ever emitting attachment bytes: each attachment's `contentBase64` is replaced
/// by a `byteLength` (the decoded size) so the operator sees what would be sent
/// without dumping base64 to the terminal. Nothing is sent to the API.
fn print_dry_run_request(request: &SendMessageRequest) -> Result<()> {
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

fn read_audience_csv(path: &Path) -> Result<Vec<AudienceMemberInput>> {
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
        members.push(AudienceMemberInput {
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
) -> Result<Option<SendMessageReact>> {
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

    Ok(Some(SendMessageReact { source, props }))
}

fn read_send_attachments(
    paths: &[PathBuf],
    delivery: AttachmentDelivery,
    link_expiry_hours: Option<u32>,
) -> Result<Option<Vec<SendMessageAttachment>>> {
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
        attachments.push(SendMessageAttachment {
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
        let request = SendMessageRequest {
            inbox_id: "inbox_1".to_string(),
            to: vec!["max@example.com".to_string()],
            cc: None,
            bcc: None,
            subject: "Hi".to_string(),
            text: Some("Body".to_string()),
            html: None,
            react: None,
            attachments: Some(vec![SendMessageAttachment {
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
            channel: None,
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

    /// Parses a `letter send` command line into its [`LetterSendArgs`] for the
    /// payment-slip build tests below.
    fn letter_send_args(extra: &[&str]) -> cli::LetterSendArgs {
        let mut argv = vec!["dairo", "letter", "send"];
        argv.extend_from_slice(extra);
        match cli::Cli::parse_from(argv).command {
            cli::Command::Letter {
                command: cli::LetterCommand::Send(args),
            } => args,
            _ => panic!("expected letter send command"),
        }
    }

    #[test]
    fn build_letter_payment_renders_structured_slip_and_sets_provider_flag() {
        let args = letter_send_args(&[
            "--template-id",
            "tmpl_invoice",
            "--to-name",
            "Jane Doe",
            "--to-street",
            "Hauptstrasse",
            "--to-house-number",
            "12",
            "--to-postal-code",
            "8001",
            "--to-city",
            "Zürich",
            "--to-country",
            "CH",
            "--payment-type",
            "qr",
            "--payment-amount",
            "49.90",
            "--payment-creditor-name",
            "Acme AG",
            "--payment-creditor-iban",
            "CH9300762011623852957",
            "--payment-creditor-country",
            "CH",
            "--payment-reference",
            "210000000003139471430009017",
            "--payment-message",
            "Invoice inv_123",
        ]);
        let request = build_create_letter_request(&args).unwrap();

        // The Dairo-render path carries the template, no inline bytes.
        assert_eq!(request.template_id.as_deref(), Some("tmpl_invoice"));
        assert!(request.pdf_base64.is_none());
        assert!(request.file.is_none());

        let payment = request.payment.expect("payment object is present");
        assert_eq!(payment.payment_type, "qr");
        // The currency defaults from the slip kind (qr => CHF).
        assert_eq!(payment.currency, "CHF");
        assert_eq!(payment.amount, 49.90);
        assert_eq!(payment.creditor.name, "Acme AG");
        assert_eq!(payment.creditor.iban, "CH9300762011623852957");
        assert_eq!(payment.creditor.country, "CH");
        assert_eq!(
            payment.reference.as_deref(),
            Some("210000000003139471430009017")
        );
        assert_eq!(payment.message.as_deref(), Some("Invoice inv_123"));
        // The debtor defaults to the letter's `to` address.
        let debtor = payment.debtor.expect("debtor defaults to the recipient");
        assert_eq!(debtor.name, "Jane Doe");
        assert_eq!(debtor.street.as_deref(), Some("Hauptstrasse"));
        assert_eq!(debtor.postal_code.as_deref(), Some("8001"));
        assert_eq!(debtor.country, "CH");

        // The structured slip also drives the provider paymentSlip flag.
        assert_eq!(request.payment_slip.as_deref(), Some("qr"));
    }

    #[test]
    fn build_letter_payment_uses_explicit_debtor_and_defaults_sepa_currency() {
        let args = letter_send_args(&[
            "--template-id",
            "tmpl_x",
            "--to-name",
            "Recipient",
            "--to-street",
            "S",
            "--to-country",
            "CH",
            "--payment-type",
            "sepaAt",
            "--payment-amount",
            "12.5",
            "--payment-creditor-name",
            "Verein",
            "--payment-creditor-iban",
            "AT611904300234573201",
            "--payment-creditor-country",
            "AT",
            "--payment-debtor-name",
            "Explicit Payer",
            "--payment-debtor-city",
            "Wien",
            "--payment-debtor-country",
            "AT",
        ]);
        let payment = build_create_letter_request(&args)
            .unwrap()
            .payment
            .expect("payment object is present");
        // sepaAt defaults to EUR.
        assert_eq!(payment.currency, "EUR");
        let debtor = payment.debtor.unwrap();
        assert_eq!(debtor.name, "Explicit Payer");
        assert_eq!(debtor.city.as_deref(), Some("Wien"));
        assert_eq!(debtor.country, "AT");
    }

    #[test]
    fn build_letter_payment_rejects_pdf_letter_without_template() {
        let dir = std::env::temp_dir();
        let pdf = dir.join("dairo-payment-test.pdf");
        std::fs::write(&pdf, b"%PDF-1.4\n").unwrap();
        let pdf_str = pdf.to_str().unwrap();
        let args = letter_send_args(&[
            "--pdf",
            pdf_str,
            "--to-name",
            "Jane",
            "--to-street",
            "S",
            "--to-country",
            "DE",
            "--payment-type",
            "sepaDe",
            "--payment-amount",
            "10.00",
            "--payment-creditor-name",
            "Acme",
            "--payment-creditor-iban",
            "DE89370400440532013000",
            "--payment-creditor-country",
            "DE",
        ]);
        let error = build_create_letter_request(&args)
            .expect_err("a pdf letter plus a generated slip must be rejected");
        assert!(error
            .to_string()
            .contains("payment slips require a template"));
        let _ = std::fs::remove_file(&pdf);
    }

    #[test]
    fn build_letter_payment_rejects_currency_mismatch_and_bad_amount() {
        // qr requires CHF; an explicit EUR is rejected.
        let args = letter_send_args(&[
            "--template-id",
            "t",
            "--to-name",
            "Jane",
            "--to-street",
            "S",
            "--to-country",
            "CH",
            "--payment-type",
            "qr",
            "--payment-amount",
            "5.00",
            "--payment-currency",
            "EUR",
            "--payment-creditor-name",
            "Acme",
            "--payment-creditor-iban",
            "CH93",
            "--payment-creditor-country",
            "CH",
        ]);
        let error = build_create_letter_request(&args).expect_err("qr + EUR must be rejected");
        assert!(error
            .to_string()
            .contains("requires --payment-currency CHF"));

        // More than two decimal places is rejected.
        let args = letter_send_args(&[
            "--template-id",
            "t",
            "--to-name",
            "Jane",
            "--to-street",
            "S",
            "--to-country",
            "CH",
            "--payment-type",
            "qr",
            "--payment-amount",
            "5.001",
            "--payment-creditor-name",
            "Acme",
            "--payment-creditor-iban",
            "CH93",
            "--payment-creditor-country",
            "CH",
        ]);
        let error =
            build_create_letter_request(&args).expect_err("3-decimal amount must be rejected");
        assert!(error.to_string().contains("at most two decimal places"));

        // A non-positive amount is rejected.
        let args = letter_send_args(&[
            "--template-id",
            "t",
            "--to-name",
            "Jane",
            "--to-street",
            "S",
            "--to-country",
            "CH",
            "--payment-type",
            "qr",
            "--payment-amount",
            "0",
            "--payment-creditor-name",
            "Acme",
            "--payment-creditor-iban",
            "CH93",
            "--payment-creditor-country",
            "CH",
        ]);
        let error = build_create_letter_request(&args).expect_err("zero amount must be rejected");
        assert!(error.to_string().contains("positive number"));
    }

    #[test]
    fn build_letter_without_payment_keeps_bare_slip_flag() {
        let dir = std::env::temp_dir();
        let pdf = dir.join("dairo-byo-slip-test.pdf");
        std::fs::write(&pdf, b"%PDF-1.4\n").unwrap();
        let pdf_str = pdf.to_str().unwrap();
        let args = letter_send_args(&[
            "--pdf",
            pdf_str,
            "--to-name",
            "Jane",
            "--to-street",
            "S",
            "--to-country",
            "CH",
            "--payment-slip",
            "sepaDe",
        ]);
        let request = build_create_letter_request(&args).unwrap();
        // The bring-your-own-slip flag survives; no structured payment is built.
        assert_eq!(request.payment_slip.as_deref(), Some("sepaDe"));
        assert!(request.payment.is_none());
        assert!(request.template_id.is_none());
        let _ = std::fs::remove_file(&pdf);
    }
}
