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
    /// Delivery channel this inbox provisions (`email`, ...). Added by the
    /// channel-agnostic redesign; defaulted so older payloads still deserialize.
    #[serde(default)]
    pub channel: String,
    pub address: String,
    // The redesign dropped the duplicate `username` alias; `localPart` is the
    // single canonical name.
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
pub struct SendMessageRequest {
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
    pub react: Option<SendMessageReact>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub attachments: Option<Vec<SendMessageAttachment>>,
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
    /// Optional delivery channel (defaults to the inbox's channel, `email`).
    /// Part of the channel-agnostic send request (was `SendMessageRequest`, now
    /// the unified `SendMessageRequest`). Omitted entirely when unset.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub channel: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SendMessageReact {
    pub source: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub props: Option<serde_json::Map<String, serde_json::Value>>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SendMessageAttachment {
    pub filename: String,
    #[serde(rename = "contentType")]
    pub content_type: String,
    #[serde(rename = "contentBase64")]
    pub content_base64: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub delivery: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SendMessageResponse {
    pub id: String,
    pub status: String,
    #[serde(rename = "providerMessageId")]
    pub provider_message_id: Option<String>,
    pub error: Option<String>,
    /// Set when `status == "scheduled"`: the RFC3339 time the send will fire.
    #[serde(default, rename = "scheduledAt")]
    pub scheduled_at: Option<String>,
    #[serde(default)]
    pub warnings: Vec<SendMessageWarning>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CreateAudienceRequest {
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AudienceMembersRequest {
    pub members: Vec<AudienceMemberInput>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AudienceMemberInput {
    pub handle: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
}

/// `GET /v1/audiences/{id}` now returns the flat list object with its members
/// carried as a `members` field on it (the redesign dropped the `{ list, members }`
/// wrapper). The list's own fields are flattened in alongside `members`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AudienceDetailResponse {
    #[serde(flatten)]
    pub list: Audience,
    #[serde(default)]
    pub members: Vec<AudienceMember>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AudienceImportResponse {
    #[serde(rename = "audienceId")]
    pub audience_id: String,
    pub imported: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AudienceSendResponse {
    #[serde(rename = "audienceId")]
    pub audience_id: String,
    #[serde(rename = "listName")]
    pub list_name: String,
    #[serde(rename = "recipientCount")]
    pub recipient_count: usize,
    #[serde(rename = "batchCount")]
    pub batch_count: usize,
    pub messages: Vec<SendMessageResponse>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Audience {
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
pub struct AudienceMember {
    pub id: String,
    #[serde(rename = "audienceId")]
    pub audience_id: String,
    pub handle: String,
    pub name: Option<String>,
    pub status: String,
    pub source: String,
    #[serde(rename = "createdAt")]
    pub created_at: String,
    #[serde(rename = "updatedAt")]
    pub updated_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SendMessageWarning {
    #[serde(default)]
    pub recipient: Option<String>,
    #[serde(default)]
    pub reason: Option<String>,
    #[serde(default)]
    pub message: Option<String>,
    // Source-send join key: the unified message id (`sourceMessageId`) that
    // originally produced this warning, when known.
    #[serde(default, rename = "sourceMessageId")]
    pub source_outbound_message_id: Option<String>,
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
// `SendMessageRequest`.

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
// when unset, exactly like `SendMessageRequest`.

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

/// `POST /v1/buckets/{bucketId}/objects/multipart` request body: initiate a
/// multipart upload. `partSize` is optional (backend default 256MiB, range
/// 5MiB..5GiB); the backend computes `partCount = ceil(totalBytes/partSize)`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct InitiateMultipartRequest {
    pub filename: String,
    #[serde(rename = "contentType")]
    pub content_type: String,
    #[serde(rename = "totalBytes")]
    pub total_bytes: u64,
    #[serde(rename = "partSize", skip_serializing_if = "Option::is_none")]
    pub part_size: Option<u64>,
}

/// One presigned part PUT in the multipart initiate response. The client PUTs
/// the corresponding byte range to `url` (echoing the initiate-level `headers`)
/// and reads the `ETag` response header back.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MultipartPartUrl {
    #[serde(rename = "partNumber")]
    pub part_number: u32,
    pub url: String,
}

/// Response of `POST /v1/buckets/{bucketId}/objects/multipart`: the S3 upload
/// id, the placeholder ledger object id, the part size/count, the per-part
/// branded presigned PUT urls, and the headers that MUST accompany every part
/// PUT (`x-amz-content-sha256: UNSIGNED-PAYLOAD`; NO SSE header on parts).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct InitiateMultipartResponse {
    #[serde(rename = "uploadId")]
    pub upload_id: String,
    #[serde(rename = "objectId")]
    pub object_id: String,
    #[serde(rename = "bucketId")]
    pub bucket_id: String,
    pub key: String,
    #[serde(default = "default_put_method")]
    pub method: String,
    #[serde(rename = "partSize")]
    pub part_size: u64,
    #[serde(rename = "partCount")]
    pub part_count: u32,
    /// Headers that MUST be echoed on EVERY part PUT, exactly as signed.
    #[serde(default)]
    pub headers: std::collections::BTreeMap<String, String>,
    pub parts: Vec<MultipartPartUrl>,
    #[serde(rename = "expiresInSeconds")]
    pub expires_in_seconds: u64,
}

/// One completed part `{ partNumber, etag }` reported back at complete time.
/// The S3 `ETag` response header (quotes included) is passed through verbatim.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CompletedPart {
    #[serde(rename = "partNumber")]
    pub part_number: u32,
    pub etag: String,
}

/// `POST /v1/buckets/{bucketId}/objects/multipart/{uploadId}/complete` request
/// body: the placeholder ledger id from initiate plus the collected part ETags
/// (the backend sorts ascending and CompleteMultipartUpload's them).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CompleteMultipartRequest {
    #[serde(rename = "objectId")]
    pub object_id: String,
    pub parts: Vec<CompletedPart>,
}

/// `POST /v1/buckets/{bucketId}/objects/multipart/{uploadId}/abort` request
/// body: the placeholder ledger id to abort (frees the staged S3 parts).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AbortMultipartRequest {
    #[serde(rename = "objectId")]
    pub object_id: String,
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

/// `POST /v1/buckets/{bucketId}/objects/batch-delete` request body: the storage
/// object ids to soft-delete (1..1000) in a single call.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BatchDeleteBucketObjectsRequest {
    #[serde(rename = "objectIds")]
    pub object_ids: Vec<String>,
}

/// `POST /v1/messages/batch-delete` request body: the message ids to delete
/// (1..1000) in a single call.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BatchDeleteMessagesRequest {
    #[serde(rename = "messageIds")]
    pub message_ids: Vec<String>,
}

/// One per-id failure from a batch-delete call. The id was skipped (e.g. not
/// found or not owned by the caller) and never aborts the rest of the batch.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BatchDeleteFailure {
    pub id: String,
    pub error: String,
}

/// Partial-success result of a batch-delete call (`object: "batch_delete_result"`),
/// shared by the bucket-objects and messages batch-delete endpoints. `deleted`
/// holds the ids that were removed; `failed` holds the per-id failures.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BatchDeleteResult {
    #[serde(default)]
    pub object: Option<String>,
    #[serde(default)]
    pub deleted: Vec<String>,
    #[serde(default)]
    pub failed: Vec<BatchDeleteFailure>,
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
    /// Channel filter (`email`, `a2a`, ...). Added by the channel-agnostic
    /// redesign; `channel=a2a` folds in the former `/v1/a2a/messages` surface.
    pub channel: Option<String>,
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

/// Query for `GET /v1/audit-logs/export`, the tamper-evident hash-chained
/// ledger export. All fields are optional filters: `from`/`to` bound the export
/// window (RFC-3339 timestamps), `after` resumes strictly after a sequence
/// number, and `format` selects `json` (default) or `ndjson`. Only the present
/// fields are sent.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct AuditExportQuery {
    pub from: Option<String>,
    pub to: Option<String>,
    pub after: Option<i64>,
    pub format: Option<String>,
}

/// Query for `GET /v1/telegram/voices`, the public Telegram TTS voice catalog.
/// `language` and `q` are free-text filters, `featured` restricts to the
/// curated set, and `limit`/`offset` page the catalog (server clamps
/// `limit` to 1..=100). Only the present fields are sent.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct TelegramVoicesQuery {
    pub language: Option<String>,
    pub q: Option<String>,
    pub featured: bool,
    pub limit: Option<u32>,
    pub offset: Option<u32>,
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
    pub outbound_message_id: Option<String>,
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
    /// Delivery channel this message rode (`email`, `a2a`, ...). Added by the
    /// channel-agnostic redesign; defaulted so older payloads still deserialize.
    #[serde(default)]
    pub channel: String,
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
    /// Channel-specific metadata. For outbound (folded from the old
    /// `OutboundMessage`) this carries `providerMessageId`, `provider`,
    /// `lastEventType`, `lastEventAt`, `bouncedAt`, `complainedAt`; for a2a it
    /// carries `receiptId`/`provenance`. Passed through verbatim.
    #[serde(default, rename = "channelMetadata")]
    pub channel_metadata: Option<serde_json::Value>,
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

// ---------------------------------------------------------------------------
// Dairo Phone (/v1/phone/*): outbound AI calls + phone-number provisioning
// ---------------------------------------------------------------------------

/// One provisioned phone number (`object: "phone_number"`). Unlike the rest of
/// the surface, number rows are served snake_case straight from the store, so
/// the Rust field names match the wire without renames.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PhoneNumber {
    #[serde(default = "default_phone_number_object")]
    pub object: String,
    pub id: String,
    pub phone_number: String,
    #[serde(default)]
    pub country_code: Option<String>,
    #[serde(default)]
    pub number_type: Option<String>,
    #[serde(default)]
    pub capabilities: Option<Vec<String>>,
    /// `active | pending | releasing | released | failed`. `pending` is a
    /// success: some countries (e.g. DE) hold the number until regulatory
    /// documents clear — see `requirements`.
    pub status: String,
    #[serde(default)]
    pub inbox_id: Option<String>,
    #[serde(default)]
    pub agent_id: Option<String>,
    #[serde(default)]
    pub monthly_cost_usd: Option<f64>,
    #[serde(default)]
    pub setup_cost_usd: Option<f64>,
    /// Outstanding regulatory requirements (`{ status, items }`).
    #[serde(default)]
    pub requirements: serde_json::Value,
    #[serde(default)]
    pub metadata: serde_json::Value,
    #[serde(default)]
    pub purchased_at: Option<String>,
    #[serde(default)]
    pub released_at: Option<String>,
    #[serde(default)]
    pub created_at: Option<String>,
}

fn default_phone_number_object() -> String {
    "phone_number".to_string()
}

/// One provider-inventory row from `GET /v1/phone/numbers/available`
/// (`object: "phone_number.available"`). `purchasable: false` plus a masked
/// `phoneNumber` means the provider account is not verified enough to buy the
/// number yet — the contract requires surfacing those rows, never hiding them.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AvailablePhoneNumber {
    #[serde(default = "default_available_phone_number_object")]
    pub object: String,
    #[serde(rename = "phoneNumber")]
    pub phone_number: String,
    #[serde(default, rename = "countryCode")]
    pub country_code: Option<String>,
    #[serde(default, rename = "numberType")]
    pub number_type: Option<String>,
    #[serde(default)]
    pub locality: Option<String>,
    #[serde(default)]
    pub region: Option<String>,
    #[serde(default)]
    pub capabilities: Option<Vec<String>>,
    #[serde(default, rename = "monthlyCostUsd")]
    pub monthly_cost_usd: Option<f64>,
    #[serde(default, rename = "setupCostUsd")]
    pub setup_cost_usd: Option<f64>,
    pub purchasable: bool,
}

fn default_available_phone_number_object() -> String {
    "phone_number.available".to_string()
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BuyPhoneNumberRequest {
    /// The exact E.164 number a prior availability search returned.
    #[serde(rename = "phoneNumber")]
    pub phone_number: String,
}

/// Body for `POST /v1/phone/calls`. Only `to`, `from`, and `instructions` are
/// required; every omitted optional falls back to its server default.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CreatePhoneCallRequest {
    pub to: String,
    /// Caller id. Must be an active number the account owns.
    pub from: String,
    pub instructions: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub greeting: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub voice: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub language: Option<String>,
    /// `silence | office | none` (there is no `cafe`).
    #[serde(rename = "backgroundAudio", skip_serializing_if = "Option::is_none")]
    pub background_audio: Option<String>,
    #[serde(rename = "maxDurationSeconds", skip_serializing_if = "Option::is_none")]
    pub max_duration_seconds: Option<u32>,
    /// Recording defaults to on server-side; only sent when opting out.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub record: Option<bool>,
    /// `{{handlebars}}` variables substituted into instructions/greeting.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub variables: Option<serde_json::Value>,
    #[serde(rename = "webhookUrl", skip_serializing_if = "Option::is_none")]
    pub webhook_url: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub metadata: Option<serde_json::Value>,
    #[serde(rename = "idempotencyKey", skip_serializing_if = "Option::is_none")]
    pub idempotency_key: Option<String>,
}

/// One outbound call (`object: "phone_call"`). Served snake_case from the
/// store, like [`PhoneNumber`].
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PhoneCall {
    #[serde(default = "default_phone_call_object")]
    pub object: String,
    pub id: String,
    #[serde(default)]
    pub direction: String,
    pub status: String,
    pub from_number: String,
    pub to_number: String,
    #[serde(default)]
    pub instructions: Option<String>,
    #[serde(default)]
    pub greeting: Option<String>,
    #[serde(default)]
    pub voice: Option<String>,
    #[serde(default)]
    pub model: Option<String>,
    #[serde(default)]
    pub language: Option<String>,
    #[serde(default)]
    pub background_audio: Option<String>,
    #[serde(default)]
    pub max_duration_seconds: Option<u32>,
    #[serde(default)]
    pub queued_at: Option<String>,
    #[serde(default)]
    pub started_at: Option<String>,
    #[serde(default)]
    pub answered_at: Option<String>,
    #[serde(default)]
    pub ended_at: Option<String>,
    #[serde(default)]
    pub duration_seconds: Option<i64>,
    #[serde(default)]
    pub hangup_cause: Option<String>,
    #[serde(default)]
    pub answered_by: Option<String>,
    #[serde(default)]
    pub summary: Option<String>,
    #[serde(default)]
    pub cost_usd: Option<f64>,
    #[serde(default)]
    pub error: Option<String>,
    #[serde(default)]
    pub webhook_url: Option<String>,
    #[serde(default)]
    pub metadata: serde_json::Value,
    #[serde(default)]
    pub idempotency_key: Option<String>,
    #[serde(default)]
    pub created_at: Option<String>,
}

fn default_phone_call_object() -> String {
    "phone_call".to_string()
}

impl PhoneCall {
    /// Whether the call reached one of the contract's terminal statuses, i.e.
    /// polling it further can never observe another transition.
    pub fn is_terminal(&self) -> bool {
        matches!(
            self.status.as_str(),
            "completed" | "failed" | "no_answer" | "busy" | "canceled"
        )
    }
}

/// `GET /v1/phone/calls/{id}/transcript`. `turns` is empty (and `status`
/// carries the live call status) while the call is still in flight.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PhoneCallTranscript {
    #[serde(default = "default_phone_call_transcript_object")]
    pub object: String,
    #[serde(rename = "callId")]
    pub call_id: String,
    #[serde(default)]
    pub turns: Vec<PhoneCallTranscriptTurn>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status: Option<String>,
}

fn default_phone_call_transcript_object() -> String {
    "phone_call.transcript".to_string()
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PhoneCallTranscriptTurn {
    /// `agent | caller | tool`.
    #[serde(default)]
    pub role: String,
    #[serde(default)]
    pub content: String,
    #[serde(default)]
    pub timestamp: Option<String>,
}

/// `GET /v1/phone/calls/{id}/recording`: the Dairo storage object holding the
/// call audio (the API never exposes a raw provider URL).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PhoneCallRecording {
    #[serde(default = "default_phone_call_recording_object")]
    pub object: String,
    #[serde(rename = "callId")]
    pub call_id: String,
    #[serde(rename = "objectId")]
    pub object_id: String,
}

fn default_phone_call_recording_object() -> String {
    "phone_call.recording".to_string()
}

// ---------------------------------------------------------------------------
// Slack (/v1/slack/*): "Add to Slack" install-URL minting
// ---------------------------------------------------------------------------

/// Response for `POST /v1/slack/oauth/start`: the signed Slack "Add to Slack"
/// install URL. Embed it as an "Add to Slack" button — when the customer
/// approves the install, the OAuth callback binds their workspace to this
/// account as a `channel: "slack"` inbox and inbound @mentions/DMs then fire the
/// existing `message.received` webhook. The signed state expires after 10
/// minutes, so mint one per click.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SlackOauthStart {
    /// `https://slack.com/oauth/v2/authorize` URL carrying the app's bot scopes
    /// and a signed, single-use state (10-minute expiry).
    pub url: String,
}

/// Query for `GET /v1/phone/numbers/available`. All filters are optional; the
/// server defaults `country` to `US` and clamps `limit` to 1..=100 (default 10).
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct AvailablePhoneNumbersQuery {
    pub country: Option<String>,
    pub area_code: Option<String>,
    pub contains: Option<String>,
    /// `local | toll_free | national | mobile`.
    pub number_type: Option<String>,
    pub limit: Option<u32>,
}

/// Query for `GET /v1/phone/calls`. `limit` is clamped server-side to 1..=100
/// (default 50).
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct PhoneCallListQuery {
    pub status: Option<String>,
    pub to: Option<String>,
    pub limit: Option<u32>,
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
    if let Some(value) = &query.channel {
        pairs.append_pair("channel", value);
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

pub(crate) fn apply_available_phone_numbers_query(
    url: &mut Url,
    query: &AvailablePhoneNumbersQuery,
) {
    let mut pairs = url.query_pairs_mut();
    if let Some(value) = &query.country {
        pairs.append_pair("country", value);
    }
    if let Some(value) = &query.area_code {
        pairs.append_pair("areaCode", value);
    }
    if let Some(value) = &query.contains {
        pairs.append_pair("contains", value);
    }
    if let Some(value) = &query.number_type {
        pairs.append_pair("type", value);
    }
    if let Some(value) = query.limit {
        pairs.append_pair("limit", &value.to_string());
    }
}

pub(crate) fn apply_phone_call_query(url: &mut Url, query: &PhoneCallListQuery) {
    let mut pairs = url.query_pairs_mut();
    if let Some(value) = &query.status {
        pairs.append_pair("status", value);
    }
    if let Some(value) = &query.to {
        pairs.append_pair("to", value);
    }
    if let Some(value) = query.limit {
        pairs.append_pair("limit", &value.to_string());
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
