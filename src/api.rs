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

    pub async fn list_domains(&self) -> Result<DomainListResponse> {
        self.execute_json(self.build_request(Method::GET, &["v1", "domains"], None::<&()>)?)
            .await
    }

    pub async fn create_domain(&self, body: &CreateDomainRequest) -> Result<DomainListResponse> {
        self.execute_json(self.build_request(Method::POST, &["v1", "domains"], Some(body))?)
            .await
    }

    pub async fn delete_domain(&self, domain: &str) -> Result<DomainListResponse> {
        self.execute_json(self.build_request(
            Method::DELETE,
            &["v1", "domains", domain],
            None::<&()>,
        )?)
        .await
    }

    pub async fn recheck_domain(&self, domain: &str) -> Result<DomainListResponse> {
        self.execute_json(self.build_request(
            Method::POST,
            &["v1", "domains", domain, "recheck"],
            None::<&()>,
        )?)
        .await
    }

    pub async fn list_inboxes(&self) -> Result<InboxListResponse> {
        self.execute_json(self.build_request(Method::GET, &["v1", "inboxes"], None::<&()>)?)
            .await
    }

    pub async fn create_inbox(&self, body: &CreateInboxRequest) -> Result<InboxResponse> {
        self.execute_json(self.build_request(Method::POST, &["v1", "inboxes"], Some(body))?)
            .await
    }

    pub async fn delete_inbox(&self, inbox: &str) -> Result<DeleteResponse> {
        self.execute_json(self.build_request(
            Method::DELETE,
            &["v1", "inboxes", inbox],
            None::<&()>,
        )?)
        .await
    }

    pub async fn send_email(&self, body: &SendEmailRequest) -> Result<SendEmailResponse> {
        self.execute_json(self.build_request(Method::POST, &["v1", "send-email"], Some(body))?)
            .await
    }

    pub async fn list_email_lists(&self) -> Result<EmailListListResponse> {
        self.execute_json(self.build_request(Method::GET, &["v1", "email-lists"], None::<&()>)?)
            .await
    }

    pub async fn create_email_list(
        &self,
        body: &CreateEmailListRequest,
    ) -> Result<EmailListResponse> {
        self.execute_json(self.build_request(Method::POST, &["v1", "email-lists"], Some(body))?)
            .await
    }

    pub async fn get_email_list(&self, list_id: &str) -> Result<EmailListDetailResponse> {
        self.execute_json(self.build_request(
            Method::GET,
            &["v1", "email-lists", list_id],
            None::<&()>,
        )?)
        .await
    }

    pub async fn delete_email_list(&self, list_id: &str) -> Result<DeleteResponse> {
        self.execute_json(self.build_request(
            Method::DELETE,
            &["v1", "email-lists", list_id],
            None::<&()>,
        )?)
        .await
    }

    /// Upserts members via the canonical `POST /members` endpoint (<= 2000
    /// members). `import_email_list_members` posts to the `/members/import`
    /// alias which the backend treats identically.
    pub async fn add_email_list_members(
        &self,
        list_id: &str,
        body: &EmailListMembersRequest,
    ) -> Result<EmailListImportResponse> {
        self.execute_json(self.build_request(
            Method::POST,
            &["v1", "email-lists", list_id, "members"],
            Some(body),
        )?)
        .await
    }

    pub async fn import_email_list_members(
        &self,
        list_id: &str,
        body: &EmailListMembersRequest,
    ) -> Result<EmailListImportResponse> {
        self.execute_json(self.build_request(
            Method::POST,
            &["v1", "email-lists", list_id, "members", "import"],
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
            &["v1", "email-lists", list_id, "send"],
            Some(body),
        )?)
        .await
    }

    pub async fn list_webhooks(&self) -> Result<WebhookListResponse> {
        self.execute_json(self.build_request(Method::GET, &["v1", "webhooks"], None::<&()>)?)
            .await
    }

    pub async fn create_webhook(
        &self,
        body: &CreateWebhookRequest,
    ) -> Result<CreateWebhookResponse> {
        self.execute_json(self.build_request(Method::POST, &["v1", "webhooks"], Some(body))?)
            .await
    }

    pub async fn delete_webhook(&self, webhook: &str) -> Result<DeleteResponse> {
        self.execute_json(self.build_request(
            Method::DELETE,
            &["v1", "webhooks", webhook],
            None::<&()>,
        )?)
        .await
    }

    pub async fn list_api_keys(&self) -> Result<ApiKeyListResponse> {
        self.execute_json(self.build_request(Method::GET, &["v1", "api-keys"], None::<&()>)?)
            .await
    }

    pub async fn create_api_key(&self, body: &CreateApiKeyRequest) -> Result<CreateApiKeyResponse> {
        self.execute_json(self.build_request(Method::POST, &["v1", "api-keys"], Some(body))?)
            .await
    }

    pub async fn revoke_api_key(&self, api_key_id: &str) -> Result<DeleteResponse> {
        self.execute_json(self.build_request(
            Method::DELETE,
            &["v1", "api-keys", api_key_id],
            None::<&()>,
        )?)
        .await
    }

    pub async fn list_messages(&self, query: &MessageListQuery) -> Result<MessageListResponse> {
        let mut request = self.build_request(Method::GET, &["v1", "messages"], None::<&()>)?;
        apply_message_query(request.url_mut(), query);
        self.execute_json(request).await
    }

    pub async fn get_message(&self, message_id: &str) -> Result<MessageResponse> {
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

    pub async fn list_threads(&self, query: &ThreadListQuery) -> Result<ThreadListResponse> {
        let mut request = self.build_request(Method::GET, &["v1", "threads"], None::<&()>)?;
        apply_thread_query(request.url_mut(), query);
        self.execute_json(request).await
    }

    pub async fn get_thread(&self, thread_id: &str) -> Result<ThreadResponse> {
        self.execute_json(self.build_request(
            Method::GET,
            &["v1", "threads", thread_id],
            None::<&()>,
        )?)
        .await
    }

    pub async fn list_outbound_emails(&self, limit: Option<u32>) -> Result<serde_json::Value> {
        let mut request =
            self.build_request(Method::GET, &["v1", "outbound-emails"], None::<&()>)?;
        if let Some(limit) = limit {
            request
                .url_mut()
                .query_pairs_mut()
                .append_pair("limit", &limit.to_string());
        }
        self.execute_json(request).await
    }

    pub async fn get_outbound_email(&self, email_id: &str) -> Result<serde_json::Value> {
        self.execute_json(self.build_request(
            Method::GET,
            &["v1", "outbound-emails", email_id],
            None::<&()>,
        )?)
        .await
    }

    /// Cancels a scheduled outbound email. Returns the canceled outbound email
    /// (`{ "email": { ... status: "canceled", canceledAt } }`), or surfaces the
    /// backend's `409 Conflict` if the email is no longer scheduled. Scope:
    /// `mail:send`.
    pub async fn cancel_outbound_email(&self, email_id: &str) -> Result<serde_json::Value> {
        self.execute_json(self.build_request(
            Method::POST,
            &["v1", "outbound-emails", email_id, "cancel"],
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

    pub async fn list_outbound_events(
        &self,
        email_id: Option<&str>,
        limit: Option<u32>,
    ) -> Result<serde_json::Value> {
        let mut request =
            self.build_request(Method::GET, &["v1", "outbound-events"], None::<&()>)?;
        {
            let mut pairs = request.url_mut().query_pairs_mut();
            if let Some(email_id) = email_id {
                pairs.append_pair("emailId", email_id);
            }
            if let Some(limit) = limit {
                pairs.append_pair("limit", &limit.to_string());
            }
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

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CreateDomainRequest {
    pub domain: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DomainListResponse {
    pub domains: Vec<Domain>,
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
pub struct InboxResponse {
    pub inbox: Inbox,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct InboxListResponse {
    pub inboxes: Vec<Inbox>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Inbox {
    pub id: String,
    pub address: String,
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

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EmailListListResponse {
    pub lists: Vec<EmailList>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EmailListResponse {
    pub list: EmailList,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EmailListDetailResponse {
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
    #[serde(default, rename = "sourceOutboundEmailId")]
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
pub struct WebhookListResponse {
    pub webhooks: Vec<Webhook>,
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

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CreateWebhookResponse {
    pub webhook: Webhook,
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
pub struct ApiKeyListResponse {
    #[serde(rename = "apiKeys")]
    pub api_keys: Vec<ApiKey>,
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

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CreateApiKeyResponse {
    #[serde(rename = "apiKey")]
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

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeleteResponse {
    pub deleted: bool,
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

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MessageListResponse {
    pub messages: Vec<Message>,
    pub pagination: Pagination,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MessageResponse {
    pub message: Message,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ThreadListResponse {
    pub threads: Vec<Thread>,
    pub pagination: Pagination,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ThreadResponse {
    pub thread: Thread,
    #[serde(default)]
    pub messages: Vec<Message>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Pagination {
    #[serde(rename = "nextCursor")]
    pub next_cursor: Option<String>,
    #[serde(default, rename = "hasMore")]
    pub has_more: bool,
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
            .build_request(Method::POST, &["v1", "send-email"], None::<&()>)
            .unwrap();
        let second = client
            .build_request(Method::POST, &["v1", "send-email"], None::<&()>)
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
                &["v1", "send-email"],
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
            .build_request(
                Method::DELETE,
                &["v1", "email-lists", "list_123"],
                None::<&()>,
            )
            .unwrap();
        assert_eq!(request.method(), Method::DELETE);
        assert_eq!(
            request.url().as_str(),
            "https://api.example.test/v1/email-lists/list_123"
        );
    }

    #[test]
    fn encodes_path_segments() {
        let client = ApiClient::new("https://api.example.test", "token").unwrap();
        let request = client
            .build_request(
                Method::POST,
                &["v1", "domains", "weird domain.example", "recheck"],
                None::<&()>,
            )
            .unwrap();

        assert_eq!(
            request.url().as_str(),
            "https://api.example.test/v1/domains/weird%20domain.example/recheck"
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
        let response: ApiKeyListResponse = serde_json::from_value(serde_json::json!({
            "apiKeys": [
                {
                    "id": "key_1",
                    "name": "scoped",
                    "prefix": "dairo_test_a",
                    "scopes": ["mail:send"],
                    "allowedIps": ["203.0.113.0/24"],
                    "status": "active",
                    "createdAt": "2026-06-01T00:00:00Z",
                    "lastUsedAt": null
                },
                {
                    "id": "key_2",
                    "name": "open",
                    "prefix": "dairo_test_b",
                    "scopes": ["mail:read"],
                    "status": "active",
                    "createdAt": "2026-06-01T00:00:00Z",
                    "lastUsedAt": null
                }
            ]
        }))
        .unwrap();

        assert_eq!(response.api_keys[0].allowed_ips, vec!["203.0.113.0/24"]);
        assert!(response.api_keys[1].allowed_ips.is_empty());
    }

    #[test]
    fn cancel_outbound_email_targets_cancel_route() {
        let client = ApiClient::new("https://api.example.test", "token").unwrap();
        let request = client
            .build_request(
                Method::POST,
                &["v1", "outbound-emails", "email_123", "cancel"],
                None::<&()>,
            )
            .unwrap();
        assert_eq!(request.method(), Method::POST);
        assert_eq!(
            request.url().as_str(),
            "https://api.example.test/v1/outbound-emails/email_123/cancel"
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
        let response: WebhookListResponse = serde_json::from_value(serde_json::json!({
            "webhooks": [
                {
                    "id": "wh_123",
                    "url": "https://example.com/hook",
                    "events": ["message.received", "email.delivered"],
                    "status": "active",
                    "createdAt": "2026-06-01T00:00:00Z",
                    "lastDeliveryAt": "2026-06-02T10:00:00Z"
                }
            ]
        }))
        .unwrap();

        let webhook = &response.webhooks[0];
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
