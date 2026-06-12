use reqwest::{Method, Request, StatusCode};
use serde::{Deserialize, Serialize};
use std::time::Duration;
use thiserror::Error;
use url::Url;
use uuid::Uuid;

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
            let status = response.status();
            let message = match response.json::<ErrorResponse>().await {
                Ok(error) => error.error.display_message(),
                Err(_) => status
                    .canonical_reason()
                    .unwrap_or("unexpected API error")
                    .to_string(),
            };
            return Err(ApiError::Api { status, message });
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
    /// `mail:send`). The source is dry-rendered at publish.
    pub async fn create_template(&self, body: &serde_json::Value) -> Result<serde_json::Value> {
        self.execute_json(self.build_request(Method::POST, &["v1", "templates"], Some(body))?)
            .await
    }

    /// Gets a template plus a resolved version including its `source` (`GET
    /// /v1/templates/{idOrSlug}`, scope `mail:read`). `version` pins a specific
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
    /// /v1/templates/{idOrSlug}`, scope `mail:send`). The source is immutable.
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

    /// Archives a template (`DELETE /v1/templates/{idOrSlug}`, scope `mail:send`).
    pub async fn delete_template(&self, id_or_slug: &str) -> Result<serde_json::Value> {
        self.execute_json(self.build_request(
            Method::DELETE,
            &["v1", "templates", id_or_slug],
            None::<&()>,
        )?)
        .await
    }

    /// Lists a template's versions, newest first, without `source` (`GET
    /// /v1/templates/{idOrSlug}/versions`, scope `mail:read`).
    pub async fn list_template_versions(&self, id_or_slug: &str) -> Result<serde_json::Value> {
        self.execute_json(self.build_request(
            Method::GET,
            &["v1", "templates", id_or_slug, "versions"],
            None::<&()>,
        )?)
        .await
    }

    /// Reads one version of a template including its `source` (`GET
    /// /v1/templates/{idOrSlug}/versions/{version}`, scope `mail:read`).
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
    /// /v1/templates/{idOrSlug}/versions`, scope `mail:send`). Defaults to
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
                _ => default_idempotency_key(&method, url.path()),
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
                        backoff(attempt).await;
                        attempt += 1;
                        request = next;
                        continue;
                    }
                    return Ok(response);
                }
                Err(error) => {
                    if let (Some(next), true) = (next, error.is_timeout() || error.is_connect()) {
                        backoff(attempt).await;
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
        let status = response.status();

        if status.is_success() {
            return response.json::<T>().await.map_err(ApiError::Transport);
        }

        let message = match response.json::<ErrorResponse>().await {
            Ok(error) => error.error.display_message(),
            Err(_) => status
                .canonical_reason()
                .unwrap_or("unexpected API error")
                .to_string(),
        };

        Err(ApiError::Api { status, message })
    }

    /// Executes a mutating request that returns no body on success. The redesign
    /// answers every successful delete with `204 No Content` (and no JSON body),
    /// so this drains the success case without attempting to parse a body and
    /// still surfaces the structured error envelope on failure.
    async fn execute_no_content(&self, request: Request) -> Result<()> {
        let response = self.send_with_retry(request).await?;
        let status = response.status();

        if status.is_success() {
            return Ok(());
        }

        let message = match response.json::<ErrorResponse>().await {
            Ok(error) => error.error.display_message(),
            Err(_) => status
                .canonical_reason()
                .unwrap_or("unexpected API error")
                .to_string(),
        };

        Err(ApiError::Api { status, message })
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

async fn backoff(attempt: u32) {
    let factor = 1u32 << attempt.min(16);
    let delay = RETRY_BASE_BACKOFF
        .saturating_mul(factor)
        .min(RETRY_MAX_BACKOFF);
    tokio::time::sleep(delay).await;
}

/// Deterministic idempotency key for a mutating request that the caller did not
/// supply one for. Stable across retries of the *same* logical request.
fn default_idempotency_key(method: &Method, path: &str) -> String {
    let namespace = Uuid::NAMESPACE_URL;
    let seed = format!("{method} {path}");
    Uuid::new_v5(&namespace, seed.as_bytes()).to_string()
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WhoamiResponse {
    #[serde(rename = "userId")]
    pub user_id: String,
    #[serde(rename = "workspaceId")]
    pub workspace_id: Option<String>,
    #[serde(rename = "apiKey")]
    pub api_key: WhoamiApiKey,
    pub plan: String,
    pub limits: serde_json::Value,
    pub usage: serde_json::Value,
    pub period: serde_json::Value,
    pub notes: serde_json::Value,
    pub storage: WhoamiStorage,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WhoamiApiKey {
    pub id: String,
    pub scopes: Vec<String>,
    /// IP allowlist (single IPs and/or CIDR ranges). Empty means "allow all".
    #[serde(default, rename = "allowedIps")]
    pub allowed_ips: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WhoamiStorage {
    #[serde(rename = "usedBytes")]
    pub used_bytes: i64,
    #[serde(rename = "limitBytes")]
    pub limit_bytes: i64,
    #[serde(rename = "remainingBytes")]
    pub remaining_bytes: i64,
    pub breakdown: serde_json::Value,
}

/// The unified list envelope every list endpoint now returns
/// (`{ "object": "list", "data": [...], "pagination": { nextCursor, hasMore } }`).
/// The CLI reads the typed rows from `data` and the cursor from `pagination`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ListEnvelope<T> {
    #[serde(default = "default_list_object")]
    pub object: String,
    #[serde(default = "Vec::new")]
    pub data: Vec<T>,
    #[serde(default)]
    pub pagination: Pagination,
}

fn default_list_object() -> String {
    "list".to_string()
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CreateDomainRequest {
    pub domain: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Domain {
    pub id: String,
    pub domain: String,
    pub status: String,
    #[serde(rename = "verifiedAt")]
    pub verified_at: Option<String>,
    pub region: String,
    pub records: Vec<DnsRecord>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DnsRecord {
    #[serde(rename = "type")]
    pub record_type: String,
    pub host: String,
    pub value: String,
    pub priority: Option<i64>,
    pub status: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CreateInboxRequest {
    pub username: String,
    pub domain: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub agent: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mode: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Inbox {
    pub id: String,
    pub address: String,
    // The redesign dropped the duplicate `username` field; `localPart` is the
    // single canonical name. Defaulted so older payloads still deserialize.
    #[serde(default)]
    pub username: String,
    #[serde(rename = "localPart")]
    pub local_part: String,
    pub domain: String,
    #[serde(rename = "domainStatus")]
    pub domain_status: Option<String>,
    pub agent: Option<String>,
    pub mode: String,
    pub status: String,
    #[serde(rename = "createdAt")]
    pub created_at: Option<String>,
    #[serde(rename = "lastMessageAt")]
    pub last_message_at: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SendEmailRequest {
    #[serde(rename = "inboxId")]
    pub inbox_id: String,
    pub to: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cc: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bcc: Option<Vec<String>>,
    pub subject: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub html: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub react: Option<SendEmailReact>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub attachments: Option<Vec<SendEmailAttachment>>,
    #[serde(rename = "idempotencyKey", skip_serializing_if = "Option::is_none")]
    pub idempotency_key: Option<String>,
    /// Optional scheduled-send time. RFC3339 with an explicit timezone offset
    /// (e.g. `2026-06-11T09:00:00Z` or `2026-06-11T11:00:00+02:00`). When set, the
    /// send is staged and the response status is `scheduled` with `scheduledAt`.
    #[serde(rename = "sendAt", skip_serializing_if = "Option::is_none")]
    pub send_at: Option<String>,
    #[serde(
        rename = "ignoreComplaints",
        default,
        skip_serializing_if = "std::ops::Not::not"
    )]
    pub ignore_complaints: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SendEmailReact {
    pub source: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub props: Option<serde_json::Map<String, serde_json::Value>>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SendEmailAttachment {
    pub filename: String,
    #[serde(rename = "contentType")]
    pub content_type: String,
    #[serde(rename = "contentBase64")]
    pub content_base64: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub delivery: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SendEmailResponse {
    pub id: String,
    pub status: String,
    #[serde(rename = "providerMessageId")]
    pub provider_message_id: Option<String>,
    pub error: Option<String>,
    /// Set when `status == "scheduled"`: the RFC3339 time the send will fire.
    #[serde(default, rename = "scheduledAt")]
    pub scheduled_at: Option<String>,
    #[serde(default)]
    pub warnings: Vec<SendEmailWarning>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CreateEmailListRequest {
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EmailListMembersRequest {
    pub members: Vec<EmailListMemberInput>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EmailListMemberInput {
    pub email: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
}

/// `GET /v1/lists/{id}` now returns the flat list object with its members
/// carried as a `members` field on it (the redesign dropped the `{ list, members }`
/// wrapper). The list's own fields are flattened in alongside `members`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EmailListDetailResponse {
    #[serde(flatten)]
    pub list: EmailList,
    #[serde(default)]
    pub members: Vec<EmailListMember>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EmailListImportResponse {
    #[serde(rename = "listId")]
    pub list_id: String,
    pub imported: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EmailListSendResponse {
    #[serde(rename = "listId")]
    pub list_id: String,
    #[serde(rename = "listName")]
    pub list_name: String,
    #[serde(rename = "recipientCount")]
    pub recipient_count: usize,
    #[serde(rename = "batchCount")]
    pub batch_count: usize,
    pub emails: Vec<SendEmailResponse>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EmailList {
    pub id: String,
    pub name: String,
    pub description: Option<String>,
    pub status: String,
    #[serde(rename = "memberCount")]
    pub member_count: Option<i64>,
    #[serde(rename = "createdAt")]
    pub created_at: String,
    #[serde(rename = "updatedAt")]
    pub updated_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EmailListMember {
    pub id: String,
    #[serde(rename = "listId")]
    pub list_id: String,
    pub email: String,
    pub name: Option<String>,
    pub status: String,
    pub source: String,
    #[serde(rename = "createdAt")]
    pub created_at: String,
    #[serde(rename = "updatedAt")]
    pub updated_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SendEmailWarning {
    #[serde(default)]
    pub recipient: Option<String>,
    #[serde(default)]
    pub reason: Option<String>,
    #[serde(default)]
    pub message: Option<String>,
    // The redesign renamed the source-send join key to `sourceEmailId`
    // (camelCase of `source_email_id`); accept the old name as a fallback so a
    // mixed-version response still maps.
    #[serde(default, rename = "sourceEmailId", alias = "sourceOutboundEmailId")]
    pub source_outbound_email_id: Option<String>,
    #[serde(default, rename = "providerMessageId")]
    pub provider_message_id: Option<String>,
    #[serde(default, rename = "complaintFeedbackType")]
    pub complaint_feedback_type: Option<String>,
    #[serde(default, rename = "complaintUserAgent")]
    pub complaint_user_agent: Option<String>,
    #[serde(default, rename = "lastEventAt")]
    pub last_event_at: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CreateWebhookRequest {
    pub url: String,
    pub events: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Webhook {
    pub id: String,
    pub url: String,
    pub events: Vec<String>,
    pub status: String,
    #[serde(rename = "createdAt")]
    pub created_at: String,
    #[serde(default, rename = "lastDeliveryAt")]
    pub last_delivery_at: Option<String>,
}

/// `POST /v1/webhooks` returns the created webhook object with the one-time
/// `signingSecret` as a field on it (plus `secretShownOnce: true`), not a sibling
/// top-level key. The webhook fields are flattened in and the secret is read off
/// the same object.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CreateWebhookResponse {
    #[serde(flatten)]
    pub webhook: Webhook,
    #[serde(rename = "signingSecret")]
    pub secret: String,
}

impl std::fmt::Debug for CreateWebhookResponse {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CreateWebhookResponse")
            .field("webhook", &self.webhook)
            .field("secret", &"[REDACTED]")
            .finish()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CreateApiKeyRequest {
    pub name: String,
    pub scopes: Vec<String>,
    /// Optional IP allowlist (single IPs and/or CIDR ranges). Absent/empty means
    /// the key is usable from any IP.
    #[serde(rename = "allowedIps", skip_serializing_if = "Option::is_none")]
    pub allowed_ips: Option<Vec<String>>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ApiKey {
    pub id: String,
    pub name: String,
    pub prefix: String,
    pub scopes: Vec<String>,
    /// IP allowlist (single IPs and/or CIDR ranges). Empty means "allow all".
    #[serde(default, rename = "allowedIps")]
    pub allowed_ips: Vec<String>,
    pub status: String,
    #[serde(rename = "createdAt")]
    pub created_at: String,
    #[serde(rename = "lastUsedAt")]
    pub last_used_at: Option<String>,
}

/// `POST /v1/api-keys` returns the created API-key object with the one-time
/// `secret` as a field on it (plus `secretShownOnce: true`). The key fields are
/// flattened in and the secret is read off the same object.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CreateApiKeyResponse {
    #[serde(flatten)]
    pub api_key: ApiKey,
    pub secret: String,
}

impl std::fmt::Debug for CreateApiKeyResponse {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CreateApiKeyResponse")
            .field("api_key", &self.api_key)
            .field("secret", &"[REDACTED]")
            .finish()
    }
}

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct MessageListQuery {
    pub inbox_id: Option<String>,
    pub thread_id: Option<String>,
    pub direction: Option<String>,
    pub limit: Option<u32>,
    pub cursor: Option<String>,
}

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct ThreadListQuery {
    pub inbox_id: Option<String>,
    pub limit: Option<u32>,
    pub cursor: Option<String>,
}

/// Query for `GET /v1/audit-logs`. `limit` is bounded server-side (1..=100);
/// `cursor` is the opaque `nextCursor` returned by a previous page.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct AuditLogQuery {
    pub limit: Option<u32>,
    pub cursor: Option<String>,
}

/// Query for `GET /v1/events`, the keyset-paginated read over the durable event
/// ledger that `dairo listen` polls.
///
/// - `since` is the opaque keyset cursor (`pagination.nextCursor` from a prior
///   page) the slice resumes strictly after; absent = from the start.
/// - `inbox_id`/`event_type` map to the server's single-valued `inboxId`/`type`
///   filters. `dairo listen` only sets `inboxId` for a single `--inbox`; multiple
///   inboxes stream unfiltered and filter client-side (one monotonic cursor).
/// - `wait` is the long-poll hold time in seconds (server clamps to 0..=25).
/// - `tail` requests only the head cursor "as of now" (`events: []`).
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct EventsQuery {
    pub since: Option<String>,
    pub limit: Option<u32>,
    pub inbox_id: Option<String>,
    pub event_type: Option<String>,
    pub wait: Option<u8>,
    pub tail: bool,
}

/// Query for `GET /v1/verify`, the public agent-provenance verdict endpoint.
///
/// Two mutually exclusive forms, mirroring the SDKs:
/// - by stored message id (`id`) — attest from our own outbound record;
/// - by reconstructed signature (`agent`, `kid`, `sig`, + optional signed
///   fields) — verify a provenance signature against the kid's public key.
///
/// Only the present fields are sent, so a `--id` call emits just `id=...`.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct VerifyAgentQuery {
    pub id: Option<String>,
    pub agent: Option<String>,
    pub kid: Option<String>,
    pub sig: Option<String>,
    pub from: Option<String>,
    /// Comma-joined recipients, matching the signed `to` field.
    pub to: Option<String>,
    pub subject: Option<String>,
    /// The signed timestamp.
    pub ts: Option<String>,
}

/// Query for the A2A view now read through `GET /v1/messages?channel=a2a`
/// (formerly `GET /v1/a2a/messages`): keyset pagination over agent-to-agent
/// hop receipts. `inbox_id` matches either end (sender or recipient) of the hop.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct A2aMessageQuery {
    pub limit: Option<u32>,
    pub cursor: Option<String>,
    pub inbox_id: Option<String>,
}

/// `GET /v1/threads/{id}` now returns the flat thread object with its messages
/// carried as a `messages` field on it (the redesign dropped the
/// `{ thread, messages }` wrapper).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ThreadResponse {
    #[serde(flatten)]
    pub thread: Thread,
    #[serde(default)]
    pub messages: Vec<Message>,
}

#[derive(Debug, Default, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Pagination {
    #[serde(default, rename = "nextCursor")]
    pub next_cursor: Option<String>,
    #[serde(default, rename = "hasMore")]
    pub has_more: bool,
}

/// Response shape of `GET /v1/events`: a page of ledger events, the keyset
/// pagination state, and a per-partition `gaps` list (a lost-event detector
/// surfaced as-is to the operator).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EventsResponse {
    #[serde(default)]
    pub events: Vec<LedgerEvent>,
    pub pagination: Pagination,
    /// Per-partition missing-`seq` reports. Free-form JSON (each entry is
    /// `{ partitionKey, missingSeq: [...] }`); rendered verbatim as a warning.
    #[serde(default)]
    pub gaps: Vec<serde_json::Value>,
}

/// One row from the durable event ledger, matching the backend's
/// `map_ledger_row` projection. Optional join keys (`messageId`, `threadId`,
/// `inboxId`, ...) are absent for events that do not carry them, so every field
/// past the always-present `event_id`/`event_type`/`seq` is defaulted.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LedgerEvent {
    #[serde(rename = "eventId")]
    pub event_id: String,
    #[serde(rename = "type")]
    pub event_type: String,
    #[serde(default)]
    pub seq: Option<i64>,
    #[serde(default, rename = "partitionKey")]
    pub partition_key: Option<String>,
    #[serde(default, rename = "inboxId")]
    pub inbox_id: Option<String>,
    #[serde(default, rename = "threadId")]
    pub thread_id: Option<String>,
    #[serde(default, rename = "idempotencyKey")]
    pub idempotency_key: Option<String>,
    #[serde(default, rename = "outboundEmailId")]
    pub outbound_email_id: Option<String>,
    #[serde(default, rename = "messageId")]
    pub message_id: Option<String>,
    #[serde(default, rename = "providerMessageId")]
    pub provider_message_id: Option<String>,
    #[serde(default, rename = "occurredAt")]
    pub occurred_at: Option<String>,
    #[serde(default, rename = "createdAt")]
    pub created_at: Option<String>,
    #[serde(default)]
    pub data: serde_json::Value,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MessageAddress {
    pub address: String,
    pub name: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Message {
    pub id: String,
    #[serde(rename = "inboxId")]
    pub inbox_id: String,
    #[serde(rename = "threadId")]
    pub thread_id: Option<String>,
    pub direction: String,
    pub status: String,
    pub from: MessageAddress,
    #[serde(default)]
    pub to: Vec<String>,
    #[serde(default)]
    pub cc: Vec<String>,
    #[serde(default)]
    pub bcc: Vec<String>,
    #[serde(default)]
    pub subject: String,
    #[serde(default, rename = "textPreview")]
    pub text_preview: String,
    #[serde(default, rename = "textBody")]
    pub text_body: Option<String>,
    #[serde(default, rename = "htmlBody")]
    pub html_body: Option<String>,
    #[serde(default, rename = "hasHtml")]
    pub has_html: bool,
    #[serde(default, rename = "hasAttachments")]
    pub has_attachments: bool,
    #[serde(rename = "receivedAt")]
    pub received_at: Option<String>,
    #[serde(rename = "createdAt")]
    pub created_at: Option<String>,
    #[serde(default)]
    pub attachments: Vec<MessageAttachment>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MessageAttachment {
    pub id: String,
    #[serde(rename = "messageId")]
    pub message_id: Option<String>,
    pub filename: Option<String>,
    #[serde(rename = "contentType")]
    pub content_type: Option<String>,
    #[serde(rename = "sizeBytes")]
    pub size_bytes: Option<i64>,
    #[serde(rename = "contentId")]
    pub content_id: Option<String>,
    pub disposition: Option<String>,
    #[serde(rename = "createdAt")]
    pub created_at: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AttachmentDownloadUrlResponse {
    pub attachment: MessageAttachment,
    #[serde(rename = "downloadUrl")]
    pub download_url: String,
    #[serde(default, rename = "shareUrl")]
    pub share_url: Option<String>,
    #[serde(rename = "expiresInSeconds")]
    pub expires_in_seconds: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Thread {
    pub id: String,
    #[serde(rename = "inboxId")]
    pub inbox_id: String,
    #[serde(default)]
    pub subject: String,
    #[serde(default)]
    pub status: String,
    #[serde(default, rename = "lastMessageAt")]
    pub last_message_at: Option<String>,
    #[serde(default, rename = "messageCount")]
    pub message_count: u32,
    #[serde(default, rename = "lastMessagePreview")]
    pub last_message_preview: Option<String>,
    #[serde(rename = "createdAt")]
    pub created_at: Option<String>,
    #[serde(rename = "updatedAt")]
    pub updated_at: Option<String>,
}

fn apply_message_query(url: &mut Url, query: &MessageListQuery) {
    let mut pairs = url.query_pairs_mut();
    if let Some(value) = &query.inbox_id {
        pairs.append_pair("inboxId", value);
    }
    if let Some(value) = &query.thread_id {
        pairs.append_pair("threadId", value);
    }
    if let Some(value) = &query.direction {
        pairs.append_pair("direction", value);
    }
    if let Some(value) = query.limit {
        pairs.append_pair("limit", &value.to_string());
    }
    if let Some(value) = &query.cursor {
        pairs.append_pair("cursor", value);
    }
}

fn apply_thread_query(url: &mut Url, query: &ThreadListQuery) {
    let mut pairs = url.query_pairs_mut();
    if let Some(value) = &query.inbox_id {
        pairs.append_pair("inboxId", value);
    }
    if let Some(value) = query.limit {
        pairs.append_pair("limit", &value.to_string());
    }
    if let Some(value) = &query.cursor {
        pairs.append_pair("cursor", value);
    }
}

fn apply_events_query(url: &mut Url, query: &EventsQuery) {
    let mut pairs = url.query_pairs_mut();
    if let Some(value) = &query.since {
        pairs.append_pair("since", value);
    }
    if let Some(value) = query.limit {
        pairs.append_pair("limit", &value.to_string());
    }
    if let Some(value) = &query.inbox_id {
        pairs.append_pair("inboxId", value);
    }
    if let Some(value) = &query.event_type {
        pairs.append_pair("type", value);
    }
    if let Some(value) = query.wait {
        pairs.append_pair("wait", &value.to_string());
    }
    if query.tail {
        pairs.append_pair("tail", "true");
    }
}

fn apply_verify_query(url: &mut Url, query: &VerifyAgentQuery) {
    let mut pairs = url.query_pairs_mut();
    for (key, value) in [
        ("id", &query.id),
        ("agent", &query.agent),
        ("kid", &query.kid),
        ("sig", &query.sig),
        ("from", &query.from),
        ("to", &query.to),
        ("subject", &query.subject),
        ("ts", &query.ts),
    ] {
        if let Some(value) = value {
            pairs.append_pair(key, value);
        }
    }
}

/// Per-request timeout for `GET /v1/events`: a long-poll hang holds the request
/// open for up to `wait` seconds, so the deadline is `wait + 5s` (connect +
/// final-page margin). A `wait`-less call (tail / immediate) still gets the 5s
/// floor rather than the shared 30s timeout, which is plenty for a single
/// index-covered read.
const EVENTS_REQUEST_TIMEOUT_MARGIN: Duration = Duration::from_secs(5);

fn events_request_timeout(wait: Option<u8>) -> Duration {
    Duration::from_secs(u64::from(wait.unwrap_or(0))) + EVENTS_REQUEST_TIMEOUT_MARGIN
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
struct ErrorResponse {
    error: ErrorBody,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
struct ErrorBody {
    code: Option<String>,
    message: String,
}

impl ErrorBody {
    fn display_message(self) -> String {
        match self.code {
            Some(code) if !code.trim().is_empty() => format!("[{}] {}", code, self.message),
            _ => self.message,
        }
    }
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

    #[test]
    fn idempotency_key_is_stable_across_retries_of_same_request() {
        // A per-invocation random key would defeat retry de-dup. Re-building the
        // same logical request must yield the same default key.
        let client = ApiClient::new("https://api.example.test", "token").unwrap();
        let first = client
            .build_request(Method::POST, &["v1", "emails"], None::<&()>)
            .unwrap();
        let second = client
            .build_request(Method::POST, &["v1", "emails"], None::<&()>)
            .unwrap();
        let key_of = |req: &Request| {
            req.headers()
                .get("Idempotency-Key")
                .unwrap()
                .to_str()
                .unwrap()
                .to_string()
        };
        assert_eq!(key_of(&first), key_of(&second));
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
        };

        let value = serde_json::to_value(body).unwrap();

        assert!(value.get("sendAt").is_none());
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
}
