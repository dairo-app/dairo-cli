use reqwest::{Method, Request, StatusCode};
use serde::{Deserialize, Serialize};
use thiserror::Error;
use url::Url;
use uuid::Uuid;

pub const DEFAULT_BASE_URL: &str = "https://backend.dairo.app";

#[derive(Debug, Error)]
pub enum ApiError {
    #[error("invalid API base URL: {0}")]
    InvalidBaseUrl(#[from] url::ParseError),
    #[error("failed to build API request: {0}")]
    BuildRequest(#[source] reqwest::Error),
    #[error("request failed: {0}")]
    Transport(#[from] reqwest::Error),
    #[error("Dairo API returned {status}: {message}")]
    Api { status: StatusCode, message: String },
}

pub type Result<T> = std::result::Result<T, ApiError>;

#[derive(Debug, Clone)]
pub struct ApiClient {
    base_url: Url,
    api_key: String,
    http: reqwest::Client,
}

impl ApiClient {
    pub fn new(base_url: impl AsRef<str>, api_key: impl Into<String>) -> Result<Self> {
        Ok(Self {
            base_url: Url::parse(base_url.as_ref())?,
            api_key: api_key.into(),
            http: reqwest::Client::new(),
        })
    }

    pub async fn list_domains(&self) -> Result<DomainListResponse> {
        self.execute_json(self.build_request(Method::GET, &["v1", "domains"], None::<&()>)?)
            .await
    }

    pub async fn create_domain(&self, body: &CreateDomainRequest) -> Result<DomainListResponse> {
        self.execute_json(self.build_request(Method::POST, &["v1", "domains"], Some(body))?)
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

    pub async fn send_email(&self, body: &SendEmailRequest) -> Result<SendEmailResponse> {
        self.execute_json(self.build_request(Method::POST, &["v1", "send-email"], Some(body))?)
            .await
    }

    pub(crate) fn build_request<T: Serialize>(
        &self,
        method: Method,
        path_segments: &[&str],
        body: Option<T>,
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
            .request(method.clone(), url)
            .bearer_auth(&self.api_key)
            .header("Accept", "application/json");

        if matches!(
            method,
            Method::POST | Method::PUT | Method::PATCH | Method::DELETE
        ) {
            builder = builder.header("Idempotency-Key", Uuid::new_v4().to_string());
        }

        if let Some(body) = body {
            builder = builder.json(&body);
        }

        builder.build().map_err(ApiError::BuildRequest)
    }

    async fn execute_json<T: for<'de> Deserialize<'de>>(&self, request: Request) -> Result<T> {
        let response = self.http.execute(request).await?;
        let status = response.status();

        if status.is_success() {
            return response.json::<T>().await.map_err(ApiError::Transport);
        }

        let message = match response.json::<ErrorResponse>().await {
            Ok(error) => error.error.message,
            Err(_) => status
                .canonical_reason()
                .unwrap_or("unexpected API error")
                .to_string(),
        };

        Err(ApiError::Api { status, message })
    }
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

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SendEmailRequest {
    #[serde(rename = "inboxId")]
    pub inbox_id: String,
    pub to: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cc: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bcc: Option<Vec<String>>,
    pub subject: String,
    pub text: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub html: Option<String>,
    #[serde(rename = "idempotencyKey", skip_serializing_if = "Option::is_none")]
    pub idempotency_key: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SendEmailResponse {
    pub id: String,
    pub status: String,
    #[serde(rename = "providerMessageId")]
    pub provider_message_id: Option<String>,
    pub error: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
struct ErrorResponse {
    error: ErrorBody,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
struct ErrorBody {
    message: String,
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
            text: "Body".to_string(),
            html: None,
            idempotency_key: None,
        };

        let value = serde_json::to_value(body).unwrap();

        assert_eq!(value["inboxId"], "018f");
        assert_eq!(value["to"][0], "max@example.com");
        assert_eq!(value["subject"], "Hello");
        assert!(value.get("cc").is_none());
    }
}
