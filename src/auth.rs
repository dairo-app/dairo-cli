//! Browser-based OAuth 2.0 (PKCE) login for the Dairo CLI.
//!
//! `dairo login` runs the exact same Authorization-Code + PKCE flow the hosted
//! MCP OAuth clients use (see the backend `mcp/oauth.rs` contract). The
//! authorization server lives on the MCP host (`https://mcp.dairo.app` by
//! default — NOT the `/v1` API host):
//!
//! 1. Bind a loopback (`127.0.0.1:0`) listener FIRST and learn the port, so the
//!    `redirect_uri` is `http://127.0.0.1:<port>/callback`.
//! 2. Dynamic Client Registration (`POST /oauth/register`) with that redirect to
//!    obtain a `client_id`.
//! 3. Open the system browser to `GET {base}/oauth/authorize?...` (which 302s to
//!    the dashboard authorize page) and also print the URL for manual paste.
//! 4. Accept exactly one callback connection, validate `state` (CSRF), read the
//!    `code`, and serve a small success page.
//! 5. Exchange the code at `POST /oauth/token` (form-encoded) for a
//!    `dairo_live_*` API key, which is stored in the existing config `api_key`
//!    field so every existing command keeps working unchanged.
//!
//! The token value is never printed, logged, or written anywhere except the
//! atomic `0600` config file.

use std::io::{BufRead, BufReader, Read, Write};
use std::net::{Ipv4Addr, SocketAddr, TcpListener, TcpStream};
use std::path::Path;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use anyhow::{anyhow, bail, Context, Result};
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine as _;
use rand::RngCore;
use sha2::{Digest, Sha256};
use url::Url;

use crate::config::Config;

/// The human-friendly identity this client presents on the consent screen,
/// sent as `client_name` in the Dynamic Client Registration body (not on the
/// `/authorize` URL — the backend only trusts what it learns directly from the
/// registering process, since anything on `/authorize` is a query string
/// anyone could put in a link). Without it the consent page would fall back to
/// "Dairo MCP", humanized from the DCR-issued generic `dairo-mcp-*` client_id.
const CLIENT_NAME: &str = "Dairo CLI";

/// Default scope set for `dairo login`. The backend accepts the `admin`
/// convenience bundle (see `api_key_scopes.rs::expand_bundle`), which expands to
/// the full enforced scope matrix — so the resulting key can drive every CLI
/// command.
pub const DEFAULT_LOGIN_SCOPE: &str = "admin";

/// Overall budget for the whole interactive flow waiting on the browser
/// callback. Generous enough for a human to sign in, bounded so a never-arriving
/// callback cannot hang the CLI forever.
const CALLBACK_TIMEOUT: Duration = Duration::from_secs(300);

/// Cap on the bytes we read from a single inbound HTTP request line + headers.
/// The callback is a tiny GET; this bounds a misbehaving/hostile local client.
const MAX_REQUEST_BYTES: usize = 16 * 1024;

/// Outcome of a successful login, used by the caller to print a summary.
pub struct LoginOutcome {
    pub scopes: Vec<String>,
    pub config_path: std::path::PathBuf,
}

/// Runs the browser OAuth login end to end and persists the resulting token.
///
/// `base_url` is the OAuth authorization-server base (`https://mcp.dairo.app`
/// by default; see `resolve_mcp_base_url` in `main.rs`); `scope` is the
/// space-or-comma list of requested scopes (defaults applied by the caller).
pub async fn login(base_url: &str, scope: &str, config_path: &Path) -> Result<LoginOutcome> {
    let base =
        Url::parse(base_url).with_context(|| format!("invalid OAuth base URL: {base_url}"))?;

    // Transport guard, mirroring the bearer-key path (`require_secure_base_url`):
    // the OAuth legs carry the PKCE verifier and return a freshly minted key, so
    // refuse a plaintext non-loopback base URL. http:// is allowed only for a
    // local dev backend (localhost / 127.0.0.1 / ::1).
    let host = base.host_str().unwrap_or_default();
    let is_loopback = matches!(host, "localhost" | "127.0.0.1" | "::1");
    if base.scheme() != "https" && !is_loopback {
        bail!(
            "refusing to run OAuth login over an insecure URL ({base_url}); \
             use https:// (http:// is only allowed for a localhost backend)"
        );
    }

    // 1. Bind the loopback listener FIRST so we know the redirect port. Bind ONLY
    //    to 127.0.0.1 — never 0.0.0.0 — so no other host can reach the callback.
    let listener = TcpListener::bind(SocketAddr::from((Ipv4Addr::LOCALHOST, 0)))
        .context("failed to bind a local 127.0.0.1 callback listener for OAuth login")?;
    let port = listener
        .local_addr()
        .context("failed to read the local callback listener port")?
        .port();
    let redirect_uri = format!("http://127.0.0.1:{port}/callback");

    let scopes = normalize_scope_arg(scope);
    anyhow::ensure!(
        !scopes.is_empty(),
        "login requires at least one --scope (or use the default)"
    );
    let scope_param = scopes.join(" ");

    // 2. PKCE: 64 random bytes -> base64url-no-pad verifier (96 chars, within the
    //    43..=128 RFC 7636 range), challenge = base64url-no-pad(sha256(verifier)).
    let pkce = Pkce::generate();

    // 4. CSRF state: 32 random url-safe bytes.
    let state = random_urlsafe(32);

    // 3. Dynamic Client Registration to learn the client_id the server expects.
    let http = reqwest::Client::builder()
        .user_agent(concat!("dairo-cli/", env!("CARGO_PKG_VERSION")))
        .timeout(Duration::from_secs(30))
        .build()
        .context("failed to build OAuth HTTP client")?;
    let client_id = register_client(&http, &base, &redirect_uri).await?;

    // 5. Build the authorize URL and open the browser (also printed for manual
    //    paste). The backend 302s this to the dashboard authorize page.
    let authorize_url = build_authorize_url(
        &base,
        &client_id,
        &redirect_uri,
        &pkce.challenge,
        &scope_param,
        &state,
    )?;

    println!("Opening your browser to sign in to Dairo...");
    println!(
        "If it does not open automatically, paste this URL into your browser:\n  {authorize_url}\n"
    );
    // A failure to launch the browser is non-fatal: the user can paste the URL.
    if webbrowser::open(authorize_url.as_str()).is_err() {
        eprintln!("(could not launch a browser automatically; use the URL above)");
    }
    println!("Waiting for the sign-in to complete (up to 5 minutes)...");

    // 6. Accept exactly one callback, validate state, extract the code.
    let code = wait_for_callback(listener, &state)?;

    // 7. Exchange the authorization code for the access token (form-encoded).
    let token = exchange_code(
        &http,
        &base,
        &code,
        &pkce.verifier,
        &redirect_uri,
        &client_id,
    )
    .await?;

    // 8. Persist. Keep using the existing `api_key` field so every command keeps
    //    working; record the oauth provenance metadata alongside it. The token
    //    value is never printed.
    let mut config = Config::load_from_path(config_path)?;
    let granted_scopes = token.scopes();
    config.api_key = Some(token.access_token);
    config.auth_method = Some("oauth".to_string());
    config.scopes = if granted_scopes.is_empty() {
        Some(scopes.clone())
    } else {
        Some(granted_scopes.clone())
    };
    config.obtained_at = Some(now_rfc3339());
    config.save_to_path(config_path)?;

    Ok(LoginOutcome {
        scopes: config.scopes.clone().unwrap_or(scopes),
        config_path: config_path.to_path_buf(),
    })
}

/// PKCE verifier/challenge pair.
struct Pkce {
    verifier: String,
    challenge: String,
}

impl Pkce {
    fn generate() -> Self {
        // 64 random bytes -> base64url-no-pad is 86 chars, well within the
        // 43..=128 length window and using only the [A-Za-z0-9-_] alphabet.
        let verifier = random_urlsafe(64);
        let challenge = URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes()));
        Self {
            verifier,
            challenge,
        }
    }
}

/// Fills `len` bytes of OS entropy and encodes them base64url-no-pad, yielding a
/// URL-safe token using only `[A-Za-z0-9-_]`.
fn random_urlsafe(len: usize) -> String {
    let mut bytes = vec![0u8; len];
    rand::rng().fill_bytes(&mut bytes);
    URL_SAFE_NO_PAD.encode(bytes)
}

/// Normalizes a `--scope` argument: scopes may be space- or comma-separated;
/// blanks are dropped and duplicates removed (order preserved).
fn normalize_scope_arg(scope: &str) -> Vec<String> {
    let mut seen = Vec::new();
    for token in scope.split([' ', ',', '\t', '\n']) {
        let token = token.trim();
        if token.is_empty() {
            continue;
        }
        if !seen.iter().any(|existing: &String| existing == token) {
            seen.push(token.to_string());
        }
    }
    seen
}

/// Builds a `{base}/oauth/{segment}` URL, preserving any base path prefix.
fn oauth_endpoint(base: &Url, segment: &str) -> Result<Url> {
    let mut url = base.clone();
    {
        let mut segments = url
            .path_segments_mut()
            .map_err(|_| anyhow!("OAuth base URL cannot be a base: {base}"))?;
        segments.pop_if_empty();
        segments.push("oauth");
        segments.push(segment);
    }
    Ok(url)
}

/// Dynamic Client Registration: `POST /oauth/register` with our loopback
/// redirect, returning the issued `client_id`.
async fn register_client(
    client: &reqwest::Client,
    base: &Url,
    redirect_uri: &str,
) -> Result<String> {
    let endpoint = oauth_endpoint(base, "register")?;
    let body = serde_json::json!({
        "redirect_uris": [redirect_uri],
        "grant_types": ["authorization_code"],
        "response_types": ["code"],
        "token_endpoint_auth_method": "none",
        "client_name": CLIENT_NAME,
    });
    let response = client
        .post(endpoint)
        .header("Accept", "application/json")
        .json(&body)
        .send()
        .await
        .context("OAuth client registration request failed")?;
    let status = response.status();
    let value: serde_json::Value = response
        .json()
        .await
        .context("OAuth client registration returned a non-JSON response")?;
    if !status.is_success() {
        bail!(
            "OAuth client registration failed ({status}): {}",
            oauth_error_message(&value)
        );
    }
    value
        .get("client_id")
        .and_then(serde_json::Value::as_str)
        .filter(|id| !id.is_empty())
        .map(str::to_string)
        .context("OAuth client registration response did not contain a client_id")
}

/// Builds the authorize URL the browser is sent to.
fn build_authorize_url(
    base: &Url,
    client_id: &str,
    redirect_uri: &str,
    code_challenge: &str,
    scope: &str,
    state: &str,
) -> Result<Url> {
    let mut url = oauth_endpoint(base, "authorize")?;
    url.query_pairs_mut()
        .append_pair("response_type", "code")
        .append_pair("client_id", client_id)
        .append_pair("redirect_uri", redirect_uri)
        .append_pair("code_challenge", code_challenge)
        .append_pair("code_challenge_method", "S256")
        .append_pair("scope", scope)
        .append_pair("state", state);
    Ok(url)
}

/// Token-exchange result. `expires_in` is advisory; the access token is the
/// `dairo_live_*` API key.
struct TokenResponse {
    access_token: String,
    scope: Option<String>,
}

impl TokenResponse {
    fn scopes(&self) -> Vec<String> {
        self.scope
            .as_deref()
            .map(|s| s.split_whitespace().map(str::to_string).collect::<Vec<_>>())
            .unwrap_or_default()
    }
}

/// `POST /oauth/token` (application/x-www-form-urlencoded) per the backend
/// contract. `client_id` is included for spec-completeness even though the
/// backend ignores it at exchange time; the backend reads `grant_type`, `code`,
/// `code_verifier`, and `redirect_uri`.
async fn exchange_code(
    client: &reqwest::Client,
    base: &Url,
    code: &str,
    code_verifier: &str,
    redirect_uri: &str,
    client_id: &str,
) -> Result<TokenResponse> {
    let endpoint = oauth_endpoint(base, "token")?;
    let form = [
        ("grant_type", "authorization_code"),
        ("code", code),
        ("code_verifier", code_verifier),
        ("redirect_uri", redirect_uri),
        ("client_id", client_id),
    ];
    let response = client
        .post(endpoint)
        .header("Accept", "application/json")
        .form(&form)
        .send()
        .await
        .context("OAuth token exchange request failed")?;
    let status = response.status();
    let value: serde_json::Value = response
        .json()
        .await
        .context("OAuth token exchange returned a non-JSON response")?;
    if !status.is_success() {
        bail!(
            "OAuth token exchange failed ({status}): {}",
            oauth_error_message(&value)
        );
    }
    let access_token = value
        .get("access_token")
        .and_then(serde_json::Value::as_str)
        .filter(|token| !token.is_empty())
        .map(str::to_string)
        .context("OAuth token exchange response did not contain an access_token")?;
    let scope = value
        .get("scope")
        .and_then(serde_json::Value::as_str)
        .filter(|scope| !scope.trim().is_empty())
        .map(str::to_string);
    Ok(TokenResponse {
        access_token,
        scope,
    })
}

/// Pulls a human-readable message out of an OAuth/Dairo error body without ever
/// echoing a token. Handles both the Dairo `{error:{message}}` envelope and the
/// RFC 6749 `{error, error_description}` shape.
fn oauth_error_message(value: &serde_json::Value) -> String {
    if let Some(message) = value
        .get("error")
        .and_then(|e| e.get("message"))
        .and_then(serde_json::Value::as_str)
    {
        return message.to_string();
    }
    if let Some(description) = value
        .get("error_description")
        .and_then(serde_json::Value::as_str)
    {
        return description.to_string();
    }
    if let Some(error) = value.get("error").and_then(serde_json::Value::as_str) {
        return error.to_string();
    }
    "the server returned an error".to_string()
}

/// Accepts exactly one callback connection within the timeout window, validates
/// `state`, and returns the authorization `code`. Always serves a tidy HTML page
/// back to the browser (success or error) before returning.
fn wait_for_callback(listener: TcpListener, expected_state: &str) -> Result<String> {
    // A short accept timeout lets us poll the overall deadline cleanly.
    listener
        .set_nonblocking(false)
        .context("failed to configure the callback listener")?;
    let deadline = Instant::now() + CALLBACK_TIMEOUT;

    loop {
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            bail!(
                "timed out after 5 minutes waiting for the browser sign-in callback; \
                 re-run `dairo login`, or set a token manually with `dairo auth token set`"
            );
        }
        // Bound each accept so we re-check the overall deadline even if no
        // connection arrives.
        listener
            .set_nonblocking(true)
            .context("failed to configure the callback listener")?;
        match listener.accept() {
            Ok((stream, _peer)) => {
                stream
                    .set_nonblocking(false)
                    .context("failed to configure the callback connection")?;
                return handle_callback_connection(stream, expected_state);
            }
            Err(ref e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                std::thread::sleep(Duration::from_millis(100));
                continue;
            }
            Err(e) => return Err(e).context("failed to accept the browser callback connection"),
        }
    }
    // `listener` is dropped on return, releasing the port immediately.
}

/// Reads one HTTP request from `stream`, extracts `code`/`state`/`error` from the
/// request-line query, validates the CSRF `state`, replies with a success or
/// error page, and returns the `code`.
fn handle_callback_connection(mut stream: TcpStream, expected_state: &str) -> Result<String> {
    stream.set_read_timeout(Some(Duration::from_secs(10))).ok();

    let target = match read_request_target(&stream) {
        Ok(target) => target,
        Err(e) => {
            respond_error(&mut stream, "Could not read the sign-in callback.");
            return Err(e);
        }
    };

    let query = request_target_query(&target);
    let params = parse_query(&query);

    if let Some(error) = params
        .iter()
        .find(|(k, _)| k == "error")
        .map(|(_, v)| v.clone())
    {
        let description = params
            .iter()
            .find(|(k, _)| k == "error_description")
            .map(|(_, v)| v.clone())
            .unwrap_or_else(|| error.clone());
        respond_error(&mut stream, "Sign-in was not completed.");
        bail!("the authorization server returned an error: {error} ({description})");
    }

    let returned_state = params
        .iter()
        .find(|(k, _)| k == "state")
        .map(|(_, v)| v.clone())
        .unwrap_or_default();

    // CSRF guard: a mismatched (or missing) state aborts WITHOUT a token.
    if !constant_time_eq(returned_state.as_bytes(), expected_state.as_bytes()) {
        respond_error(
            &mut stream,
            "Sign-in could not be verified (state mismatch).",
        );
        bail!("OAuth state mismatch; aborting login without requesting a token (possible CSRF)");
    }

    let code = params
        .iter()
        .find(|(k, _)| k == "code")
        .map(|(_, v)| v.clone())
        .filter(|code| !code.is_empty());

    match code {
        Some(code) => {
            respond_success(&mut stream);
            Ok(code)
        }
        None => {
            respond_error(&mut stream, "Sign-in did not return an authorization code.");
            bail!("the callback did not include an authorization code")
        }
    }
}

/// Reads the HTTP request line + headers and returns the request target (the
/// path+query from the request line). Bounded by [`MAX_REQUEST_BYTES`].
fn read_request_target(stream: &TcpStream) -> Result<String> {
    let mut reader = BufReader::new(stream.try_clone().context("failed to read callback")?);
    let mut request_line = String::new();
    let mut limited = (&mut reader).take(MAX_REQUEST_BYTES as u64);
    limited
        .read_line(&mut request_line)
        .context("failed to read the callback request line")?;
    // Request line: "GET /callback?code=...&state=... HTTP/1.1"
    let target = request_line
        .split_whitespace()
        .nth(1)
        .context("malformed callback request line")?
        .to_string();
    Ok(target)
}

/// Extracts the query string (after `?`) from a request target.
fn request_target_query(target: &str) -> String {
    target
        .split_once('?')
        .map(|(_, query)| query.to_string())
        .unwrap_or_default()
}

/// Parses an `application/x-www-form-urlencoded` query into key/value pairs,
/// percent-decoding both sides.
fn parse_query(query: &str) -> Vec<(String, String)> {
    url::form_urlencoded::parse(query.as_bytes())
        .into_owned()
        .collect()
}

/// Constant-time byte comparison so the CSRF `state` check does not leak length
/// or content through timing.
fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    let mut diff = 0u8;
    for (x, y) in a.iter().zip(b.iter()) {
        diff |= x ^ y;
    }
    diff == 0
}

fn respond_success(stream: &mut TcpStream) {
    let html = success_page_html();
    write_http_response(stream, "200 OK", &html);
}

fn respond_error(stream: &mut TcpStream, detail: &str) {
    let html = error_page_html(detail);
    write_http_response(stream, "400 Bad Request", &html);
}

fn write_http_response(stream: &mut TcpStream, status: &str, body: &str) {
    let response = format!(
        "HTTP/1.1 {status}\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {len}\r\nConnection: close\r\n\r\n{body}",
        len = body.len(),
    );
    let _ = stream.write_all(response.as_bytes());
    let _ = stream.flush();
}

fn success_page_html() -> String {
    page_html(
        "Dairo CLI \u{2014} signed in",
        "Dairo CLI \u{2014} signed in",
        "You can close this tab and return to your terminal.",
        "#0f172a",
    )
}

fn error_page_html(detail: &str) -> String {
    page_html(
        "Dairo CLI \u{2014} sign-in failed",
        "Dairo CLI \u{2014} sign-in failed",
        detail,
        "#b91c1c",
    )
}

/// Minimal inline-styled HTML so the page renders standalone with no assets.
/// `detail` is server-controlled (our own constant strings or a sanitized error
/// label), never raw token material.
fn page_html(title: &str, heading: &str, message: &str, accent: &str) -> String {
    let heading = html_escape(heading);
    let message = html_escape(message);
    let title = html_escape(title);
    format!(
        "<!doctype html><html lang=\"en\"><head><meta charset=\"utf-8\">\
<meta name=\"viewport\" content=\"width=device-width, initial-scale=1\">\
<title>{title}</title></head>\
<body style=\"margin:0;min-height:100vh;display:flex;align-items:center;justify-content:center;\
background:#f8fafc;font-family:-apple-system,BlinkMacSystemFont,'Segoe UI',Roboto,Helvetica,Arial,sans-serif;color:#0f172a;\">\
<main style=\"max-width:28rem;padding:2.5rem;background:#ffffff;border-radius:16px;\
box-shadow:0 10px 30px rgba(15,23,42,0.08);text-align:center;\">\
<div style=\"width:48px;height:48px;margin:0 auto 1.25rem;border-radius:12px;background:{accent};\"></div>\
<h1 style=\"margin:0 0 0.5rem;font-size:1.25rem;line-height:1.4;\">{heading}</h1>\
<p style=\"margin:0;color:#475569;font-size:0.95rem;line-height:1.5;\">{message}</p>\
</main></body></html>"
    )
}

/// Escapes the five HTML-significant characters so server-controlled labels can
/// never break the page markup.
fn html_escape(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    for ch in input.chars() {
        match ch {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#39;"),
            _ => out.push(ch),
        }
    }
    out
}

/// Formats the current UTC time as an RFC3339 timestamp (`YYYY-MM-DDTHH:MM:SSZ`)
/// without pulling in a date/time crate. Used only for the `obtained_at`
/// provenance field on the stored config.
fn now_rfc3339() -> String {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default();
    let secs = now.as_secs();
    let days = secs / 86_400;
    let rem = secs % 86_400;
    let (hour, minute, second) = (rem / 3600, (rem % 3600) / 60, rem % 60);
    let (year, month, day) = civil_from_days(days as i64);
    format!("{year:04}-{month:02}-{day:02}T{hour:02}:{minute:02}:{second:02}Z")
}

/// Converts a count of days since the Unix epoch (1970-01-01) into a civil
/// (year, month, day) date. Uses Howard Hinnant's well-known `civil_from_days`
/// algorithm, valid for all dates we will ever stamp.
fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097; // [0, 146096]
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365; // [0, 399]
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100); // [0, 365]
    let mp = (5 * doy + 2) / 153; // [0, 11]
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32; // [1, 31]
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32; // [1, 12]
    let year = if m <= 2 { y + 1 } else { y };
    (year, m, d)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pkce_verifier_is_url_safe_and_within_length_bounds() {
        let pkce = Pkce::generate();
        assert!(
            (43..=128).contains(&pkce.verifier.len()),
            "verifier length {} out of RFC7636 bounds",
            pkce.verifier.len()
        );
        assert!(pkce
            .verifier
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_'));
        assert!(pkce
            .challenge
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_'));
    }

    #[test]
    fn pkce_challenge_is_sha256_of_verifier() {
        let pkce = Pkce::generate();
        let expected = URL_SAFE_NO_PAD.encode(Sha256::digest(pkce.verifier.as_bytes()));
        assert_eq!(pkce.challenge, expected);
    }

    #[test]
    fn pkce_pairs_are_unique_per_invocation() {
        let a = Pkce::generate();
        let b = Pkce::generate();
        assert_ne!(a.verifier, b.verifier);
        assert_ne!(a.challenge, b.challenge);
    }

    #[test]
    fn normalizes_space_and_comma_separated_scopes() {
        assert_eq!(
            normalize_scope_arg("messages:read, messages:send  webhooks:write"),
            vec!["messages:read", "messages:send", "webhooks:write"]
        );
        assert_eq!(normalize_scope_arg("admin"), vec!["admin"]);
        assert_eq!(normalize_scope_arg("a a a"), vec!["a"]);
        assert!(normalize_scope_arg("   ").is_empty());
    }

    #[test]
    fn builds_authorize_url_with_all_pkce_params() {
        let base = Url::parse("https://mcp.dairo.app").unwrap();
        let url = build_authorize_url(
            &base,
            "dairo-mcp-abc",
            "http://127.0.0.1:54321/callback",
            "challenge123",
            "messages:read messages:send",
            "state-xyz",
        )
        .unwrap();

        assert_eq!(url.path(), "/oauth/authorize");
        let pairs: std::collections::HashMap<_, _> = url.query_pairs().into_owned().collect();
        assert_eq!(pairs.get("response_type").map(String::as_str), Some("code"));
        assert_eq!(
            pairs.get("client_id").map(String::as_str),
            Some("dairo-mcp-abc")
        );
        // client_name is NOT on the authorize URL — it travels only in the DCR
        // body (see `register_client`), which the backend trusts because it
        // comes directly from this process rather than a shareable link.
        assert!(!pairs.contains_key("client_name"));
        assert_eq!(
            pairs.get("redirect_uri").map(String::as_str),
            Some("http://127.0.0.1:54321/callback")
        );
        assert_eq!(
            pairs.get("code_challenge").map(String::as_str),
            Some("challenge123")
        );
        assert_eq!(
            pairs.get("code_challenge_method").map(String::as_str),
            Some("S256")
        );
        assert_eq!(
            pairs.get("scope").map(String::as_str),
            Some("messages:read messages:send")
        );
        assert_eq!(pairs.get("state").map(String::as_str), Some("state-xyz"));
    }

    #[test]
    fn oauth_endpoint_preserves_base_path_prefix() {
        let base = Url::parse("https://api.example.test/root").unwrap();
        let url = oauth_endpoint(&base, "token").unwrap();
        assert_eq!(url.as_str(), "https://api.example.test/root/oauth/token");
    }

    #[test]
    fn parses_callback_query_and_target() {
        let target = "/callback?code=abc123&state=xyz%20789";
        assert_eq!(request_target_query(target), "code=abc123&state=xyz%20789");
        let params = parse_query(&request_target_query(target));
        assert_eq!(
            params,
            vec![
                ("code".to_string(), "abc123".to_string()),
                ("state".to_string(), "xyz 789".to_string()),
            ]
        );
    }

    #[test]
    fn target_without_query_yields_empty_params() {
        assert_eq!(request_target_query("/callback"), "");
        assert!(parse_query("").is_empty());
    }

    #[test]
    fn constant_time_eq_matches_only_identical_bytes() {
        assert!(constant_time_eq(b"state-abc", b"state-abc"));
        assert!(!constant_time_eq(b"state-abc", b"state-abd"));
        assert!(!constant_time_eq(b"state-abc", b"state-ab"));
        assert!(!constant_time_eq(b"", b"x"));
    }

    #[test]
    fn token_response_splits_scope_field() {
        let token = TokenResponse {
            access_token: "secret".to_string(),
            scope: Some("messages:read webhooks:write".to_string()),
        };
        assert_eq!(token.scopes(), vec!["messages:read", "webhooks:write"]);

        let none = TokenResponse {
            access_token: "secret".to_string(),
            scope: None,
        };
        assert!(none.scopes().is_empty());
    }

    #[test]
    fn oauth_error_message_reads_dairo_and_rfc_shapes() {
        let dairo = serde_json::json!({ "error": { "message": "Invalid OAuth code" } });
        assert_eq!(oauth_error_message(&dairo), "Invalid OAuth code");

        let rfc = serde_json::json!({ "error": "invalid_grant", "error_description": "expired" });
        assert_eq!(oauth_error_message(&rfc), "expired");

        let bare = serde_json::json!({ "error": "invalid_grant" });
        assert_eq!(oauth_error_message(&bare), "invalid_grant");

        let empty = serde_json::json!({});
        assert_eq!(oauth_error_message(&empty), "the server returned an error");
    }

    #[test]
    fn html_escape_neutralizes_markup() {
        assert_eq!(
            html_escape("<b>&\"'</b>"),
            "&lt;b&gt;&amp;&quot;&#39;&lt;/b&gt;"
        );
    }

    #[test]
    fn now_rfc3339_has_expected_shape() {
        let stamp = now_rfc3339();
        // YYYY-MM-DDTHH:MM:SSZ
        assert_eq!(stamp.len(), 20);
        assert!(stamp.ends_with('Z'));
        assert_eq!(&stamp[4..5], "-");
        assert_eq!(&stamp[10..11], "T");
    }

    #[test]
    fn civil_from_days_matches_known_dates() {
        assert_eq!(civil_from_days(0), (1970, 1, 1));
        // 2000-01-01 is 10957 days after the epoch.
        assert_eq!(civil_from_days(10_957), (2000, 1, 1));
        // 2026-06-20 is 20624 days after the epoch.
        assert_eq!(civil_from_days(20_624), (2026, 6, 20));
    }
}
