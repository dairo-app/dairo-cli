use reqwest::{Method, Request, StatusCode};
use serde::{Deserialize, Serialize};
use std::time::Duration;
use thiserror::Error;
use url::Url;
use uuid::Uuid;

mod models;
pub use models::*;

pub const DEFAULT_BASE_URL: &str = "https://api.dairo.app";

/// Wall-clock timeout applied to every request so a hung connection cannot
/// block the CLI forever.
const REQUEST_TIMEOUT: Duration = Duration::from_secs(30);

/// Number of *additional* attempts after the first one for transient
/// server-side failures (429/502). Total attempts = `MAX_RETRIES + 1`.
const MAX_RETRIES: u32 = 3;

/// Base backoff between retries. Doubled each attempt (capped).
const RETRY_BASE_BACKOFF: Duration = Duration::from_millis(250);
const RETRY_MAX_BACKOFF: Duration = Duration::from_secs(5);

/// User-Agent advertised to the API. Never include the API key here.
const USER_AGENT: &str = concat!("dairo-cli/", env!("CARGO_PKG_VERSION"));

/// Size threshold at/under which `upload_file` keeps the single-PUT branded
/// flow; strictly above it switches to the resumable multipart flow. 64MiB
/// comfortably fits one PUT while keeping the multipart win for big objects.
pub const MULTIPART_THRESHOLD_BYTES: u64 = 64 * 1024 * 1024;

/// Wall-clock timeout for one part PUT. A multipart part can be up to 5GiB, so
/// the 30s API timeout is far too short — a part needs its own generous budget.
const PART_UPLOAD_TIMEOUT: Duration = Duration::from_secs(60 * 60);

/// Max number of part PUTs in flight at once. Bounded so a huge object doesn't
/// open thousands of sockets, while still parallelising for throughput.
const MULTIPART_PARALLELISM: usize = 4;

/// Additional attempts after the first for a single part PUT (transient S3
/// failures). This per-part retry is the resumability/robustness win.
const PART_MAX_RETRIES: u32 = 3;

#[derive(Debug, Error)]
pub enum ApiError {
    #[error("invalid API base URL: {0}")]
    InvalidBaseUrl(#[from] url::ParseError),
    #[error(
        "refusing to send the API key over an insecure URL: {0} is not HTTPS. \
         Use an https:// base URL, or http://localhost for local development."
    )]
    InsecureBaseUrl(String),
    #[error("failed to build API request: {0}")]
    BuildRequest(#[source] reqwest::Error),
    #[error("request failed: {0}")]
    Transport(#[from] reqwest::Error),
    #[error("Dairo API returned {status}: {message}")]
    Api { status: StatusCode, message: String },
}

pub type Result<T> = std::result::Result<T, ApiError>;

#[derive(Clone)]
pub struct ApiClient {
    base_url: Url,
    api_key: String,
    http: reqwest::Client,
}

// Hand-written `Debug` so the bearer API key can never leak through `{:?}`,
// error chains, panics, or telemetry that derives Debug.
impl std::fmt::Debug for ApiClient {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ApiClient")
            .field("base_url", &self.base_url.as_str())
            .field("api_key", &"[REDACTED]")
            .finish()
    }
}

impl ApiClient {
    pub fn new(base_url: impl AsRef<str>, api_key: impl Into<String>) -> Result<Self> {
        let base_url = Url::parse(base_url.as_ref())?;
        require_secure_base_url(&base_url)?;
        let http = reqwest::Client::builder()
            .user_agent(USER_AGENT)
            .timeout(REQUEST_TIMEOUT)
            .build()
            .map_err(ApiError::BuildRequest)?;
        Ok(Self {
            base_url,
            api_key: api_key.into(),
            http,
        })
    }

    pub async fn whoami(&self) -> Result<WhoamiResponse> {
        self.execute_json(self.build_request(Method::GET, &["v1", "whoami"], None::<&()>)?)
            .await
    }

    /// Lists domains (`GET /v1/domains`, scope `domains:read`). The live API
    /// returns the unified list envelope (`{ object:"list", data:[...] }`); the
    /// CLI reads `.data`.
    pub async fn list_domains(&self) -> Result<ListEnvelope<Domain>> {
        self.execute_json(self.build_request(Method::GET, &["v1", "domains"], None::<&()>)?)
            .await
    }

    /// Adds a domain and returns the single created domain object
    /// (`POST /v1/domains`, scope `domains:write`). The redesign returns the
    /// created domain, never the whole collection.
    pub async fn create_domain(&self, body: &CreateDomainRequest) -> Result<Domain> {
        self.execute_json(self.build_request(Method::POST, &["v1", "domains"], Some(body))?)
            .await
    }

    /// Removes a domain (`DELETE /v1/domains/{domain}`, scope `domains:write`).
    /// Returns 204 No Content (no body).
    pub async fn delete_domain(&self, domain: &str) -> Result<()> {
        self.execute_no_content(self.build_request(
            Method::DELETE,
            &["v1", "domains", domain],
            None::<&()>,
        )?)
        .await
    }

    /// Re-runs DNS verification for a domain (`POST /v1/domains/{domain}/verify`,
    /// scope `domains:write`; was `.../recheck`). Returns the single domain object.
    pub async fn recheck_domain(&self, domain: &str) -> Result<Domain> {
        self.execute_json(self.build_request(
            Method::POST,
            &["v1", "domains", domain, "verify"],
            None::<&()>,
        )?)
        .await
    }

    /// Lists inboxes (`GET /v1/inboxes`, scope `inboxes:read`).
    pub async fn list_inboxes(&self) -> Result<ListEnvelope<Inbox>> {
        self.execute_json(self.build_request(Method::GET, &["v1", "inboxes"], None::<&()>)?)
            .await
    }

    /// Creates an inbox (`POST /v1/inboxes`, scope `inboxes:write`). Returns the
    /// single inbox object.
    pub async fn create_inbox(&self, body: &CreateInboxRequest) -> Result<Inbox> {
        self.execute_json(self.build_request(Method::POST, &["v1", "inboxes"], Some(body))?)
            .await
    }

    /// Deletes an inbox (`DELETE /v1/inboxes/{inbox}`, scope `inboxes:write`).
    /// Returns 204 No Content.
    pub async fn delete_inbox(&self, inbox: &str) -> Result<()> {
        self.execute_no_content(self.build_request(
            Method::DELETE,
            &["v1", "inboxes", inbox],
            None::<&()>,
        )?)
        .await
    }

    /// Gets an inbox extraction schema (`GET /v1/inboxes/{inbox}/schema`, scope
    /// `inboxes:read`). The path segment accepts the inbox uuid or address.
    pub async fn get_inbox_schema(&self, inbox: &str) -> Result<serde_json::Value> {
        self.execute_json(self.build_request(
            Method::GET,
            &["v1", "inboxes", inbox, "schema"],
            None::<&()>,
        )?)
        .await
    }

    /// Attaches/replaces an inbox extraction schema
    /// (`PUT /v1/inboxes/{inbox}/schema`, scope `inboxes:write`).
    pub async fn set_inbox_schema(
        &self,
        inbox: &str,
        body: &serde_json::Value,
    ) -> Result<serde_json::Value> {
        self.execute_json(self.build_request(
            Method::PUT,
            &["v1", "inboxes", inbox, "schema"],
            Some(body),
        )?)
        .await
    }

    /// Deletes an inbox extraction schema
    /// (`DELETE /v1/inboxes/{inbox}/schema`, scope `inboxes:write`).
    pub async fn delete_inbox_schema(&self, inbox: &str) -> Result<()> {
        self.execute_no_content(self.build_request(
            Method::DELETE,
            &["v1", "inboxes", inbox, "schema"],
            None::<&()>,
        )?)
        .await
    }

    /// Registers a verification-code wait
    /// (`POST /v1/inboxes/{inbox}/verification-waits`, scope `inboxes:write`).
    pub async fn register_verification_wait(
        &self,
        inbox: &str,
        body: &serde_json::Value,
    ) -> Result<serde_json::Value> {
        self.execute_json(self.build_request(
            Method::POST,
            &["v1", "inboxes", inbox, "verification-waits"],
            Some(body),
        )?)
        .await
    }

    /// Lists verification-code waits for an inbox
    /// (`GET /v1/inboxes/{inbox}/verification-waits`, scope `inboxes:read`).
    pub async fn list_verification_waits(&self, inbox: &str) -> Result<serde_json::Value> {
        self.execute_json(self.build_request(
            Method::GET,
            &["v1", "inboxes", inbox, "verification-waits"],
            None::<&()>,
        )?)
        .await
    }

    /// Gets one verification-code wait
    /// (`GET /v1/inboxes/{inbox}/verification-waits/{waitId}`, scope
    /// `inboxes:read`).
    pub async fn get_verification_wait(
        &self,
        inbox: &str,
        wait_id: &str,
    ) -> Result<serde_json::Value> {
        self.execute_json(self.build_request(
            Method::GET,
            &["v1", "inboxes", inbox, "verification-waits", wait_id],
            None::<&()>,
        )?)
        .await
    }

    /// Cancels one verification-code wait
    /// (`DELETE /v1/inboxes/{inbox}/verification-waits/{waitId}`, scope
    /// `inboxes:write`). The backend returns the canceled wait object.
    pub async fn cancel_verification_wait(
        &self,
        inbox: &str,
        wait_id: &str,
    ) -> Result<serde_json::Value> {
        self.execute_json(self.build_request(
            Method::DELETE,
            &["v1", "inboxes", inbox, "verification-waits", wait_id],
            None::<&()>,
        )?)
        .await
    }

    /// Sends (or schedules) an outbound email (`POST /v1/emails`, scope
    /// `mail:send`; was `/v1/send-email`). The response is the single `email`
    /// object envelope.
    pub async fn send_email(&self, body: &SendEmailRequest) -> Result<SendEmailResponse> {
        self.execute_json(self.build_request(Method::POST, &["v1", "emails"], Some(body))?)
            .await
    }

    /// Lists active lists (`GET /v1/lists`, scope `lists:read`; was
    /// `/v1/email-lists`).
    pub async fn list_email_lists(&self) -> Result<ListEnvelope<EmailList>> {
        self.execute_json(self.build_request(Method::GET, &["v1", "lists"], None::<&()>)?)
            .await
    }

    /// Creates a list (`POST /v1/lists`, scope `lists:write`). Returns the single
    /// list object.
    pub async fn create_email_list(&self, body: &CreateEmailListRequest) -> Result<EmailList> {
        self.execute_json(self.build_request(Method::POST, &["v1", "lists"], Some(body))?)
            .await
    }

    /// Gets a list plus its active members (`GET /v1/lists/{id}`, scope
    /// `lists:read`). The members are carried as a field on the single list object.
    pub async fn get_email_list(&self, list_id: &str) -> Result<EmailListDetailResponse> {
        self.execute_json(self.build_request(
            Method::GET,
            &["v1", "lists", list_id],
            None::<&()>,
        )?)
        .await
    }

    /// Archives a list (`DELETE /v1/lists/{id}`, scope `lists:write`). Returns 204.
    pub async fn delete_email_list(&self, list_id: &str) -> Result<()> {
        self.execute_no_content(self.build_request(
            Method::DELETE,
            &["v1", "lists", list_id],
            None::<&()>,
        )?)
        .await
    }

    /// Upserts members via the canonical `POST /v1/lists/{id}/members` endpoint
    /// (<= 2000 members). The `/members/import` alias was removed in the redesign;
    /// both the manual add and CSV import now post here.
    pub async fn add_email_list_members(
        &self,
        list_id: &str,
        body: &EmailListMembersRequest,
    ) -> Result<EmailListImportResponse> {
        self.execute_json(self.build_request(
            Method::POST,
            &["v1", "lists", list_id, "members"],
            Some(body),
        )?)
        .await
    }

    pub async fn send_email_list(
        &self,
        list_id: &str,
        body: &SendEmailRequest,
    ) -> Result<EmailListSendResponse> {
        self.execute_json(self.build_request(
            Method::POST,
            &["v1", "lists", list_id, "send"],
            Some(body),
        )?)
        .await
    }

    /// Lists webhook subscriptions (`GET /v1/webhooks`, scope `webhooks:read`).
    pub async fn list_webhooks(&self) -> Result<ListEnvelope<Webhook>> {
        self.execute_json(self.build_request(Method::GET, &["v1", "webhooks"], None::<&()>)?)
            .await
    }

    /// Creates a webhook and returns its one-time signing secret as a field on
    /// the created object (`POST /v1/webhooks`, scope `webhooks:write`). The
    /// redesign puts `signingSecret` on the object, not a sibling top-level key.
    pub async fn create_webhook(
        &self,
        body: &CreateWebhookRequest,
    ) -> Result<CreateWebhookResponse> {
        self.execute_json(self.build_request(Method::POST, &["v1", "webhooks"], Some(body))?)
            .await
    }

    /// Deletes a webhook (`DELETE /v1/webhooks/{webhook}`, scope `webhooks:write`).
    /// Returns 204 No Content.
    pub async fn delete_webhook(&self, webhook: &str) -> Result<()> {
        self.execute_no_content(self.build_request(
            Method::DELETE,
            &["v1", "webhooks", webhook],
            None::<&()>,
        )?)
        .await
    }

    /// Lists API keys (`GET /v1/api-keys`, scope `keys:read`).
    pub async fn list_api_keys(&self) -> Result<ListEnvelope<ApiKey>> {
        self.execute_json(self.build_request(Method::GET, &["v1", "api-keys"], None::<&()>)?)
            .await
    }

    /// Creates an API key and returns its one-time secret as a field on the
    /// created object (`POST /v1/api-keys`, scope `keys:write`).
    pub async fn create_api_key(&self, body: &CreateApiKeyRequest) -> Result<CreateApiKeyResponse> {
        self.execute_json(self.build_request(Method::POST, &["v1", "api-keys"], Some(body))?)
            .await
    }

    /// Revokes an API key (`DELETE /v1/api-keys/{apiKeyId}`, scope `keys:write`).
    /// Returns 204 No Content.
    pub async fn revoke_api_key(&self, api_key_id: &str) -> Result<()> {
        self.execute_no_content(self.build_request(
            Method::DELETE,
            &["v1", "api-keys", api_key_id],
            None::<&()>,
        )?)
        .await
    }

    /// Revokes the API key whose stored prefix matches `token` server-side, used
    /// by `dairo logout`.
    ///
    /// The backend never returns key secrets after creation; a key is only
    /// addressable for revocation by its `id`. The token the CLI holds is the raw
    /// secret, so this resolves the secret to its `id` by matching the backend's
    /// stored `key_prefix` (the first 18 chars of the secret followed by `...`,
    /// per `mcp/oauth.rs`) against the listed active keys, then revokes that id.
    ///
    /// Returns `Ok(true)` if a matching active key was found and revoked,
    /// `Ok(false)` if no active key matched (already revoked, or the key cannot be
    /// resolved). Requires the `keys:read` + `keys:write` scopes, which the
    /// default `admin` login bundle includes.
    pub async fn revoke_token_by_prefix(&self, token: &str) -> Result<bool> {
        let Some(prefix) = token_revocation_prefix(token) else {
            return Ok(false);
        };
        let keys = self.list_api_keys().await?;
        let Some(target) = keys
            .data
            .iter()
            .find(|key| key.status.eq_ignore_ascii_case("active") && key.prefix == prefix)
        else {
            return Ok(false);
        };
        self.revoke_api_key(&target.id).await?;
        Ok(true)
    }

    /// Lists messages with keyset pagination (`GET /v1/messages`, scope
    /// `mail:read`). Returns the unified list envelope. Passing `channel=a2a`
    /// folds in the former `/v1/a2a/messages` agent-to-agent surface.
    pub async fn list_messages(&self, query: &MessageListQuery) -> Result<ListEnvelope<Message>> {
        let mut request = self.build_request(Method::GET, &["v1", "messages"], None::<&()>)?;
        apply_message_query(request.url_mut(), query);
        self.execute_json(request).await
    }

    /// Gets a single message including its full bodies (`GET /v1/messages/{id}`,
    /// scope `mail:read`). The redesign returns the flat message object.
    pub async fn get_message(&self, message_id: &str) -> Result<Message> {
        self.execute_json(self.build_request(
            Method::GET,
            &["v1", "messages", message_id],
            None::<&()>,
        )?)
        .await
    }

    pub async fn get_attachment_url(
        &self,
        attachment_id: &str,
        expiry_hours: Option<u32>,
    ) -> Result<AttachmentDownloadUrlResponse> {
        let mut request = self.build_request(
            Method::GET,
            &["v1", "attachments", attachment_id, "url"],
            None::<&()>,
        )?;
        if let Some(hours) = expiry_hours {
            request
                .url_mut()
                .query_pairs_mut()
                .append_pair("expiryHours", &hours.to_string());
        }
        self.execute_json(request).await
    }

    /// Branded share link for an attachment. Unlike `/url` (which returns a raw
    /// signed S3 URL), `/link` returns a Dairo-branded `shareUrl` plus
    /// `downloadUrl`. `expiry_hours` is clamped server-side to 1..=168.
    pub async fn get_attachment_link(
        &self,
        attachment_id: &str,
        expiry_hours: Option<u32>,
    ) -> Result<AttachmentDownloadUrlResponse> {
        let mut request = self.build_request(
            Method::GET,
            &["v1", "attachments", attachment_id, "link"],
            None::<&()>,
        )?;
        if let Some(hours) = expiry_hours {
            request
                .url_mut()
                .query_pairs_mut()
                .append_pair("expiryHours", &hours.to_string());
        }
        self.execute_json(request).await
    }

    pub async fn download_attachment_bytes(&self, attachment_id: &str) -> Result<Vec<u8>> {
        let request = self.build_request(
            Method::GET,
            &["v1", "attachments", attachment_id, "download"],
            None::<&()>,
        )?;
        let response = self.send_with_retry(request).await?;
        if !response.status().is_success() {
            return Err(error_from_response(response).await);
        }
        response
            .bytes()
            .await
            .map(|bytes| bytes.to_vec())
            .map_err(ApiError::Transport)
    }

    /// Lists threads with keyset pagination (`GET /v1/threads`, scope
    /// `mail:read`). Returns the unified list envelope.
    pub async fn list_threads(&self, query: &ThreadListQuery) -> Result<ListEnvelope<Thread>> {
        let mut request = self.build_request(Method::GET, &["v1", "threads"], None::<&()>)?;
        apply_thread_query(request.url_mut(), query);
        self.execute_json(request).await
    }

    /// Gets a thread plus its messages (`GET /v1/threads/{id}`, scope
    /// `mail:read`). The redesign flattens the thread object with `messages` as a
    /// field on it.
    pub async fn get_thread(&self, thread_id: &str) -> Result<ThreadResponse> {
        self.execute_json(self.build_request(
            Method::GET,
            &["v1", "threads", thread_id],
            None::<&()>,
        )?)
        .await
    }

    /// Lists outbound emails, most recent first (`GET /v1/emails`, scope
    /// `mail:read`; was `/v1/outbound-emails`). Passes through the unified list
    /// envelope verbatim.
    pub async fn list_outbound_emails(&self, limit: Option<u32>) -> Result<serde_json::Value> {
        let mut request = self.build_request(Method::GET, &["v1", "emails"], None::<&()>)?;
        if let Some(limit) = limit {
            request
                .url_mut()
                .query_pairs_mut()
                .append_pair("limit", &limit.to_string());
        }
        self.execute_json(request).await
    }

    /// Gets one outbound email plus its delivery-event timeline
    /// (`GET /v1/emails/{id}`, scope `mail:read`; was `/v1/outbound-emails/{id}`).
    pub async fn get_outbound_email(&self, email_id: &str) -> Result<serde_json::Value> {
        self.execute_json(self.build_request(
            Method::GET,
            &["v1", "emails", email_id],
            None::<&()>,
        )?)
        .await
    }

    /// Cancels a scheduled outbound email (`POST /v1/emails/{id}/cancel`, scope
    /// `mail:send`). Returns the canceled `email` object (`status: "canceled"`),
    /// or surfaces the backend's `409 Conflict` if the email is no longer
    /// scheduled.
    pub async fn cancel_outbound_email(&self, email_id: &str) -> Result<serde_json::Value> {
        self.execute_json(self.build_request(
            Method::POST,
            &["v1", "emails", email_id, "cancel"],
            None::<&()>,
        )?)
        .await
    }

    /// Lists the user's audit-log rows, newest first, with keyset pagination.
    /// Scope: `mail:read`. Returns
    /// `{ "logs": [...], "pagination": { "nextCursor": ... } }`.
    pub async fn list_audit_logs(&self, query: &AuditLogQuery) -> Result<serde_json::Value> {
        let mut request = self.build_request(Method::GET, &["v1", "audit-logs"], None::<&()>)?;
        {
            let mut pairs = request.url_mut().query_pairs_mut();
            if let Some(limit) = query.limit {
                pairs.append_pair("limit", &limit.to_string());
            }
            if let Some(cursor) = &query.cursor {
                pairs.append_pair("cursor", cursor);
            }
        }
        self.execute_json(request).await
    }

    /// Returns the tenant's dedicated IP pool status. Scope: `mail:read`. Returns
    /// `{ "pools": [...] }`.
    pub async fn list_dedicated_ips(&self) -> Result<serde_json::Value> {
        self.execute_json(self.build_request(Method::GET, &["v1", "dedicated-ips"], None::<&()>)?)
            .await
    }

    /// Reads a keyset-paginated slice of the durable event ledger
    /// (`GET /v1/events`). Scope: `mail:read`. This is the substrate `dairo
    /// listen` rides: each call returns events oldest-first in `(createdAt, id)`
    /// order plus a `nextCursor` to resume from.
    ///
    /// The endpoint supports two additive query params used by `dairo listen`:
    /// `tail=true` (return only the head cursor as of now, with `events: []`) and
    /// `wait=<0..=25>` seconds (server-side long-poll that holds the request open
    /// until a row appears or the budget elapses). Because a `wait` hang can keep
    /// the request open for up to `wait` seconds, this method overrides the
    /// shared 30s client timeout with a per-request `wait + 5s` deadline so the
    /// long-poll always returns cleanly inside the budget rather than tripping the
    /// global timeout.
    pub async fn list_events(&self, query: &EventsQuery) -> Result<EventsResponse> {
        let mut request = self.build_request(Method::GET, &["v1", "events"], None::<&()>)?;
        apply_events_query(request.url_mut(), query);
        // Long-poll: a `wait`-second hang must fit inside the request timeout.
        // Add a 5s margin for connect + the server returning the final empty
        // page. `tail`/`wait=0` requests still get a sane bounded timeout.
        *request.timeout_mut() = Some(events_request_timeout(query.wait));
        self.execute_json(request).await
    }

    /// Lists the delivery-event timeline for one outbound email
    /// (`GET /v1/emails/{id}/events`, scope `mail:read`). The redesign folds the
    /// former flat `/v1/outbound-events?emailId=` reader into this per-email
    /// sub-resource, so an `email_id` is now required.
    pub async fn list_outbound_events(
        &self,
        email_id: &str,
        limit: Option<u32>,
    ) -> Result<serde_json::Value> {
        let mut request = self.build_request(
            Method::GET,
            &["v1", "emails", email_id, "events"],
            None::<&()>,
        )?;
        if let Some(limit) = limit {
            request
                .url_mut()
                .query_pairs_mut()
                .append_pair("limit", &limit.to_string());
        }
        self.execute_json(request).await
    }

    // --- Templates (templates.rs) -----------------------------------------
    // Named container + immutable, append-only versions. Reads use
    // `templates:read`; create/patch/delete/version-publish use `templates:write`
    // (de-overloaded off mail:read/mail:send). Bodies carry free-form
    // `source`/`variables`, so requests are assembled as `serde_json::Value` and
    // responses pass through verbatim — matching the outbound/audit-logs
    // precedent.

    /// Lists active templates (`GET /v1/templates`, scope `templates:read`).
    pub async fn list_templates(&self) -> Result<serde_json::Value> {
        self.execute_json(self.build_request(Method::GET, &["v1", "templates"], None::<&()>)?)
            .await
    }

    /// Creates a template and publishes v1 (`POST /v1/templates`, scope
    /// `templates:write`). The source is dry-rendered at publish.
    pub async fn create_template(&self, body: &serde_json::Value) -> Result<serde_json::Value> {
        self.execute_json(self.build_request(Method::POST, &["v1", "templates"], Some(body))?)
            .await
    }

    /// Gets a template plus a resolved version including its `source` (`GET
    /// /v1/templates/{idOrSlug}`, scope `templates:read`). `version` pins a specific
    /// version instead of the container's `currentVersion`.
    pub async fn get_template(
        &self,
        id_or_slug: &str,
        version: Option<u32>,
    ) -> Result<serde_json::Value> {
        let mut request =
            self.build_request(Method::GET, &["v1", "templates", id_or_slug], None::<&()>)?;
        if let Some(version) = version {
            request
                .url_mut()
                .query_pairs_mut()
                .append_pair("version", &version.to_string());
        }
        self.execute_json(request).await
    }

    /// Updates template metadata or re-points `currentVersion` (`PATCH
    /// /v1/templates/{idOrSlug}`, scope `templates:write`). The source is immutable.
    pub async fn update_template(
        &self,
        id_or_slug: &str,
        body: &serde_json::Value,
    ) -> Result<serde_json::Value> {
        self.execute_json(self.build_request(
            Method::PATCH,
            &["v1", "templates", id_or_slug],
            Some(body),
        )?)
        .await
    }

    /// Archives a template (`DELETE /v1/templates/{idOrSlug}`, scope `templates:write`).
    pub async fn delete_template(&self, id_or_slug: &str) -> Result<serde_json::Value> {
        self.execute_json(self.build_request(
            Method::DELETE,
            &["v1", "templates", id_or_slug],
            None::<&()>,
        )?)
        .await
    }

    /// Lists a template's versions, newest first, without `source` (`GET
    /// /v1/templates/{idOrSlug}/versions`, scope `templates:read`).
    pub async fn list_template_versions(&self, id_or_slug: &str) -> Result<serde_json::Value> {
        self.execute_json(self.build_request(
            Method::GET,
            &["v1", "templates", id_or_slug, "versions"],
            None::<&()>,
        )?)
        .await
    }

    /// Reads one version of a template including its `source` (`GET
    /// /v1/templates/{idOrSlug}/versions/{version}`, scope `templates:read`).
    pub async fn get_template_version(
        &self,
        id_or_slug: &str,
        version: u32,
    ) -> Result<serde_json::Value> {
        self.execute_json(self.build_request(
            Method::GET,
            &[
                "v1",
                "templates",
                id_or_slug,
                "versions",
                &version.to_string(),
            ],
            None::<&()>,
        )?)
        .await
    }

    /// Publishes a new immutable version (`POST
    /// /v1/templates/{idOrSlug}/versions`, scope `templates:write`). Defaults to
    /// promoting it to `currentVersion`; `promote: false` publishes a draft.
    pub async fn publish_template_version(
        &self,
        id_or_slug: &str,
        body: &serde_json::Value,
    ) -> Result<serde_json::Value> {
        self.execute_json(self.build_request(
            Method::POST,
            &["v1", "templates", id_or_slug, "versions"],
            Some(body),
        )?)
        .await
    }

    // --- Events replay (events.rs) ----------------------------------------
    // The catch-up read `GET /v1/events` already has `list_events`
    // (long-poll-aware, used by `dairo listen`). `events list` reuses it; replay
    // is a `webhooks:write` POST.

    /// Re-delivers a ledger slice to the user's webhooks (`POST
    /// /v1/events/replay`, scope `events:write`). The body must carry exactly
    /// one lower bound (`since`, `sinceSeq` + `inboxId`, or `sinceTimestamp`).
    pub async fn replay_events(&self, body: &serde_json::Value) -> Result<serde_json::Value> {
        self.execute_json(self.build_request(
            Method::POST,
            &["v1", "events", "replay"],
            Some(body),
        )?)
        .await
    }

    // --- Agent passport (agent_passport.rs) -------------------------------
    // CRUD reads use `agents:read`. `verify` is a public, unauthenticated verdict
    // endpoint — it always answers 200 with a verdict, so a failed verification is
    // not a request error.

    /// Lists the caller's agent passports, newest first (`GET /v1/agents`,
    /// scope `agents:read`).
    pub async fn list_agents(&self) -> Result<serde_json::Value> {
        self.execute_json(self.build_request(Method::GET, &["v1", "agents"], None::<&()>)?)
            .await
    }

    /// Gets a passport by its uuid `id` or portable `agt_…` `agentId` (`GET
    /// /v1/agents/{idOrAgent}`, scope `agents:read`).
    pub async fn get_agent(&self, id_or_agent: &str) -> Result<serde_json::Value> {
        self.execute_json(self.build_request(
            Method::GET,
            &["v1", "agents", id_or_agent],
            None::<&()>,
        )?)
        .await
    }

    /// Verifies an agent's outbound attribution (`GET /v1/agents/verify`; was
    /// `/v1/verify`). Public/unauthenticated; always answers a verdict. Pass
    /// `{ id }` to attest from an outbound record, or `{ agent, kid, sig, ... }`
    /// to verify a reconstructed provenance signature. The bearer key is ignored
    /// here.
    pub async fn verify_agent(&self, query: &VerifyAgentQuery) -> Result<serde_json::Value> {
        let mut request =
            self.build_request(Method::GET, &["v1", "agents", "verify"], None::<&()>)?;
        apply_verify_query(request.url_mut(), query);
        self.execute_json(request).await
    }

    // --- Reputation (agent_reputation.rs) ---------------------------------

    /// Fleet view of every agent's circuit-breaker state, newest-tripped first
    /// (`GET /v1/agents/reputation`, scope `agents:read`; was top-level
    /// `/v1/reputation`).
    pub async fn list_reputation(&self) -> Result<serde_json::Value> {
        self.execute_json(self.build_request(
            Method::GET,
            &["v1", "agents", "reputation"],
            None::<&()>,
        )?)
        .await
    }

    // --- Budgets (budgets.rs) ---------------------------------------------
    // Reads reuse `mail:read`; setting a cap (PUT) requires `keys:write`. The
    // `get` resolver takes a scope (`account`, or a key/agent `scopeId`); `set`
    // is an idempotent upsert keyed on `(scope, scopeId)`.

    /// Lists every budget with its live windowed usage (`GET /v1/budgets`, scope
    /// `budgets:read`).
    pub async fn list_budgets(&self) -> Result<serde_json::Value> {
        self.execute_json(self.build_request(Method::GET, &["v1", "budgets"], None::<&()>)?)
            .await
    }

    /// Gets a single budget by scope: `account`, or a key/agent budget by its
    /// `scopeId` (`GET /v1/budgets/{scope}`, scope `budgets:read`).
    pub async fn get_budget(&self, scope: &str) -> Result<serde_json::Value> {
        self.execute_json(self.build_request(
            Method::GET,
            &["v1", "budgets", scope],
            None::<&()>,
        )?)
        .await
    }

    /// Sets/replaces a budget (`PUT /v1/budgets`, scope `budgets:write`).
    /// Idempotent upsert keyed on `(scope, scopeId)`.
    pub async fn set_budget(&self, body: &serde_json::Value) -> Result<serde_json::Value> {
        self.execute_json(self.build_request(Method::PUT, &["v1", "budgets"], Some(body))?)
            .await
    }

    /// Deletes a budget by scope (`DELETE /v1/budgets/{scope}`, scope
    /// `budgets:write`). Returns 204 No Content; replaces the old
    /// `enabled:false` disable-as-delete.
    pub async fn delete_budget(&self, scope: &str) -> Result<()> {
        self.execute_no_content(self.build_request(
            Method::DELETE,
            &["v1", "budgets", scope],
            None::<&()>,
        )?)
        .await
    }

    // --- Compliance / account (compliance.rs) -----------------------------
    // EU sovereignty surfaces. The `/v1/compliance/*` junk-drawer is gone:
    // residency is now an account property, erasure jobs are a real resource.

    /// Reads the data-residency / subprocessor posture, including the honest
    /// CLOUD-Act exposure note (`GET /v1/account/residency`, scope
    /// `account:read`; was `/v1/compliance/residency`).
    pub async fn compliance_residency(&self) -> Result<serde_json::Value> {
        self.execute_json(self.build_request(
            Method::GET,
            &["v1", "account", "residency"],
            None::<&()>,
        )?)
        .await
    }

    /// Enqueues a GDPR erasure job (`POST /v1/erasure-jobs`, scope
    /// `compliance:write`; merges the former `/compliance/erase` +
    /// `/compliance/purge-inbox` via a typed body). Accepted async with 202.
    pub async fn create_erasure_job(&self, body: &serde_json::Value) -> Result<serde_json::Value> {
        self.execute_json(self.build_request(Method::POST, &["v1", "erasure-jobs"], Some(body))?)
            .await
    }

    /// Lists erasure jobs, newest first (`GET /v1/erasure-jobs`, scope
    /// `compliance:read`).
    pub async fn list_erasure_jobs(&self) -> Result<serde_json::Value> {
        self.execute_json(self.build_request(Method::GET, &["v1", "erasure-jobs"], None::<&()>)?)
            .await
    }

    /// Polls a subject-erasure job, including tallies and the signed deletion
    /// certificate once completed (`GET /v1/erasure-jobs/{id}`, scope
    /// `compliance:read`; was `/v1/compliance/erasure-jobs/{id}`).
    pub async fn get_erasure_job(&self, job_id: &str) -> Result<serde_json::Value> {
        self.execute_json(self.build_request(
            Method::GET,
            &["v1", "erasure-jobs", job_id],
            None::<&()>,
        )?)
        .await
    }

    // --- A2A mail (folded into messages) ----------------------------------
    // Cross-tenant agent-to-agent hop receipts. The private `/v1/a2a/messages`
    // acronym is gone: A2A hops are read through the messages surface with
    // `channel=a2a` (REDESIGN §9 rows 14-15).

    /// Lists agent-to-agent hop receipts with keyset pagination
    /// (`GET /v1/messages?channel=a2a`, scope `mail:read`; was
    /// `/v1/a2a/messages`).
    pub async fn list_a2a_messages(&self, query: &A2aMessageQuery) -> Result<serde_json::Value> {
        let mut request = self.build_request(Method::GET, &["v1", "messages"], None::<&()>)?;
        {
            let mut pairs = request.url_mut().query_pairs_mut();
            pairs.append_pair("channel", "a2a");
            if let Some(limit) = query.limit {
                pairs.append_pair("limit", &limit.to_string());
            }
            if let Some(cursor) = &query.cursor {
                pairs.append_pair("cursor", cursor);
            }
            if let Some(inbox_id) = &query.inbox_id {
                pairs.append_pair("inboxId", inbox_id);
            }
        }
        self.execute_json(request).await
    }

    /// Gets a single A2A hop receipt (`GET /v1/messages/{id}`, scope `mail:read`;
    /// was `/v1/a2a/messages/{id}` — an a2a id resolves here too).
    pub async fn get_a2a_message(&self, id: &str) -> Result<serde_json::Value> {
        self.execute_json(self.build_request(Method::GET, &["v1", "messages", id], None::<&()>)?)
            .await
    }

    // --- Letters (physical-mail surface) ----------------------------
    // The `/v1/letters` resource. Reads use `letters:read`; create/cancel use
    // `letters:send`. Responses pass through as `serde_json::Value` (the unified
    // envelope, rendered verbatim by `print_json`) matching the
    // outbound/templates convention for the newer resource families. The two
    // POST mutations (create, cancel) ride the shared body-inclusive default
    // idempotency-key path automatically.

    /// Creates (and queues) a physical-mail letter (`POST /v1/letters`, scope
    /// `letters:send`). Returns the single `letter` object envelope. No provider
    /// is contacted on the request path; the worker fleet does the slow work.
    pub async fn create_letter(&self, body: &CreateLetterRequest) -> Result<serde_json::Value> {
        self.execute_json(self.build_request(Method::POST, &["v1", "letters"], Some(body))?)
            .await
    }

    /// Lists letters, most-recent-first, with keyset pagination
    /// (`GET /v1/letters`, scope `letters:read`). Optional `status`/`country`
    /// filters narrow the page.
    pub async fn list_letters(&self, query: &LetterListQuery) -> Result<serde_json::Value> {
        let mut request = self.build_request(Method::GET, &["v1", "letters"], None::<&()>)?;
        apply_letter_query(request.url_mut(), query);
        self.execute_json(request).await
    }

    /// Gets one letter plus its inlined delivery-event timeline
    /// (`GET /v1/letters/{id}`, scope `letters:read`).
    pub async fn get_letter(&self, id: &str) -> Result<serde_json::Value> {
        self.execute_json(self.build_request(Method::GET, &["v1", "letters", id], None::<&()>)?)
            .await
    }

    /// Cancels a letter that has not yet been dispatched
    /// (`POST /v1/letters/{id}/cancel`, scope `letters:send`). Returns the
    /// canceled `letter` object, or surfaces the backend's `409 Conflict`
    /// (`letter_not_cancelable`) when it can no longer be canceled.
    pub async fn cancel_letter(&self, id: &str) -> Result<serde_json::Value> {
        self.execute_json(self.build_request(
            Method::POST,
            &["v1", "letters", id, "cancel"],
            // An empty JSON object body is accepted; sending it also gives the
            // default idempotency key a stable body to fold in.
            Some(&serde_json::json!({})),
        )?)
        .await
    }

    /// Lists a letter's delivery events in the unified list envelope
    /// (`GET /v1/letters/{id}/events`, scope `letters:read`).
    pub async fn list_letter_events(
        &self,
        id: &str,
        limit: Option<u32>,
        cursor: Option<&str>,
    ) -> Result<serde_json::Value> {
        let mut request =
            self.build_request(Method::GET, &["v1", "letters", id, "events"], None::<&()>)?;
        {
            let mut pairs = request.url_mut().query_pairs_mut();
            if let Some(limit) = limit {
                pairs.append_pair("limit", &limit.to_string());
            }
            if let Some(cursor) = cursor {
                pairs.append_pair("cursor", cursor);
            }
        }
        self.execute_json(request).await
    }

    /// Computes the price for a hypothetical or real letter without creating one
    /// (`POST /v1/letters/price`, scope `letters:read`). Returns the
    /// `letter_price` object.
    pub async fn price_letter(&self, body: &LetterPriceRequest) -> Result<serde_json::Value> {
        self.execute_json(self.build_request(
            Method::POST,
            &["v1", "letters", "price"],
            Some(body),
        )?)
        .await
    }

    // --- Storage buckets (buckets.rs) -------------------------------------
    // The `/v1/buckets` named object store. Bucket/object reads use
    // `buckets:read`; create/patch/delete and the object upload/finalize/delete
    // mutations use `buckets:write`. Responses pass through as
    // `serde_json::Value` (the unified envelope, rendered verbatim by
    // `print_json`) for the bucket/object CRUD, matching the letters/templates
    // convention; the upload/download flows need the presigned-URL fields, so
    // those use typed structs.

    /// Lists buckets, each carrying its `usedBytes`/`objectCount`
    /// (`GET /v1/buckets`, scope `buckets:read`). The default bucket is lazily
    /// seeded server-side before listing.
    pub async fn list_buckets(&self) -> Result<serde_json::Value> {
        self.execute_json(self.build_request(Method::GET, &["v1", "buckets"], None::<&()>)?)
            .await
    }

    /// Creates a named bucket (`POST /v1/buckets`, scope `buckets:write`).
    /// Returns the single `bucket` object envelope. Surfaces the backend's
    /// `429` (per-plan bucket limit) or `409` (duplicate name) verbatim.
    pub async fn create_bucket(&self, body: &CreateBucketRequest) -> Result<serde_json::Value> {
        self.execute_json(self.build_request(Method::POST, &["v1", "buckets"], Some(body))?)
            .await
    }

    /// Gets one bucket envelope (`GET /v1/buckets/{bucketId}`, scope
    /// `buckets:read`).
    pub async fn get_bucket(&self, bucket_id: &str) -> Result<serde_json::Value> {
        self.execute_json(self.build_request(
            Method::GET,
            &["v1", "buckets", bucket_id],
            None::<&()>,
        )?)
        .await
    }

    /// Archives a bucket and marks its child objects deleted
    /// (`DELETE /v1/buckets/{bucketId}`, scope `buckets:write`). Surfaces the
    /// backend's `409` when the bucket is the protected default. Returns the
    /// archived `bucket` envelope.
    pub async fn delete_bucket(&self, bucket_id: &str) -> Result<()> {
        // DELETE /v1/buckets/{id} responds 204 with an EMPTY body; parsing it as
        // JSON (execute_json) fails with "EOF while parsing a value" even though the
        // delete succeeded. Use the no-content path like delete_webhook/revoke_api_key.
        self.execute_no_content(self.build_request(
            Method::DELETE,
            &["v1", "buckets", bucket_id],
            None::<&()>,
        )?)
        .await
    }

    /// Lists a bucket's objects with keyset pagination
    /// (`GET /v1/buckets/{bucketId}/objects`, scope `buckets:read`).
    pub async fn list_bucket_objects(
        &self,
        bucket_id: &str,
        query: &BucketObjectListQuery,
    ) -> Result<serde_json::Value> {
        let mut request = self.build_request(
            Method::GET,
            &["v1", "buckets", bucket_id, "objects"],
            None::<&()>,
        )?;
        apply_bucket_object_query(request.url_mut(), query);
        self.execute_json(request).await
    }

    /// Soft-deletes a bucket object (`DELETE
    /// /v1/buckets/{bucketId}/objects/{objectId}`, scope `buckets:write`). The
    /// ledger row is marked deleted and the S3 object is removed best-effort.
    pub async fn delete_bucket_object(&self, bucket_id: &str, object_id: &str) -> Result<()> {
        // DELETE /v1/buckets/{id}/objects/{objectId} responds 204 with an EMPTY body;
        // execute_json would fail with "EOF while parsing a value" despite success.
        self.execute_no_content(self.build_request(
            Method::DELETE,
            &["v1", "buckets", bucket_id, "objects", object_id],
            None::<&()>,
        )?)
        .await
    }

    /// Initiates an upload (`POST /v1/buckets/{bucketId}/objects`, scope
    /// `buckets:write`). Returns the presigned S3 PUT URL plus any SSE headers
    /// that must accompany the PUT. Records nothing in the ledger yet.
    pub async fn initiate_bucket_upload(
        &self,
        bucket_id: &str,
        body: &InitiateUploadRequest,
    ) -> Result<InitiateUploadResponse> {
        self.execute_json(self.build_request(
            Method::POST,
            &["v1", "buckets", bucket_id, "objects"],
            Some(body),
        )?)
        .await
    }

    /// Finalizes an upload (`POST
    /// /v1/buckets/{bucketId}/objects/{objectId}/finalize`, scope
    /// `buckets:write`). The backend HEADs the S3 object for its true size,
    /// gates the storage limit, and records the ledger object. Returns the
    /// `bucket_object` envelope.
    pub async fn finalize_bucket_upload(
        &self,
        bucket_id: &str,
        object_id: &str,
    ) -> Result<serde_json::Value> {
        self.execute_json(self.build_request(
            Method::POST,
            &["v1", "buckets", bucket_id, "objects", object_id, "finalize"],
            // An empty JSON object gives the default idempotency key a stable
            // body to fold in, matching the letters-cancel precedent.
            Some(&serde_json::json!({})),
        )?)
        .await
    }

    /// Requests a presigned download URL for a bucket object (`GET
    /// /v1/buckets/{bucketId}/objects/{objectId}/download`, scope
    /// `buckets:read`).
    pub async fn get_bucket_object_download(
        &self,
        bucket_id: &str,
        object_id: &str,
    ) -> Result<BucketObjectDownloadResponse> {
        self.execute_json(self.build_request(
            Method::GET,
            &["v1", "buckets", bucket_id, "objects", object_id, "download"],
            None::<&()>,
        )?)
        .await
    }

    /// Initiates a multipart upload (`POST
    /// /v1/buckets/{bucketId}/objects/multipart`, scope `buckets:write`).
    /// Returns the S3 upload id, placeholder object id, part size/count and the
    /// per-part branded presigned PUT urls. Records a `multipart_pending`
    /// placeholder ledger row.
    pub async fn initiate_multipart_upload(
        &self,
        bucket_id: &str,
        body: &InitiateMultipartRequest,
    ) -> Result<InitiateMultipartResponse> {
        self.execute_json(self.build_request(
            Method::POST,
            &["v1", "buckets", bucket_id, "objects", "multipart"],
            Some(body),
        )?)
        .await
    }

    /// Completes a multipart upload (`POST
    /// /v1/buckets/{bucketId}/objects/multipart/{uploadId}/complete`, scope
    /// `buckets:write`). The backend CompleteMultipartUpload's the parts
    /// (sorted ascending), HEADs the true size, gates the storage limit, and
    /// records the object. Idempotent. Returns the `bucket_object` envelope.
    pub async fn complete_multipart_upload(
        &self,
        bucket_id: &str,
        upload_id: &str,
        body: &CompleteMultipartRequest,
    ) -> Result<serde_json::Value> {
        self.execute_json(self.build_request(
            Method::POST,
            &[
                "v1", "buckets", bucket_id, "objects", "multipart", upload_id, "complete",
            ],
            Some(body),
        )?)
        .await
    }

    /// Aborts a multipart upload (`POST
    /// /v1/buckets/{bucketId}/objects/multipart/{uploadId}/abort`, scope
    /// `buckets:write`). Frees the staged S3 parts and soft-deletes the
    /// placeholder ledger row. The backend returns `null`.
    pub async fn abort_multipart_upload(
        &self,
        bucket_id: &str,
        upload_id: &str,
        body: &AbortMultipartRequest,
    ) -> Result<()> {
        // The abort endpoint returns a `null` body; parsing it as a typed JSON
        // value would be pointless, so discard it (mirrors the DELETE 204 path
        // via execute_no_content, which ignores any body on a 2xx).
        self.execute_no_content(self.build_request(
            Method::POST,
            &[
                "v1", "buckets", bucket_id, "objects", "multipart", upload_id, "abort",
            ],
            Some(body),
        )?)
        .await
    }

    /// Uploads a local file to a bucket end-to-end, AUTO-SELECTING the transfer
    /// strategy by size: files at/under [`MULTIPART_THRESHOLD_BYTES`] take the
    /// single-PUT branded flow; larger files take the resumable multipart flow
    /// (parallel + per-part retried part PUTs). Returns the finalized
    /// `bucket_object` envelope either way.
    pub async fn upload_file(
        &self,
        bucket_id: &str,
        filename: &str,
        content_type: &str,
        bytes: Vec<u8>,
    ) -> Result<serde_json::Value> {
        if bytes.len() as u64 > MULTIPART_THRESHOLD_BYTES {
            self.upload_file_multipart(bucket_id, filename, content_type, bytes)
                .await
        } else {
            self.upload_file_single(bucket_id, filename, content_type, bytes)
                .await
        }
    }

    /// Single-PUT branded upload: initiate (presigned PUT), PUT the bytes
    /// straight to S3 (echoing the required SSE headers), then finalize so the
    /// ledger records the object's true size.
    ///
    /// The PUT goes to a presigned S3 URL with no Dairo bearer auth, so it uses
    /// a fresh short-lived client rather than `self.http` (which always attaches
    /// the API key). The bytes are read fully into memory, matching the
    /// attachment-send path.
    pub async fn upload_file_single(
        &self,
        bucket_id: &str,
        filename: &str,
        content_type: &str,
        bytes: Vec<u8>,
    ) -> Result<serde_json::Value> {
        let initiate = self
            .initiate_bucket_upload(
                bucket_id,
                &InitiateUploadRequest {
                    filename: filename.to_string(),
                    content_type: content_type.to_string(),
                    expected_bytes: Some(bytes.len() as u64),
                },
            )
            .await?;

        // The presigned PUT must NOT carry the Dairo bearer token, and must echo
        // every header the URL was signed with (the SSE headers), or S3 returns
        // a SignatureDoesNotMatch 403.
        let s3 = reqwest::Client::builder()
            .user_agent(USER_AGENT)
            .timeout(REQUEST_TIMEOUT)
            .build()
            .map_err(ApiError::BuildRequest)?;
        // The backend's signed `headers` map already includes the Content-Type the
        // URL was signed with. Setting it again here makes reqwest send TWO
        // Content-Type headers, which it joins as `text/plain,text/plain` — breaking
        // the presigned-PUT signature (S3 403 SignatureDoesNotMatch). Only set the
        // explicit Content-Type when the signed headers don't already carry one.
        let signed_has_content_type = initiate
            .headers
            .iter()
            .any(|(name, _)| name.eq_ignore_ascii_case("content-type"));
        let mut put = s3.put(&initiate.upload_url);
        if !signed_has_content_type {
            put = put.header("Content-Type", content_type);
        }
        for (name, value) in &initiate.headers {
            put = put.header(name, value);
        }
        let put = put.body(bytes);
        let response = put.send().await.map_err(ApiError::Transport)?;
        if !response.status().is_success() {
            let status = response.status();
            let message = response
                .text()
                .await
                .unwrap_or_else(|_| "presigned upload failed".to_string());
            return Err(ApiError::Api { status, message });
        }

        self.finalize_bucket_upload(bucket_id, &initiate.object_id)
            .await
    }

    /// Resumable multipart upload: initiate (records a placeholder + returns the
    /// per-part branded presigned PUT urls), PUT every part to its branded url
    /// with bounded parallelism and per-part retry (reading the `ETag` response
    /// header off each), then complete with the collected `{partNumber, etag}`.
    /// On any failure the upload is aborted (best-effort) so no staged parts are
    /// left billing. Returns the finalized `bucket_object` envelope.
    pub async fn upload_file_multipart(
        &self,
        bucket_id: &str,
        filename: &str,
        content_type: &str,
        bytes: Vec<u8>,
    ) -> Result<serde_json::Value> {
        let total_bytes = bytes.len() as u64;
        let initiate = self
            .initiate_multipart_upload(
                bucket_id,
                &InitiateMultipartRequest {
                    filename: filename.to_string(),
                    content_type: content_type.to_string(),
                    total_bytes,
                    // Let the backend pick the default part size (256MiB).
                    part_size: None,
                },
            )
            .await?;

        // Upload every part, then complete. Any error past the initiate must
        // abort the upload so staged parts don't linger.
        match self.upload_parts_then_complete(bucket_id, &initiate, bytes).await {
            Ok(object) => Ok(object),
            Err(error) => {
                // Best-effort abort; preserve the original error regardless of
                // whether the abort itself succeeds.
                let _ = self
                    .abort_multipart_upload(
                        bucket_id,
                        &initiate.upload_id,
                        &AbortMultipartRequest {
                            object_id: initiate.object_id.clone(),
                        },
                    )
                    .await;
                Err(error)
            }
        }
    }

    /// Slices `bytes` into the parts described by `initiate`, PUTs them to their
    /// branded urls with bounded parallelism + per-part retry, then completes.
    async fn upload_parts_then_complete(
        &self,
        bucket_id: &str,
        initiate: &InitiateMultipartResponse,
        bytes: Vec<u8>,
    ) -> Result<serde_json::Value> {
        // A presigned PUT carries no Dairo bearer token and must echo the
        // initiate-level signed headers verbatim, so use a fresh client with a
        // part-sized timeout rather than `self.http` (which attaches the key).
        let s3 = std::sync::Arc::new(
            reqwest::Client::builder()
                .user_agent(USER_AGENT)
                .timeout(PART_UPLOAD_TIMEOUT)
                .build()
                .map_err(ApiError::BuildRequest)?,
        );
        let bytes = std::sync::Arc::new(bytes);
        let headers = std::sync::Arc::new(initiate.headers.clone());
        let part_size = initiate.part_size;
        let total = bytes.len() as u64;

        // Drive a bounded number of part PUTs concurrently. Parts are pulled off
        // a shared index; each task computes its own byte range from part_size.
        let next = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let parts = std::sync::Arc::new(initiate.parts.clone());
        let workers = MULTIPART_PARALLELISM.min(parts.len().max(1));

        let mut set = tokio::task::JoinSet::new();
        for _ in 0..workers {
            let s3 = s3.clone();
            let bytes = bytes.clone();
            let headers = headers.clone();
            let parts = parts.clone();
            let next = next.clone();
            set.spawn(async move {
                let mut done: Vec<CompletedPart> = Vec::new();
                loop {
                    let idx = next.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                    if idx >= parts.len() {
                        break;
                    }
                    let part = &parts[idx];
                    // `part_number` is 1-based; the byte range is contiguous.
                    let start = (part.part_number as u64 - 1) * part_size;
                    let end = (start + part_size).min(total);
                    let chunk = bytes[start as usize..end as usize].to_vec();
                    let etag = put_one_part_with_retry(&s3, &part.url, &headers, chunk).await?;
                    done.push(CompletedPart {
                        part_number: part.part_number,
                        etag,
                    });
                }
                Ok::<_, ApiError>(done)
            });
        }

        let mut completed: Vec<CompletedPart> = Vec::new();
        while let Some(joined) = set.join_next().await {
            match joined {
                Ok(Ok(mut done)) => completed.append(&mut done),
                Ok(Err(error)) => {
                    set.abort_all();
                    return Err(error);
                }
                Err(join_error) => {
                    set.abort_all();
                    return Err(ApiError::Api {
                        status: StatusCode::INTERNAL_SERVER_ERROR,
                        message: format!("multipart upload task failed: {join_error}"),
                    });
                }
            }
        }

        // The backend sorts ascending, but send them ordered for a stable body.
        completed.sort_by_key(|part| part.part_number);

        self.complete_multipart_upload(
            bucket_id,
            &initiate.upload_id,
            &CompleteMultipartRequest {
                object_id: initiate.object_id.clone(),
                parts: completed,
            },
        )
        .await
    }

    /// Downloads a bucket object's bytes: requests a presigned GET URL, then
    /// fetches it from S3 (no Dairo bearer auth on the presigned URL).
    pub async fn download_file(&self, bucket_id: &str, object_id: &str) -> Result<Vec<u8>> {
        let download = self
            .get_bucket_object_download(bucket_id, object_id)
            .await?;
        let s3 = reqwest::Client::builder()
            .user_agent(USER_AGENT)
            .timeout(REQUEST_TIMEOUT)
            .build()
            .map_err(ApiError::BuildRequest)?;
        let response = s3
            .get(&download.download_url)
            .send()
            .await
            .map_err(ApiError::Transport)?;
        if !response.status().is_success() {
            let status = response.status();
            let message = response
                .text()
                .await
                .unwrap_or_else(|_| "presigned download failed".to_string());
            return Err(ApiError::Api { status, message });
        }
        response
            .bytes()
            .await
            .map(|bytes| bytes.to_vec())
            .map_err(ApiError::Transport)
    }

    /// Fetches the public MCP tool catalog (`GET /v1/mcp/catalog`), the single
    /// source of truth for the hosted MCP surface served at `api.dairo.app/mcp`.
    ///
    /// The catalog itself is public and cacheable; the bearer key this client
    /// always attaches is ignored by the endpoint unless `for_me` is set. When
    /// `for_me` is true we add `?for=me`, which makes the server annotate every
    /// tool with `allowed: bool` (and echo `keyScopes`) computed from the calling
    /// key's scopes — a pure in-memory filter, no extra round-trips.
    pub async fn mcp_catalog(&self, for_me: bool) -> Result<serde_json::Value> {
        let mut request =
            self.build_request(Method::GET, &["v1", "mcp", "catalog"], None::<&()>)?;
        if for_me {
            request.url_mut().query_pairs_mut().append_pair("for", "me");
        }
        self.execute_json(request).await
    }

    pub(crate) fn build_request<T: Serialize>(
        &self,
        method: Method,
        path_segments: &[&str],
        body: Option<T>,
    ) -> Result<Request> {
        self.build_request_with_idempotency(method, path_segments, body, None)
    }

    /// Builds a request, attaching a stable, caller-supplied `Idempotency-Key`
    /// for mutating verbs.
    ///
    /// A per-invocation random key would defeat retry de-duplication: a retried
    /// POST would carry a fresh key and the server would treat it as a new
    /// request. When the caller does not supply a key we derive a deterministic
    /// one from the method + path so the *same logical request* re-sends with
    /// the *same* key.
    pub(crate) fn build_request_with_idempotency<T: Serialize>(
        &self,
        method: Method,
        path_segments: &[&str],
        body: Option<T>,
        idempotency_key: Option<&str>,
    ) -> Result<Request> {
        let mut url = self.base_url.clone();
        {
            let mut segments = url
                .path_segments_mut()
                .map_err(|_| url::ParseError::SetHostOnCannotBeABaseUrl)?;
            segments.pop_if_empty();
            for segment in path_segments {
                segments.push(segment);
            }
        }

        let mut builder = self
            .http
            .request(method.clone(), url.clone())
            .bearer_auth(&self.api_key)
            .header("Accept", "application/json");

        if matches!(
            method,
            Method::POST | Method::PUT | Method::PATCH | Method::DELETE
        ) {
            let key = match idempotency_key {
                Some(key) if !key.trim().is_empty() => key.to_string(),
                _ => {
                    // Fold the serialized body into the default key so two *different*
                    // requests to the same endpoint (e.g. two distinct `dairo send`
                    // emails) get distinct keys. With method+path alone, every POST to
                    // a path collides on one key and the server's idempotency dedup
                    // returns the FIRST request's row for all the rest — so only one
                    // email per endpoint could ever be sent.
                    let body_repr = body
                        .as_ref()
                        .and_then(|body| serde_json::to_string(body).ok())
                        .unwrap_or_default();
                    default_idempotency_key(&method, url.path(), &body_repr)
                }
            };
            builder = builder.header("Idempotency-Key", key);
        }

        if let Some(body) = body {
            builder = builder.json(&body);
        }

        builder.build().map_err(ApiError::BuildRequest)
    }

    /// Sends a request with a bounded retry/backoff policy for transient
    /// server-side failures (429 Too Many Requests, 502 Bad Gateway) and
    /// transient transport errors. Because every mutating request carries a
    /// stable `Idempotency-Key`, replaying a POST is safe.
    async fn send_with_retry(&self, request: Request) -> Result<reqwest::Response> {
        // `request` is the live request to send on the next attempt. A retry
        // replays a clone; once we exhaust retries (or the body is not
        // cloneable) we consume `request` itself on the final attempt.
        let mut request = request;
        let mut attempt: u32 = 0;
        loop {
            let can_retry = attempt < MAX_RETRIES;
            // Prepare this attempt's request and, if a retry is still possible,
            // keep a clone for the next iteration.
            let (this_attempt, next) = if can_retry {
                match request.try_clone() {
                    Some(clone) => (request, Some(clone)),
                    // Non-cloneable body: send the original, disable retries.
                    None => {
                        return self
                            .http
                            .execute(request)
                            .await
                            .map_err(ApiError::Transport)
                    }
                }
            } else {
                (request, None)
            };

            match self.http.execute(this_attempt).await {
                Ok(response) => {
                    if let (true, Some(next)) = (is_retryable_status(response.status()), next) {
                        backoff(attempt, RETRY_BASE_BACKOFF, RETRY_MAX_BACKOFF).await;
                        attempt += 1;
                        request = next;
                        continue;
                    }
                    return Ok(response);
                }
                Err(error) => {
                    if let (Some(next), true) = (next, error.is_timeout() || error.is_connect()) {
                        backoff(attempt, RETRY_BASE_BACKOFF, RETRY_MAX_BACKOFF).await;
                        attempt += 1;
                        request = next;
                        continue;
                    }
                    return Err(ApiError::Transport(error));
                }
            }
        }
    }

    async fn execute_json<T: for<'de> Deserialize<'de>>(&self, request: Request) -> Result<T> {
        let response = self.send_with_retry(request).await?;

        if response.status().is_success() {
            return response.json::<T>().await.map_err(ApiError::Transport);
        }

        Err(error_from_response(response).await)
    }

    /// Executes a mutating request that returns no body on success. The redesign
    /// answers every successful delete with `204 No Content` (and no JSON body),
    /// so this drains the success case without attempting to parse a body and
    /// still surfaces the structured error envelope on failure.
    async fn execute_no_content(&self, request: Request) -> Result<()> {
        let response = self.send_with_retry(request).await?;

        if response.status().is_success() {
            return Ok(());
        }

        Err(error_from_response(response).await)
    }
}

/// Rejects base URLs that would send the bearer API key in cleartext. HTTPS is
/// always allowed; plain HTTP is only permitted for explicit loopback hosts so
/// local development against a dev server keeps working.
fn require_secure_base_url(url: &Url) -> Result<()> {
    match url.scheme() {
        "https" => Ok(()),
        "http" if is_local_host(url) => Ok(()),
        _ => Err(ApiError::InsecureBaseUrl(url.as_str().to_string())),
    }
}

fn is_local_host(url: &Url) -> bool {
    match url.host() {
        Some(url::Host::Domain(host)) => {
            let host = host.to_ascii_lowercase();
            host == "localhost" || host == "localhost." || host.ends_with(".localhost")
        }
        Some(url::Host::Ipv4(ip)) => ip.is_loopback(),
        Some(url::Host::Ipv6(ip)) => ip.is_loopback(),
        None => false,
    }
}

fn is_retryable_status(status: StatusCode) -> bool {
    status == StatusCode::TOO_MANY_REQUESTS || status == StatusCode::BAD_GATEWAY
}

/// Builds an [`ApiError::Api`] from a failed response by decoding the canonical
/// error envelope (`{ "error": { message, code, type, param } }`). If the body
/// is missing or unparseable it falls back to the HTTP status reason phrase.
/// The response is consumed because reading the body requires ownership.
async fn error_from_response(response: reqwest::Response) -> ApiError {
    let status = response.status();
    let message = match response.json::<ErrorResponse>().await {
        Ok(error) => error.error.display_message(),
        Err(_) => status
            .canonical_reason()
            .unwrap_or("unexpected API error")
            .to_string(),
    };
    ApiError::Api { status, message }
}

/// PUTs one multipart part to its branded presigned url, echoing the signed
/// `headers` (`x-amz-content-sha256: UNSIGNED-PAYLOAD`; no SSE header on parts),
/// with bounded retry on transient failures. Returns the `ETag` response header
/// (quotes preserved) which the complete step reports back per part. The retry
/// here is the resumability win: a flaky part PUT is replayed rather than
/// failing the whole large upload.
async fn put_one_part_with_retry(
    s3: &reqwest::Client,
    url: &str,
    headers: &std::collections::BTreeMap<String, String>,
    chunk: Vec<u8>,
) -> Result<String> {
    let mut attempt: u32 = 0;
    loop {
        let mut put = s3.put(url).body(chunk.clone());
        for (name, value) in headers {
            put = put.header(name, value);
        }
        match put.send().await {
            Ok(response) if response.status().is_success() => {
                // S3 returns the part's ETag in the response header; it is the
                // value the complete call must echo back per part.
                return response
                    .headers()
                    .get(reqwest::header::ETAG)
                    .and_then(|value| value.to_str().ok())
                    .map(str::to_string)
                    .ok_or_else(|| ApiError::Api {
                        status: StatusCode::BAD_GATEWAY,
                        message: "part upload succeeded but returned no ETag header".to_string(),
                    });
            }
            Ok(response) => {
                let status = response.status();
                if attempt < PART_MAX_RETRIES && is_retryable_status(status) {
                    backoff(attempt, RETRY_BASE_BACKOFF, RETRY_MAX_BACKOFF).await;
                    attempt += 1;
                    continue;
                }
                let message = response
                    .text()
                    .await
                    .unwrap_or_else(|_| "part upload failed".to_string());
                return Err(ApiError::Api { status, message });
            }
            Err(error) => {
                if attempt < PART_MAX_RETRIES && (error.is_timeout() || error.is_connect()) {
                    backoff(attempt, RETRY_BASE_BACKOFF, RETRY_MAX_BACKOFF).await;
                    attempt += 1;
                    continue;
                }
                return Err(ApiError::Transport(error));
            }
        }
    }
}

/// Sleeps for an exponentially increasing, capped backoff before the next
/// retry attempt. `base` is doubled each attempt and clamped to `max`. Shared
/// by the API retry loop and the `listen` event/forward loops so both use one
/// backoff policy.
pub(crate) async fn backoff(attempt: u32, base: Duration, max: Duration) {
    let factor = 1u32 << attempt.min(16);
    let delay = base.saturating_mul(factor).min(max);
    tokio::time::sleep(delay).await;
}

/// Deterministic idempotency key for a mutating request the caller did not supply
/// one for. Seeded from method + path + serialized body, so the *same* logical
/// request (identical content) is stable across retries and de-duplicates safely,
/// while two *different* requests to the same endpoint get distinct keys.
fn default_idempotency_key(method: &Method, path: &str, body: &str) -> String {
    let namespace = Uuid::NAMESPACE_URL;
    let seed = format!("{method} {path} {body}");
    Uuid::new_v5(&namespace, seed.as_bytes()).to_string()
}

/// Reconstructs the backend's stored `key_prefix` for an API-key secret so
/// `revoke_token_by_prefix` can resolve a held token to its key `id`.
///
/// The backend mints `key_prefix = format!("{}...", &raw_secret[..18])` (see
/// `mcp/oauth.rs` and the api-keys creation path). Returns `None` for tokens too
/// short to carry an 18-char prefix (so a malformed token never matches).
fn token_revocation_prefix(token: &str) -> Option<String> {
    let token = token.trim();
    if token.len() < 18 || !token.is_char_boundary(18) {
        return None;
    }
    Some(format!("{}...", &token[..18]))
}

#[cfg(test)]
mod tests {
    use super::*;
    use reqwest::header::{ACCEPT, AUTHORIZATION, CONTENT_TYPE};

    #[test]
    fn constructs_domain_add_request() {
        let client = ApiClient::new("https://api.example.test/root", "dairo_test_123").unwrap();
        let request = client
            .build_request(
                Method::POST,
                &["v1", "domains"],
                Some(&CreateDomainRequest {
                    domain: "example.com".to_string(),
                }),
            )
            .unwrap();

        assert_eq!(request.method(), Method::POST);
        assert_eq!(
            request.url().as_str(),
            "https://api.example.test/root/v1/domains"
        );
        assert_eq!(
            request
                .headers()
                .get(AUTHORIZATION)
                .unwrap()
                .to_str()
                .unwrap(),
            "Bearer dairo_test_123"
        );
        assert_eq!(
            request.headers().get(ACCEPT).unwrap().to_str().unwrap(),
            "application/json"
        );
        assert!(request.headers().get("Idempotency-Key").is_some());
        assert_eq!(
            request
                .headers()
                .get(CONTENT_TYPE)
                .unwrap()
                .to_str()
                .unwrap(),
            "application/json"
        );
    }

    #[test]
    fn rejects_non_https_base_url_to_protect_api_key() {
        let error = ApiClient::new("http://api.dairo.app", "dairo_secret")
            .expect_err("plain http to a public host must be rejected");
        assert!(matches!(error, ApiError::InsecureBaseUrl(_)));
        // The error must never echo the API key.
        assert!(!error.to_string().contains("dairo_secret"));

        let error = ApiClient::new("ftp://api.dairo.app", "dairo_secret")
            .expect_err("non-http(s) schemes must be rejected");
        assert!(matches!(error, ApiError::InsecureBaseUrl(_)));
    }

    #[test]
    fn allows_http_only_for_loopback_hosts() {
        assert!(ApiClient::new("http://localhost:8787", "token").is_ok());
        assert!(ApiClient::new("http://127.0.0.1:8787", "token").is_ok());
        assert!(ApiClient::new("http://[::1]:8787", "token").is_ok());
        assert!(ApiClient::new("http://dev.localhost", "token").is_ok());
        // A non-loopback host over http is still rejected.
        assert!(ApiClient::new("http://example.com", "token").is_err());
    }

    #[test]
    fn debug_never_leaks_api_key() {
        let client = ApiClient::new("https://api.dairo.app", "dairo_super_secret").unwrap();
        let rendered = format!("{client:?}");
        assert!(!rendered.contains("dairo_super_secret"));
        assert!(rendered.contains("[REDACTED]"));
    }

    fn idempotency_key_of(req: &Request) -> String {
        req.headers()
            .get("Idempotency-Key")
            .unwrap()
            .to_str()
            .unwrap()
            .to_string()
    }

    #[test]
    fn idempotency_key_is_stable_across_retries_of_same_request() {
        // A per-invocation random key would defeat retry de-dup. Re-building the
        // same logical request (same method + path + body) must yield the same key.
        let client = ApiClient::new("https://api.example.test", "token").unwrap();
        let body = serde_json::json!({ "to": ["a@example.test"], "subject": "hi" });
        let first = client
            .build_request(Method::POST, &["v1", "emails"], Some(&body))
            .unwrap();
        let second = client
            .build_request(Method::POST, &["v1", "emails"], Some(&body))
            .unwrap();
        assert_eq!(idempotency_key_of(&first), idempotency_key_of(&second));
    }

    #[test]
    fn idempotency_key_differs_for_different_bodies_on_same_endpoint() {
        // Regression: two distinct `dairo send` emails must NOT collide on one key,
        // or the server de-dupes the second to the first and only one email sends.
        let client = ApiClient::new("https://api.example.test", "token").unwrap();
        let one = client
            .build_request(
                Method::POST,
                &["v1", "emails"],
                Some(&serde_json::json!({ "to": ["a@example.test"], "subject": "one" })),
            )
            .unwrap();
        let two = client
            .build_request(
                Method::POST,
                &["v1", "emails"],
                Some(&serde_json::json!({ "to": ["a@example.test"], "subject": "two" })),
            )
            .unwrap();
        assert_ne!(idempotency_key_of(&one), idempotency_key_of(&two));
    }

    #[test]
    fn caller_supplied_idempotency_key_is_used_verbatim() {
        let client = ApiClient::new("https://api.example.test", "token").unwrap();
        let request = client
            .build_request_with_idempotency(
                Method::POST,
                &["v1", "emails"],
                None::<&()>,
                Some("caller-key-123"),
            )
            .unwrap();
        assert_eq!(
            request
                .headers()
                .get("Idempotency-Key")
                .unwrap()
                .to_str()
                .unwrap(),
            "caller-key-123"
        );
    }

    #[test]
    fn attachment_link_targets_branded_link_route_not_url() {
        let client = ApiClient::new("https://api.example.test", "token").unwrap();
        // We cannot exercise the async network call here, but verify the path
        // wiring matches the backend's branded `/link` route.
        let request = client
            .build_request(
                Method::GET,
                &["v1", "attachments", "att_123", "link"],
                None::<&()>,
            )
            .unwrap();
        assert_eq!(
            request.url().as_str(),
            "https://api.example.test/v1/attachments/att_123/link"
        );
    }

    #[test]
    fn email_list_delete_targets_list_resource() {
        let client = ApiClient::new("https://api.example.test", "token").unwrap();
        let request = client
            .build_request(Method::DELETE, &["v1", "lists", "list_123"], None::<&()>)
            .unwrap();
        assert_eq!(request.method(), Method::DELETE);
        assert_eq!(
            request.url().as_str(),
            "https://api.example.test/v1/lists/list_123"
        );
    }

    #[test]
    fn encodes_path_segments() {
        let client = ApiClient::new("https://api.example.test", "token").unwrap();
        let request = client
            .build_request(
                Method::POST,
                &["v1", "domains", "weird domain.example", "verify"],
                None::<&()>,
            )
            .unwrap();

        assert_eq!(
            request.url().as_str(),
            "https://api.example.test/v1/domains/weird%20domain.example/verify"
        );
    }

    #[test]
    fn serializes_send_body_with_openapi_names() {
        let body = SendEmailRequest {
            inbox_id: "018f".to_string(),
            to: vec!["max@example.com".to_string()],
            cc: None,
            bcc: None,
            subject: "Hello".to_string(),
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
            reply_to: None,
            headers: None,
            tags: None,
        };

        let value = serde_json::to_value(body).unwrap();

        assert_eq!(value["inboxId"], "018f");
        assert_eq!(value["to"][0], "max@example.com");
        assert_eq!(value["subject"], "Hello");
        assert_eq!(value["text"], "Body");
        assert!(value.get("cc").is_none());
        assert!(value.get("react").is_none());
        assert_eq!(value["attachments"][0]["filename"], "invoice.pdf");
        assert_eq!(value["attachments"][0]["contentType"], "application/pdf");
        assert_eq!(value["attachments"][0]["contentBase64"], "JVBERi0xLjQ=");
    }

    #[test]
    fn serializes_send_body_with_hosted_react_source() {
        let body = SendEmailRequest {
            inbox_id: "018f".to_string(),
            to: vec!["max@example.com".to_string()],
            cc: None,
            bcc: None,
            subject: "Hello".to_string(),
            text: None,
            html: None,
            react: Some(SendEmailReact {
                source: "export default function Email(props) { return <p>{props.name}</p>; }"
                    .to_string(),
                props: Some(serde_json::Map::from_iter([(
                    "name".to_string(),
                    serde_json::Value::String("Max".to_string()),
                )])),
            }),
            attachments: None,
            idempotency_key: None,
            send_at: None,
            ignore_complaints: false,
            reply_to: None,
            headers: None,
            tags: None,
        };

        let value = serde_json::to_value(body).unwrap();

        assert!(value.get("text").is_none());
        assert!(value.get("html").is_none());
        assert_eq!(
            value["react"]["source"],
            "export default function Email(props) { return <p>{props.name}</p>; }"
        );
        assert_eq!(value["react"]["props"]["name"], "Max");
    }

    #[test]
    fn serializes_send_body_with_send_at_for_scheduling() {
        let body = SendEmailRequest {
            inbox_id: "018f".to_string(),
            to: vec!["max@example.com".to_string()],
            cc: None,
            bcc: None,
            subject: "Hello".to_string(),
            text: Some("Body".to_string()),
            html: None,
            react: None,
            attachments: None,
            idempotency_key: None,
            send_at: Some("2026-06-11T09:00:00Z".to_string()),
            ignore_complaints: false,
            reply_to: None,
            headers: None,
            tags: None,
        };

        let value = serde_json::to_value(body).unwrap();

        assert_eq!(value["sendAt"], "2026-06-11T09:00:00Z");
    }

    #[test]
    fn omits_send_at_when_sending_immediately() {
        let body = SendEmailRequest {
            inbox_id: "018f".to_string(),
            to: vec!["max@example.com".to_string()],
            cc: None,
            bcc: None,
            subject: "Hello".to_string(),
            text: Some("Body".to_string()),
            html: None,
            react: None,
            attachments: None,
            idempotency_key: None,
            send_at: None,
            ignore_complaints: false,
            reply_to: None,
            headers: None,
            tags: None,
        };

        let value = serde_json::to_value(body).unwrap();

        assert!(value.get("sendAt").is_none());
        // The new reply-to/headers/tags fields are omitted entirely when unset.
        assert!(value.get("replyTo").is_none());
        assert!(value.get("headers").is_none());
        assert!(value.get("tags").is_none());
    }

    #[test]
    fn serializes_reply_to_headers_and_tags_with_wire_names() {
        let mut headers = std::collections::BTreeMap::new();
        headers.insert("X-Campaign".to_string(), "spring".to_string());
        let mut tags = std::collections::BTreeMap::new();
        tags.insert("env".to_string(), "prod".to_string());
        let body = SendEmailRequest {
            inbox_id: "018f".to_string(),
            to: vec!["max@example.com".to_string()],
            cc: None,
            bcc: None,
            subject: "Hello".to_string(),
            text: Some("Body".to_string()),
            html: None,
            react: None,
            attachments: None,
            idempotency_key: None,
            send_at: None,
            ignore_complaints: false,
            reply_to: Some("support@dairo.app".to_string()),
            headers: Some(headers),
            tags: Some(tags),
        };

        let value = serde_json::to_value(body).unwrap();

        assert_eq!(value["replyTo"], "support@dairo.app");
        assert_eq!(value["headers"]["X-Campaign"], "spring");
        assert_eq!(value["tags"]["env"], "prod");
    }

    #[test]
    fn send_response_deserializes_scheduled_status_with_scheduled_at() {
        let response: SendEmailResponse = serde_json::from_str(
            r#"{
                "id": "email_123",
                "status": "scheduled",
                "providerMessageId": null,
                "error": null,
                "scheduledAt": "2026-06-11T09:00:00+00:00"
            }"#,
        )
        .unwrap();

        assert_eq!(response.status, "scheduled");
        assert_eq!(
            response.scheduled_at.as_deref(),
            Some("2026-06-11T09:00:00+00:00")
        );
    }

    #[test]
    fn serializes_create_api_key_body_with_allowed_ips() {
        let body = CreateApiKeyRequest {
            name: "CI".to_string(),
            scopes: vec!["mail:send".to_string()],
            allowed_ips: Some(vec![
                "203.0.113.0/24".to_string(),
                "198.51.100.7".to_string(),
            ]),
        };
        let value = serde_json::to_value(body).unwrap();
        assert_eq!(value["allowedIps"][0], "203.0.113.0/24");
        assert_eq!(value["allowedIps"][1], "198.51.100.7");
    }

    #[test]
    fn omits_allowed_ips_when_unset() {
        let body = CreateApiKeyRequest {
            name: "CI".to_string(),
            scopes: vec!["mail:send".to_string()],
            allowed_ips: None,
        };
        let value = serde_json::to_value(body).unwrap();
        assert!(value.get("allowedIps").is_none());
    }

    #[test]
    fn deserializes_api_key_allowed_ips_and_defaults_to_empty() {
        // The live API now returns the unified list envelope: rows live under
        // `data`, each carrying its `object: "api_key"` discriminator (ignored).
        let response: ListEnvelope<ApiKey> = serde_json::from_value(serde_json::json!({
            "object": "list",
            "data": [
                {
                    "object": "api_key",
                    "id": "key_1",
                    "name": "scoped",
                    "prefix": "dairo_test_a",
                    "environment": "test",
                    "scopes": ["mail:send"],
                    "allowedIps": ["203.0.113.0/24"],
                    "status": "active",
                    "createdAt": "2026-06-01T00:00:00Z",
                    "lastUsedAt": null
                },
                {
                    "object": "api_key",
                    "id": "key_2",
                    "name": "open",
                    "prefix": "dairo_test_b",
                    "environment": "test",
                    "scopes": ["mail:read"],
                    "status": "active",
                    "createdAt": "2026-06-01T00:00:00Z",
                    "lastUsedAt": null
                }
            ],
            "pagination": { "nextCursor": null, "hasMore": false }
        }))
        .unwrap();

        assert_eq!(response.data[0].allowed_ips, vec!["203.0.113.0/24"]);
        assert!(response.data[1].allowed_ips.is_empty());
    }

    #[test]
    fn token_revocation_prefix_matches_backend_key_prefix() {
        // The backend stores `format!("{}...", &raw_secret[..18])`. Reconstruct it.
        let token = "dairo_live_0123456789abcdef0123456789abcdef";
        assert_eq!(
            token_revocation_prefix(token).as_deref(),
            Some("dairo_live_0123456...")
        );
        // A short or empty token cannot be resolved and never matches a key.
        assert_eq!(token_revocation_prefix("short"), None);
        assert_eq!(token_revocation_prefix(""), None);
    }

    #[test]
    fn cancel_outbound_email_targets_cancel_route() {
        let client = ApiClient::new("https://api.example.test", "token").unwrap();
        let request = client
            .build_request(
                Method::POST,
                &["v1", "emails", "email_123", "cancel"],
                None::<&()>,
            )
            .unwrap();
        assert_eq!(request.method(), Method::POST);
        assert_eq!(
            request.url().as_str(),
            "https://api.example.test/v1/emails/email_123/cancel"
        );
    }

    #[test]
    fn send_response_accepts_legacy_payload_without_warnings() {
        let response: SendEmailResponse = serde_json::from_str(
            r#"{
                "id": "email_123",
                "status": "queued",
                "providerMessageId": null,
                "error": null
            }"#,
        )
        .unwrap();

        assert_eq!(response.id, "email_123");
        assert!(response.warnings.is_empty());
    }

    #[test]
    fn send_response_deserializes_complaint_warning_metadata() {
        let response: SendEmailResponse = serde_json::from_str(
            r#"{
                "id": "email_123",
                "status": "queued",
                "providerMessageId": "ses_message_123",
                "error": null,
                "warnings": [
                    {
                        "recipient": "max@example.com",
                        "reason": "complaint",
                        "message": "Recipient previously complained; do not contact again unless you are sure.",
                        "sourceOutboundEmailId": "email_old",
                        "providerMessageId": "ses_old",
                        "complaintFeedbackType": "abuse",
                        "complaintUserAgent": "AnyMailbox/1.0",
                        "lastEventAt": "2026-06-02T10:00:00Z"
                    }
                ]
            }"#,
        )
        .unwrap();

        let warning = response.warnings.first().unwrap();
        assert_eq!(warning.recipient.as_deref(), Some("max@example.com"));
        assert_eq!(warning.reason.as_deref(), Some("complaint"));
        assert_eq!(
            warning.source_outbound_email_id.as_deref(),
            Some("email_old")
        );
        assert_eq!(warning.provider_message_id.as_deref(), Some("ses_old"));
        assert_eq!(warning.complaint_feedback_type.as_deref(), Some("abuse"));
        assert_eq!(
            warning.complaint_user_agent.as_deref(),
            Some("AnyMailbox/1.0")
        );
        assert_eq!(
            warning.last_event_at.as_deref(),
            Some("2026-06-02T10:00:00Z")
        );
    }

    #[test]
    fn constructs_webhook_and_api_key_requests() {
        let client = ApiClient::new("https://api.example.test", "token").unwrap();
        let webhook = client
            .build_request(
                Method::DELETE,
                &["v1", "webhooks", "https://example.com/hook"],
                None::<&()>,
            )
            .unwrap();
        assert_eq!(
            webhook.url().as_str(),
            "https://api.example.test/v1/webhooks/https:%2F%2Fexample.com%2Fhook"
        );

        let api_key = client
            .build_request(Method::DELETE, &["v1", "api-keys", "key_123"], None::<&()>)
            .unwrap();
        assert_eq!(
            api_key.url().as_str(),
            "https://api.example.test/v1/api-keys/key_123"
        );
    }

    #[test]
    fn deserializes_webhook_delivery_state_without_secret_hash() {
        // Unified list envelope: webhooks live under `data`.
        let response: ListEnvelope<Webhook> = serde_json::from_value(serde_json::json!({
            "object": "list",
            "data": [
                {
                    "object": "webhook",
                    "id": "wh_123",
                    "url": "https://example.com/hook",
                    "events": ["message.received", "email.delivered"],
                    "status": "active",
                    "createdAt": "2026-06-01T00:00:00Z",
                    "lastDeliveryAt": "2026-06-02T10:00:00Z"
                }
            ],
            "pagination": { "nextCursor": null, "hasMore": false }
        }))
        .unwrap();

        let webhook = &response.data[0];
        assert_eq!(webhook.events[0], "message.received");
        assert_eq!(
            webhook.last_delivery_at.as_deref(),
            Some("2026-06-02T10:00:00Z")
        );
    }

    #[test]
    fn deserializes_message_body_fields() {
        let message: Message = serde_json::from_value(serde_json::json!({
            "id": "msg_123",
            "inboxId": "inbox_123",
            "threadId": null,
            "direction": "inbound",
            "status": "received",
            "from": { "address": "sender@example.com", "name": null },
            "to": ["test@dairo.app"],
            "subject": "Hello",
            "textPreview": "Body preview",
            "textBody": "Full plain body",
            "htmlBody": "<p>Full html body</p>",
            "hasHtml": true,
            "hasAttachments": false,
            "receivedAt": "2026-06-01T00:00:00Z",
            "createdAt": "2026-06-01T00:00:00Z",
            "attachments": []
        }))
        .unwrap();

        assert_eq!(message.text_body.as_deref(), Some("Full plain body"));
        assert_eq!(message.html_body.as_deref(), Some("<p>Full html body</p>"));
    }

    #[test]
    fn events_query_serializes_tail_and_wait_and_filters() {
        let client = ApiClient::new("https://api.example.test", "token").unwrap();
        let mut request = client
            .build_request(Method::GET, &["v1", "events"], None::<&()>)
            .unwrap();
        apply_events_query(
            request.url_mut(),
            &EventsQuery {
                since: Some("cursor_abc".to_string()),
                limit: Some(50),
                inbox_id: Some("inbox_123".to_string()),
                event_type: Some("message.received".to_string()),
                wait: Some(25),
                tail: false,
            },
        );
        let query = request.url().query().unwrap();
        assert!(query.contains("since=cursor_abc"));
        assert!(query.contains("limit=50"));
        assert!(query.contains("inboxId=inbox_123"));
        assert!(query.contains("type=message.received"));
        assert!(query.contains("wait=25"));
        assert!(!query.contains("tail="));
    }

    #[test]
    fn events_query_tail_sets_only_tail_param() {
        let client = ApiClient::new("https://api.example.test", "token").unwrap();
        let mut request = client
            .build_request(Method::GET, &["v1", "events"], None::<&()>)
            .unwrap();
        apply_events_query(
            request.url_mut(),
            &EventsQuery {
                tail: true,
                limit: Some(1),
                ..Default::default()
            },
        );
        let query = request.url().query().unwrap();
        assert!(query.contains("tail=true"));
        assert!(query.contains("limit=1"));
        assert!(!query.contains("since="));
        assert!(!query.contains("wait="));
    }

    #[test]
    fn events_request_timeout_tracks_wait_plus_margin() {
        // A 25s long-poll hang must fit inside the request timeout (wait + 5s).
        assert_eq!(events_request_timeout(Some(25)), Duration::from_secs(30));
        // tail / immediate (no wait) still gets the 5s floor, not the 30s default.
        assert_eq!(events_request_timeout(None), Duration::from_secs(5));
        assert_eq!(events_request_timeout(Some(0)), Duration::from_secs(5));
    }

    #[test]
    fn deserializes_events_response_with_ledger_rows_and_gaps() {
        let response: EventsResponse = serde_json::from_value(serde_json::json!({
            "events": [
                {
                    "eventId": "evt_1",
                    "type": "message.received",
                    "seq": 7,
                    "partitionKey": "inbox:abc",
                    "inboxId": "abc",
                    "threadId": "thread_1",
                    "idempotencyKey": "idem-1",
                    "outboundEmailId": null,
                    "messageId": "msg_1",
                    "providerMessageId": null,
                    "occurredAt": "2026-06-11T00:00:00Z",
                    "createdAt": "2026-06-11T00:00:01Z",
                    "data": { "from": "sender@example.com", "subject": "Hi" }
                }
            ],
            "pagination": { "nextCursor": "cursor_xyz", "hasMore": true },
            "gaps": [ { "partitionKey": "inbox:abc", "missingSeq": [5, 6] } ]
        }))
        .unwrap();

        assert_eq!(response.events.len(), 1);
        let event = &response.events[0];
        assert_eq!(event.event_id, "evt_1");
        assert_eq!(event.event_type, "message.received");
        assert_eq!(event.seq, Some(7));
        assert_eq!(event.message_id.as_deref(), Some("msg_1"));
        assert_eq!(event.data["subject"], "Hi");
        assert_eq!(
            response.pagination.next_cursor.as_deref(),
            Some("cursor_xyz")
        );
        assert!(response.pagination.has_more);
        assert_eq!(response.gaps.len(), 1);
        assert_eq!(response.gaps[0]["partitionKey"], "inbox:abc");
    }

    #[test]
    fn deserializes_tail_response_with_empty_events() {
        // The tail bootstrap returns events:[] plus the head cursor.
        let response: EventsResponse = serde_json::from_value(serde_json::json!({
            "events": [],
            "pagination": { "nextCursor": "cursor_head", "hasMore": false },
            "gaps": []
        }))
        .unwrap();
        assert!(response.events.is_empty());
        assert_eq!(
            response.pagination.next_cursor.as_deref(),
            Some("cursor_head")
        );
        assert!(!response.pagination.has_more);
    }

    #[test]
    fn deserializes_ledger_event_without_optional_join_keys() {
        // Outbound/delivery events omit message-specific keys; those must default
        // to None rather than fail deserialization.
        let event: LedgerEvent = serde_json::from_value(serde_json::json!({
            "eventId": "evt_2",
            "type": "email.delivered"
        }))
        .unwrap();
        assert_eq!(event.event_id, "evt_2");
        assert_eq!(event.event_type, "email.delivered");
        assert_eq!(event.message_id, None);
        assert_eq!(event.inbox_id, None);
        assert_eq!(event.seq, None);
        assert!(event.data.is_null());
    }

    #[test]
    fn template_version_targets_versioned_route() {
        let client = ApiClient::new("https://api.example.test", "token").unwrap();
        let request = client
            .build_request(
                Method::GET,
                &["v1", "templates", "welcome", "versions", "3"],
                None::<&()>,
            )
            .unwrap();
        assert_eq!(
            request.url().as_str(),
            "https://api.example.test/v1/templates/welcome/versions/3"
        );
    }

    #[test]
    fn update_template_uses_patch_verb() {
        let client = ApiClient::new("https://api.example.test", "token").unwrap();
        let request = client
            .build_request(
                Method::PATCH,
                &["v1", "templates", "tmpl_123"],
                Some(&serde_json::json!({ "name": "Renamed" })),
            )
            .unwrap();
        assert_eq!(request.method(), Method::PATCH);
        assert_eq!(
            request.url().as_str(),
            "https://api.example.test/v1/templates/tmpl_123"
        );
        // Mutating verbs carry an idempotency key (PATCH is included).
        assert!(request.headers().get("Idempotency-Key").is_some());
    }

    #[test]
    fn inbox_schema_targets_schema_subresource() {
        let client = ApiClient::new("https://api.example.test", "token").unwrap();
        let request = client
            .build_request(
                Method::PUT,
                &["v1", "inboxes", "agent@example.com", "schema"],
                Some(&serde_json::json!({ "schema": {} })),
            )
            .unwrap();
        assert_eq!(request.method(), Method::PUT);
        assert_eq!(
            request.url().as_str(),
            "https://api.example.test/v1/inboxes/agent@example.com/schema"
        );
    }

    #[test]
    fn verification_wait_targets_wait_subresource() {
        let client = ApiClient::new("https://api.example.test", "token").unwrap();
        let request = client
            .build_request(
                Method::DELETE,
                &[
                    "v1",
                    "inboxes",
                    "inbox_123",
                    "verification-waits",
                    "wait_123",
                ],
                None::<&()>,
            )
            .unwrap();
        assert_eq!(request.method(), Method::DELETE);
        assert_eq!(
            request.url().as_str(),
            "https://api.example.test/v1/inboxes/inbox_123/verification-waits/wait_123"
        );
    }

    #[test]
    fn events_replay_targets_replay_route() {
        let client = ApiClient::new("https://api.example.test", "token").unwrap();
        let request = client
            .build_request(
                Method::POST,
                &["v1", "events", "replay"],
                Some(&serde_json::json!({ "since": "cursor_abc" })),
            )
            .unwrap();
        assert_eq!(request.method(), Method::POST);
        assert_eq!(
            request.url().as_str(),
            "https://api.example.test/v1/events/replay"
        );
    }

    #[test]
    fn set_budget_uses_put_verb() {
        let client = ApiClient::new("https://api.example.test", "token").unwrap();
        let request = client
            .build_request(
                Method::PUT,
                &["v1", "budgets"],
                Some(&serde_json::json!({ "scope": "account" })),
            )
            .unwrap();
        assert_eq!(request.method(), Method::PUT);
        assert_eq!(
            request.url().as_str(),
            "https://api.example.test/v1/budgets"
        );
    }

    #[test]
    fn erasure_job_targets_erasure_jobs_route() {
        let client = ApiClient::new("https://api.example.test", "token").unwrap();
        let request = client
            .build_request(Method::GET, &["v1", "erasure-jobs", "job_123"], None::<&()>)
            .unwrap();
        assert_eq!(
            request.url().as_str(),
            "https://api.example.test/v1/erasure-jobs/job_123"
        );
    }

    #[test]
    fn a2a_message_targets_messages_route() {
        let client = ApiClient::new("https://api.example.test", "token").unwrap();
        // A2A single hops fold into the messages surface (an a2a id resolves on
        // GET /v1/messages/{id}); the private /v1/a2a acronym is gone.
        let request = client
            .build_request(Method::GET, &["v1", "messages", "a2a_123"], None::<&()>)
            .unwrap();
        assert_eq!(
            request.url().as_str(),
            "https://api.example.test/v1/messages/a2a_123"
        );
    }

    #[test]
    fn a2a_query_serializes_channel_pagination_and_inbox_filter() {
        let client = ApiClient::new("https://api.example.test", "token").unwrap();
        // The A2A list folds into GET /v1/messages?channel=a2a.
        let mut request = client
            .build_request(Method::GET, &["v1", "messages"], None::<&()>)
            .unwrap();
        {
            let mut pairs = request.url_mut().query_pairs_mut();
            pairs.append_pair("channel", "a2a");
            let query = A2aMessageQuery {
                limit: Some(25),
                cursor: Some("cursor_abc".to_string()),
                inbox_id: Some("inbox_123".to_string()),
            };
            if let Some(limit) = query.limit {
                pairs.append_pair("limit", &limit.to_string());
            }
            if let Some(cursor) = &query.cursor {
                pairs.append_pair("cursor", cursor);
            }
            if let Some(inbox_id) = &query.inbox_id {
                pairs.append_pair("inboxId", inbox_id);
            }
        }
        let query = request.url().query().unwrap();
        assert!(query.contains("channel=a2a"));
        assert!(query.contains("limit=25"));
        assert!(query.contains("cursor=cursor_abc"));
        assert!(query.contains("inboxId=inbox_123"));
    }

    #[test]
    fn verify_query_by_message_id_sends_only_id() {
        let client = ApiClient::new("https://api.example.test", "token").unwrap();
        let mut request = client
            .build_request(Method::GET, &["v1", "agents", "verify"], None::<&()>)
            .unwrap();
        apply_verify_query(
            request.url_mut(),
            &VerifyAgentQuery {
                id: Some("msg_123".to_string()),
                ..Default::default()
            },
        );
        let query = request.url().query().unwrap();
        assert_eq!(query, "id=msg_123");
    }

    #[test]
    fn verify_query_by_signature_sends_all_present_fields() {
        let client = ApiClient::new("https://api.example.test", "token").unwrap();
        let mut request = client
            .build_request(Method::GET, &["v1", "agents", "verify"], None::<&()>)
            .unwrap();
        apply_verify_query(
            request.url_mut(),
            &VerifyAgentQuery {
                agent: Some("agt_abc".to_string()),
                kid: Some("kid_1".to_string()),
                sig: Some("deadbeef".to_string()),
                to: Some("a@example.com,b@example.com".to_string()),
                ..Default::default()
            },
        );
        let query = request.url().query().unwrap();
        assert!(query.contains("agent=agt_abc"));
        assert!(query.contains("kid=kid_1"));
        assert!(query.contains("sig=deadbeef"));
        assert!(query.contains("to=a%40example.com%2Cb%40example.com"));
        // The message-id form must not be sent in the signature form. Guard
        // against `kid=` matching `id=` by checking the param boundary.
        assert!(!query.split('&').any(|pair| pair.starts_with("id=")));
        assert!(!query.contains("from="));
    }

    #[test]
    fn secret_response_debug_is_redacted() {
        let webhook = CreateWebhookResponse {
            webhook: Webhook {
                id: "wh_123".to_string(),
                url: "https://example.com/hook".to_string(),
                events: vec!["message.received".to_string()],
                status: "active".to_string(),
                created_at: "2026-01-01T00:00:00Z".to_string(),
                last_delivery_at: None,
            },
            secret: "whsec_real_secret".to_string(),
        };
        assert!(!format!("{webhook:?}").contains("whsec_real_secret"));

        let api_key = CreateApiKeyResponse {
            api_key: ApiKey {
                id: "key_123".to_string(),
                name: "CI".to_string(),
                prefix: "dairo_test_abc".to_string(),
                scopes: vec!["mail:send".to_string()],
                allowed_ips: Vec::new(),
                status: "active".to_string(),
                created_at: "2026-01-01T00:00:00Z".to_string(),
                last_used_at: None,
            },
            secret: "dairo_real_secret".to_string(),
        };
        assert!(!format!("{api_key:?}").contains("dairo_real_secret"));
    }

    // --- Letters (physical-mail surface) ----------------------------

    #[test]
    fn serializes_create_letter_body_with_openapi_names() {
        let body = CreateLetterRequest {
            pdf_base64: Some("JVBERi0xLjQ=".to_string()),
            file: None,
            file_name: "invoice.pdf".to_string(),
            to: PostalAddress {
                name: Some("Jane Doe".to_string()),
                street: Some("Hauptstrasse".to_string()),
                house_number: Some("12".to_string()),
                postal_code: Some("8001".to_string()),
                city: Some("Zürich".to_string()),
                country: "CH".to_string(),
                ..Default::default()
            },
            from: None,
            template_id: None,
            print: Some(LetterPrintOptions {
                mode: Some("grayscale".to_string()),
                sides: Some("duplex".to_string()),
                address_placement: Some("left".to_string()),
            }),
            delivery: Some("priority".to_string()),
            payment_slip: Some("sepaDe".to_string()),
            payment: None,
            notifications: Some(true),
            auto_send: Some(false),
            metadata: Some(serde_json::json!({ "invoiceId": "inv_123" })),
        };

        let value = serde_json::to_value(body).unwrap();

        // Wire field names match the canonical OpenAPI (camelCase).
        assert_eq!(value["pdfBase64"], "JVBERi0xLjQ=");
        assert_eq!(value["fileName"], "invoice.pdf");
        assert_eq!(value["to"]["name"], "Jane Doe");
        assert_eq!(value["to"]["houseNumber"], "12");
        assert_eq!(value["to"]["postalCode"], "8001");
        assert_eq!(value["to"]["country"], "CH");
        assert_eq!(value["print"]["mode"], "grayscale");
        assert_eq!(value["print"]["sides"], "duplex");
        assert_eq!(value["print"]["addressPlacement"], "left");
        assert_eq!(value["delivery"], "priority");
        // The payment slip is the camelCase public token; notifications opt-in
        // is sent as a bool.
        assert_eq!(value["paymentSlip"], "sepaDe");
        assert_eq!(value["notifications"], true);
        // A draft must send autoSend=false explicitly.
        assert_eq!(value["autoSend"], false);
        assert_eq!(value["metadata"]["invoiceId"], "inv_123");
        // Unset optionals are omitted entirely.
        assert!(value.get("file").is_none());
        assert!(value.get("from").is_none());
        assert!(value.get("templateId").is_none());
        assert!(value.get("payment").is_none());
    }

    #[test]
    fn serializes_create_letter_payment_object_with_openapi_names() {
        let body = CreateLetterRequest {
            pdf_base64: None,
            file: None,
            file_name: "invoice.pdf".to_string(),
            to: PostalAddress {
                name: Some("Jane Doe".to_string()),
                street: Some("Hauptstrasse".to_string()),
                house_number: Some("12".to_string()),
                postal_code: Some("8001".to_string()),
                city: Some("Zürich".to_string()),
                country: "CH".to_string(),
                ..Default::default()
            },
            from: None,
            template_id: Some("tmpl_invoice".to_string()),
            print: None,
            delivery: None,
            // The structured payment object also sets the bare flag from its type.
            payment_slip: Some("qr".to_string()),
            payment: Some(LetterPayment {
                payment_type: "qr".to_string(),
                creditor: LetterCreditor {
                    name: "Acme AG".to_string(),
                    iban: "CH9300762011623852957".to_string(),
                    bic: None,
                    street: Some("Bahnhofstrasse".to_string()),
                    house_number: Some("1".to_string()),
                    postal_code: Some("8001".to_string()),
                    city: Some("Zürich".to_string()),
                    country: "CH".to_string(),
                },
                amount: 49.90,
                currency: "CHF".to_string(),
                reference: Some("210000000003139471430009017".to_string()),
                message: Some("Invoice inv_123".to_string()),
                debtor: Some(LetterDebtor {
                    name: "Jane Doe".to_string(),
                    street: Some("Hauptstrasse".to_string()),
                    house_number: Some("12".to_string()),
                    postal_code: Some("8001".to_string()),
                    city: Some("Zürich".to_string()),
                    country: "CH".to_string(),
                }),
            }),
            notifications: None,
            auto_send: Some(false),
            metadata: None,
        };

        let value = serde_json::to_value(body).unwrap();

        assert_eq!(value["templateId"], "tmpl_invoice");
        // The structured slip kind is sent under `type`; the bare flag mirrors it.
        assert_eq!(value["payment"]["type"], "qr");
        assert_eq!(value["paymentSlip"], "qr");
        assert_eq!(value["payment"]["creditor"]["name"], "Acme AG");
        assert_eq!(
            value["payment"]["creditor"]["iban"],
            "CH9300762011623852957"
        );
        assert_eq!(value["payment"]["creditor"]["houseNumber"], "1");
        assert_eq!(value["payment"]["creditor"]["postalCode"], "8001");
        assert_eq!(value["payment"]["creditor"]["country"], "CH");
        assert_eq!(value["payment"]["amount"], 49.90);
        assert_eq!(value["payment"]["currency"], "CHF");
        assert_eq!(value["payment"]["message"], "Invoice inv_123");
        assert_eq!(value["payment"]["debtor"]["name"], "Jane Doe");
        assert_eq!(value["payment"]["debtor"]["postalCode"], "8001");
        // Unset creditor BIC is omitted entirely (camelCase omit-when-None).
        assert!(value["payment"]["creditor"].get("bic").is_none());
        // No inline PDF on the Dairo-render path.
        assert!(value.get("pdfBase64").is_none());
    }

    #[test]
    fn create_letter_omits_autosend_when_confirmed_and_serializes_file_ref() {
        let body = CreateLetterRequest {
            pdf_base64: None,
            file: Some(LetterFileRef {
                attachment_id: "att_9f2c".to_string(),
                message_id: Some("msg_abc".to_string()),
            }),
            file_name: "statement.pdf".to_string(),
            to: PostalAddress {
                street: Some("Main St".to_string()),
                country: "US".to_string(),
                ..Default::default()
            },
            from: None,
            template_id: None,
            print: None,
            delivery: None,
            payment_slip: None,
            payment: None,
            notifications: None,
            // A confirmed send omits autoSend so the server applies its `true`
            // default; only the draft path sends `false`.
            auto_send: None,
            metadata: None,
        };

        let value = serde_json::to_value(body).unwrap();

        assert!(value.get("autoSend").is_none());
        assert!(value.get("pdfBase64").is_none());
        assert!(value.get("print").is_none());
        assert!(value.get("delivery").is_none());
        // Unset payment-slip / payment / notifications are omitted entirely.
        assert!(value.get("paymentSlip").is_none());
        assert!(value.get("payment").is_none());
        assert!(value.get("templateId").is_none());
        assert!(value.get("notifications").is_none());
        assert_eq!(value["file"]["attachmentId"], "att_9f2c");
        assert_eq!(value["file"]["messageId"], "msg_abc");
        assert_eq!(value["fileName"], "statement.pdf");
    }

    #[test]
    fn serializes_letter_price_body_with_openapi_names() {
        let body = LetterPriceRequest {
            country: "CH".to_string(),
            page_count: Some(3),
            pdf_base64: None,
            print: Some(LetterPrintOptions {
                mode: Some("grayscale".to_string()),
                sides: Some("duplex".to_string()),
                address_placement: None,
            }),
            delivery: Some("economy".to_string()),
            paper_types: Some(vec!["standard".to_string(), "qr".to_string()]),
        };

        let value = serde_json::to_value(body).unwrap();

        assert_eq!(value["country"], "CH");
        assert_eq!(value["pageCount"], 3);
        assert_eq!(value["print"]["mode"], "grayscale");
        assert_eq!(value["delivery"], "economy");
        assert_eq!(value["paperTypes"][0], "standard");
        assert_eq!(value["paperTypes"][1], "qr");
        assert!(value.get("pdfBase64").is_none());
        // An empty paper-types vec would still serialize; the CLI sends None so
        // the field is absent. Verify the omitted-when-None contract holds here.
    }

    #[test]
    fn create_letter_targets_letters_collection_with_idempotency_key() {
        let client = ApiClient::new("https://api.example.test", "token").unwrap();
        let body = CreateLetterRequest {
            pdf_base64: Some("JVBERi0xLjQ=".to_string()),
            file: None,
            file_name: "invoice.pdf".to_string(),
            to: PostalAddress {
                street: Some("Main St".to_string()),
                country: "US".to_string(),
                ..Default::default()
            },
            from: None,
            template_id: None,
            print: None,
            delivery: None,
            payment_slip: None,
            payment: None,
            notifications: None,
            auto_send: Some(false),
            metadata: None,
        };
        let request = client
            .build_request(Method::POST, &["v1", "letters"], Some(&body))
            .unwrap();
        assert_eq!(request.method(), Method::POST);
        assert_eq!(
            request.url().as_str(),
            "https://api.example.test/v1/letters"
        );
        // Mutating verbs carry a default Idempotency-Key (body-inclusive).
        assert!(request.headers().get("Idempotency-Key").is_some());
    }

    #[test]
    fn cancel_letter_targets_cancel_subresource() {
        let client = ApiClient::new("https://api.example.test", "token").unwrap();
        let request = client
            .build_request(
                Method::POST,
                &["v1", "letters", "let_123", "cancel"],
                Some(&serde_json::json!({})),
            )
            .unwrap();
        assert_eq!(request.method(), Method::POST);
        assert_eq!(
            request.url().as_str(),
            "https://api.example.test/v1/letters/let_123/cancel"
        );
    }

    #[test]
    fn letter_events_targets_events_subresource() {
        let client = ApiClient::new("https://api.example.test", "token").unwrap();
        let request = client
            .build_request(
                Method::GET,
                &["v1", "letters", "let_123", "events"],
                None::<&()>,
            )
            .unwrap();
        assert_eq!(
            request.url().as_str(),
            "https://api.example.test/v1/letters/let_123/events"
        );
    }

    #[test]
    fn price_letter_targets_price_subresource() {
        let client = ApiClient::new("https://api.example.test", "token").unwrap();
        let request = client
            .build_request(
                Method::POST,
                &["v1", "letters", "price"],
                Some(&serde_json::json!({ "country": "CH" })),
            )
            .unwrap();
        assert_eq!(
            request.url().as_str(),
            "https://api.example.test/v1/letters/price"
        );
    }

    #[test]
    fn applies_letter_list_query_filters() {
        let client = ApiClient::new("https://api.example.test", "token").unwrap();
        let mut request = client
            .build_request(Method::GET, &["v1", "letters"], None::<&()>)
            .unwrap();
        apply_letter_query(
            request.url_mut(),
            &LetterListQuery {
                limit: Some(20),
                cursor: Some("cur_1".to_string()),
                status: Some("in_transit".to_string()),
                country: Some("CH".to_string()),
            },
        );
        let query = request.url().query().unwrap();
        assert!(query.contains("limit=20"));
        assert!(query.contains("cursor=cur_1"));
        assert!(query.contains("status=in_transit"));
        assert!(query.contains("country=CH"));
    }

    #[test]
    fn multipart_initiate_targets_objects_multipart_route() {
        // Regression: the initiate must hit `/objects/multipart`, not the
        // single-PUT `/objects`, and serialize the documented field names.
        let client = ApiClient::new("https://api.example.test", "token").unwrap();
        let request = client
            .build_request(
                Method::POST,
                &["v1", "buckets", "buk_1", "objects", "multipart"],
                Some(&InitiateMultipartRequest {
                    filename: "big.bin".to_string(),
                    content_type: "application/octet-stream".to_string(),
                    total_bytes: 700 * 1024 * 1024,
                    part_size: None,
                }),
            )
            .unwrap();
        assert_eq!(request.method(), Method::POST);
        assert_eq!(
            request.url().as_str(),
            "https://api.example.test/v1/buckets/buk_1/objects/multipart"
        );
        let body = std::str::from_utf8(request.body().unwrap().as_bytes().unwrap()).unwrap();
        assert!(body.contains("\"totalBytes\":734003200"), "body: {body}");
        assert!(body.contains("\"contentType\":\"application/octet-stream\""));
        // partSize is None -> omitted so the backend default (256MiB) applies.
        assert!(!body.contains("partSize"), "partSize must be omitted: {body}");
    }

    #[test]
    fn multipart_complete_and_abort_target_uploadid_subroutes() {
        let client = ApiClient::new("https://api.example.test", "token").unwrap();
        let complete = client
            .build_request(
                Method::POST,
                &[
                    "v1", "buckets", "buk_1", "objects", "multipart", "up_9", "complete",
                ],
                Some(&CompleteMultipartRequest {
                    object_id: "obj_1".to_string(),
                    parts: vec![CompletedPart {
                        part_number: 1,
                        etag: "\"abc\"".to_string(),
                    }],
                }),
            )
            .unwrap();
        assert_eq!(
            complete.url().as_str(),
            "https://api.example.test/v1/buckets/buk_1/objects/multipart/up_9/complete"
        );
        let body = std::str::from_utf8(complete.body().unwrap().as_bytes().unwrap()).unwrap();
        assert!(body.contains("\"partNumber\":1"));
        assert!(body.contains("\"objectId\":\"obj_1\""));

        let abort = client
            .build_request(
                Method::POST,
                &[
                    "v1", "buckets", "buk_1", "objects", "multipart", "up_9", "abort",
                ],
                Some(&AbortMultipartRequest {
                    object_id: "obj_1".to_string(),
                }),
            )
            .unwrap();
        assert_eq!(
            abort.url().as_str(),
            "https://api.example.test/v1/buckets/buk_1/objects/multipart/up_9/abort"
        );
    }

    #[test]
    fn multipart_initiate_response_deserializes_branded_part_urls() {
        // The initiate response carries the per-part BRANDED storage.dairo.app
        // urls plus the headers echoed on every part PUT.
        let raw = serde_json::json!({
            "object": "bucket_multipart_upload",
            "uploadId": "up_9",
            "objectId": "obj_1",
            "bucketId": "buk_1",
            "key": "uploads/u/buk_1/uuid",
            "method": "PUT",
            "partSize": 268435456,
            "partCount": 2,
            "headers": { "x-amz-content-sha256": "UNSIGNED-PAYLOAD" },
            "parts": [
                { "partNumber": 1, "url": "https://storage.dairo.app/u/k?partNumber=1&uploadId=up_9" },
                { "partNumber": 2, "url": "https://storage.dairo.app/u/k?partNumber=2&uploadId=up_9" }
            ],
            "oneTime": true,
            "expiresInSeconds": 3600,
            "downloadUrl": "https://storage.dairo.app/d/tok/big.bin",
            "shareUrl": "https://storage.dairo.app/s/tok",
            "linkExpiresInSeconds": 3600
        });
        let parsed: InitiateMultipartResponse = serde_json::from_value(raw).unwrap();
        assert_eq!(parsed.upload_id, "up_9");
        assert_eq!(parsed.part_count, 2);
        assert_eq!(parsed.part_size, 268435456);
        assert_eq!(parsed.parts.len(), 2);
        assert_eq!(parsed.parts[1].part_number, 2);
        assert!(parsed.parts[0].url.starts_with("https://storage.dairo.app/u/"));
        assert_eq!(
            parsed.headers.get("x-amz-content-sha256").map(String::as_str),
            Some("UNSIGNED-PAYLOAD")
        );
    }

    #[test]
    fn multipart_threshold_is_64_mib() {
        // The auto-select boundary: <= threshold single-PUT, strictly above
        // multipart. Pin the documented 64MiB so a refactor can't silently move
        // it (which would change which uploads go resumable).
        assert_eq!(MULTIPART_THRESHOLD_BYTES, 64 * 1024 * 1024);
    }
}
