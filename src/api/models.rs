//! Wire types for the Dairo API: request/response models, list/query parameter
//! structs, the small URL query-application helpers, and the error envelope.
//!
//! Split out of the former monolithic `api.rs` purely to shrink the file; the
//! public surface is unchanged because `super` re-exports everything with
//! `pub use models::*`.

use std::time::Duration;

use serde::{Deserialize, Serialize};
use url::Url;

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
    /// Optional single reply-to address. Omitted entirely when unset.
    #[serde(rename = "replyTo", skip_serializing_if = "Option::is_none")]
    pub reply_to: Option<String>,
    /// Optional custom MIME headers (`{ name: value }`), allowlisted server-side.
    /// Omitted entirely when empty/unset.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub headers: Option<std::collections::BTreeMap<String, String>>,
    /// Optional SES message tags (`{ name: value }`). Omitted entirely when
    /// empty/unset.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tags: Option<std::collections::BTreeMap<String, String>>,
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

// ---------------------------------------------------------------------------
// Letters (physical-mail surface)
// ---------------------------------------------------------------------------
// Wire types for the `/v1/letters` resource. The PII-bearing address blocks
// (`to`/`from`) and the print options carry the unified envelope's camelCase
// field names. Optional fields are `skip_serializing_if = "Option::is_none"` so
// an unset flag is omitted from the wire request entirely, exactly like
// `SendEmailRequest`.

/// `POST /v1/letters` request body. Exactly one of `pdf_base64` / `file`
/// carries the PDF; the CLI enforces the exactly-one rule before the request
/// goes out.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CreateLetterRequest {
    #[serde(rename = "pdfBase64", skip_serializing_if = "Option::is_none")]
    pub pdf_base64: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub file: Option<LetterFileRef>,
    #[serde(rename = "fileName")]
    pub file_name: String,
    pub to: PostalAddress,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub from: Option<PostalAddress>,
    /// Optional Dairo letter template to render server-side (the "Dairo-render"
    /// path). When set, the PDF is generated from the template rather than
    /// supplied inline; it is also the only path on which a structured `payment`
    /// slip is honored (a `pdfBase64` letter plus `payment` is rejected
    /// client-side). Omitted from the wire request when unset.
    #[serde(rename = "templateId", skip_serializing_if = "Option::is_none")]
    pub template_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub print: Option<LetterPrintOptions>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub delivery: Option<String>,
    /// Optional payment-slip overlay token (`qr`/`sepaDe`/`sepaAt`). Omitted from
    /// the wire request when unset (a normal letter with no slip). This bare
    /// string flag is the bring-your-own-slip path: the supplied PDF already
    /// carries a slip and this only tells the provider which paper to use. For a
    /// Dairo-generated slip, send the structured `payment` object instead (which
    /// also sets this flag from `payment.type`).
    #[serde(rename = "paymentSlip", skip_serializing_if = "Option::is_none")]
    pub payment_slip: Option<String>,
    /// Optional structured payment slip that Dairo *generates* and composites
    /// full-width at the bottom of the rendered letter. Honored only on the
    /// Dairo-render path (`template_id`); when present the CLI also sets
    /// `payment_slip` from `payment.type`. Omitted from the wire request when
    /// unset.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub payment: Option<LetterPayment>,
    /// Opt-in to delivery-tracking notifications. `Some(false)` is sent
    /// explicitly; `None` omits the field so the backend applies its default.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub notifications: Option<bool>,
    /// `false` creates the letter as a draft (not auto-submitted). The wire
    /// default is `true`, so the field is omitted when auto-send is requested.
    #[serde(rename = "autoSend", skip_serializing_if = "Option::is_none")]
    pub auto_send: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub metadata: Option<serde_json::Value>,
}

/// A reference to an existing Dairo attachment used as the letter's PDF, an
/// alternative to inline `pdfBase64`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LetterFileRef {
    #[serde(rename = "attachmentId")]
    pub attachment_id: String,
    #[serde(rename = "messageId", skip_serializing_if = "Option::is_none")]
    pub message_id: Option<String>,
}

/// Structured payment slip that Dairo generates and composites at the bottom of
/// a template-rendered letter. The slip kind drives the format: `qr` is a Swiss
/// QR-bill (CHF), `sepaDe`/`sepaAt` are German/Austrian SEPA Zahlschein + GiroCode
/// (EUR). Field names are the unified envelope's camelCase. Optional fields are
/// omitted from the wire request when unset, exactly like `PostalAddress`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LetterPayment {
    /// Slip kind / public token: `qr` (Swiss QR-bill), `sepaDe` (German SEPA),
    /// or `sepaAt` (Austrian SEPA).
    #[serde(rename = "type")]
    pub payment_type: String,
    /// The party being paid (the slip's beneficiary).
    pub creditor: LetterCreditor,
    /// Amount due. Must be > 0 with at most two decimal places (enforced
    /// client-side before the request goes out).
    pub amount: f64,
    /// `CHF` for `qr`, `EUR` for `sepaDe`/`sepaAt` (the CLI enforces the pairing).
    pub currency: String,
    /// Optional structured reference (e.g. a QR / creditor reference).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reference: Option<String>,
    /// Optional unstructured remittance information (Verwendungszweck).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
    /// Optional payer. Omitted from the wire request when unset; the CLI defaults
    /// it to the letter's `to` address.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub debtor: Option<LetterDebtor>,
}

/// The beneficiary of a payment slip. `name`, `iban`, and `country` are required
/// by the contract; every other field is omitted from the wire request when
/// unset.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LetterCreditor {
    pub name: String,
    pub iban: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bic: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub street: Option<String>,
    #[serde(rename = "houseNumber", skip_serializing_if = "Option::is_none")]
    pub house_number: Option<String>,
    #[serde(rename = "postalCode", skip_serializing_if = "Option::is_none")]
    pub postal_code: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub city: Option<String>,
    pub country: String,
}

/// The payer of a payment slip. Only `name` and `country` are required; every
/// other field is omitted from the wire request when unset. Defaults to the
/// letter's `to` address when not supplied explicitly.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LetterDebtor {
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub street: Option<String>,
    #[serde(rename = "houseNumber", skip_serializing_if = "Option::is_none")]
    pub house_number: Option<String>,
    #[serde(rename = "postalCode", skip_serializing_if = "Option::is_none")]
    pub postal_code: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub city: Option<String>,
    pub country: String,
}

/// A postal address (`to`/`from`). Only `country` is required by the contract;
/// every other field is omitted from the wire request when unset.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct PostalAddress {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub company: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub street: Option<String>,
    #[serde(rename = "houseNumber", skip_serializing_if = "Option::is_none")]
    pub house_number: Option<String>,
    #[serde(rename = "poBox", skip_serializing_if = "Option::is_none")]
    pub po_box: Option<String>,
    #[serde(rename = "addressLine2", skip_serializing_if = "Option::is_none")]
    pub address_line2: Option<String>,
    #[serde(rename = "postalCode", skip_serializing_if = "Option::is_none")]
    pub postal_code: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub city: Option<String>,
    pub country: String,
}

/// Print options (`{ mode, sides, addressPlacement }`). Each value is omitted
/// from the wire request when unset so the backend applies its own defaults.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct LetterPrintOptions {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mode: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sides: Option<String>,
    #[serde(rename = "addressPlacement", skip_serializing_if = "Option::is_none")]
    pub address_placement: Option<String>,
}

impl LetterPrintOptions {
    /// `true` when no print option was set, so the field can be omitted entirely
    /// from the request rather than sending an empty object.
    pub fn is_empty(&self) -> bool {
        self.mode.is_none() && self.sides.is_none() && self.address_placement.is_none()
    }
}

/// `POST /v1/letters/price` request body. Either `page_count` (cheap preview)
/// or `pdf_base64` (exact page count) drives the price; `country` is required.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LetterPriceRequest {
    pub country: String,
    #[serde(rename = "pageCount", skip_serializing_if = "Option::is_none")]
    pub page_count: Option<u32>,
    #[serde(rename = "pdfBase64", skip_serializing_if = "Option::is_none")]
    pub pdf_base64: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub print: Option<LetterPrintOptions>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub delivery: Option<String>,
    #[serde(rename = "paperTypes", skip_serializing_if = "Option::is_none")]
    pub paper_types: Option<Vec<String>>,
}

/// Query for `GET /v1/letters`: keyset pagination plus optional `status` /
/// `country` filters. Empty filters are not appended to the URL.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct LetterListQuery {
    pub limit: Option<u32>,
    pub cursor: Option<String>,
    pub status: Option<String>,
    pub country: Option<String>,
}

// ---------------------------------------------------------------------------
// Storage buckets (`/v1/buckets`)
// ---------------------------------------------------------------------------
// Wire types for the named-bucket object store. Bucket reads use `buckets:read`;
// create/patch/delete and the object upload/finalize/delete mutations use
// `buckets:write`. The single-object and list endpoints return the unified
// envelope, rendered verbatim by `print_json`. Upload is a three-step flow:
// initiate (returns a presigned PUT URL + required SSE headers), PUT the bytes
// straight to S3, then finalize (HEADs for the true size and records the
// ledger object). Optional request fields are omitted from the wire request
// when unset, exactly like `SendEmailRequest`.

/// `POST /v1/buckets` request body. `name` is the unique-per-user slug; the
/// optional display name / description default server-side.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CreateBucketRequest {
    pub name: String,
    #[serde(rename = "displayName", skip_serializing_if = "Option::is_none")]
    pub display_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
}

/// `POST /v1/buckets/{bucketId}/objects` request body: initiate an upload. The
/// optional `expectedBytes` lets the backend pre-check the storage limit; the
/// true size is HEADed at finalize regardless.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct InitiateUploadRequest {
    pub filename: String,
    #[serde(rename = "contentType")]
    pub content_type: String,
    #[serde(rename = "expectedBytes", skip_serializing_if = "Option::is_none")]
    pub expected_bytes: Option<u64>,
}

/// Response of `POST /v1/buckets/{bucketId}/objects`: a presigned S3 PUT the
/// client uploads the bytes to, plus any SSE headers that MUST be echoed on the
/// PUT (else S3 rejects with 403). Nothing is recorded in the ledger yet.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct InitiateUploadResponse {
    #[serde(rename = "objectId")]
    pub object_id: String,
    #[serde(rename = "uploadUrl")]
    pub upload_url: String,
    #[serde(default = "default_put_method")]
    pub method: String,
    /// SSE (and any other) headers that MUST accompany the PUT, exactly as the
    /// bucket policy requires. Empty when none are needed.
    #[serde(default)]
    pub headers: std::collections::BTreeMap<String, String>,
    #[serde(rename = "expiresInSeconds")]
    pub expires_in_seconds: u64,
}

fn default_put_method() -> String {
    "PUT".to_string()
}

/// Response of `GET /v1/buckets/{bucketId}/objects/{objectId}/download`: a
/// presigned S3 GET URL the client streams the bytes from.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BucketObjectDownloadResponse {
    #[serde(rename = "downloadUrl")]
    pub download_url: String,
    #[serde(rename = "expiresInSeconds")]
    pub expires_in_seconds: u64,
}

/// Query for `GET /v1/buckets/{bucketId}/objects`: keyset pagination over a
/// bucket's objects. Empty values are not appended to the URL.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct BucketObjectListQuery {
    pub limit: Option<u32>,
    pub cursor: Option<String>,
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

pub(crate) fn apply_message_query(url: &mut Url, query: &MessageListQuery) {
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

pub(crate) fn apply_thread_query(url: &mut Url, query: &ThreadListQuery) {
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

pub(crate) fn apply_events_query(url: &mut Url, query: &EventsQuery) {
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

pub(crate) fn apply_letter_query(url: &mut Url, query: &LetterListQuery) {
    let mut pairs = url.query_pairs_mut();
    if let Some(value) = query.limit {
        pairs.append_pair("limit", &value.to_string());
    }
    if let Some(value) = &query.cursor {
        pairs.append_pair("cursor", value);
    }
    if let Some(value) = &query.status {
        pairs.append_pair("status", value);
    }
    if let Some(value) = &query.country {
        pairs.append_pair("country", value);
    }
}

pub(crate) fn apply_bucket_object_query(url: &mut Url, query: &BucketObjectListQuery) {
    let mut pairs = url.query_pairs_mut();
    if let Some(value) = query.limit {
        pairs.append_pair("limit", &value.to_string());
    }
    if let Some(value) = &query.cursor {
        pairs.append_pair("cursor", value);
    }
}

pub(crate) fn apply_verify_query(url: &mut Url, query: &VerifyAgentQuery) {
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
pub(crate) const EVENTS_REQUEST_TIMEOUT_MARGIN: Duration = Duration::from_secs(5);

pub(crate) fn events_request_timeout(wait: Option<u8>) -> Duration {
    Duration::from_secs(u64::from(wait.unwrap_or(0))) + EVENTS_REQUEST_TIMEOUT_MARGIN
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub(crate) struct ErrorResponse {
    pub(crate) error: ErrorBody,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub(crate) struct ErrorBody {
    code: Option<String>,
    message: String,
}

impl ErrorBody {
    pub(crate) fn display_message(self) -> String {
        match self.code {
            Some(code) if !code.trim().is_empty() => format!("[{}] {}", code, self.message),
            _ => self.message,
        }
    }
}
