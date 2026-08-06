//! RFC 8628 Device Authorization Grant login: `dairo login --device-code`.
//!
//! For headless machines (SSH sessions, containers, CI) where the browser
//! PKCE flow cannot work — there is no local browser, and a browser on
//! another machine could never reach the CLI's `127.0.0.1` callback.
//! Modeled on `gh auth login`:
//!
//! 1. Dynamic Client Registration (`POST /oauth/register`, shared with the
//!    browser flow) to obtain a `client_id` with the trusted "Dairo CLI"
//!    display name.
//! 2. `POST /oauth/device/code` mints a high-entropy `device_code` (held by
//!    this process, never shown) and a short `user_code` (shown to the user).
//! 3. The user opens `https://platform.dairo.app/activate` on ANY device,
//!    enters the code, and approves on the standard consent screen.
//! 4. This process polls `POST /oauth/token` with the RFC 8628 grant URN,
//!    honoring `authorization_pending` / `slow_down` / `expired_token` /
//!    `access_denied`, until approval yields a `dairo_live_*` API key that is
//!    persisted exactly like the browser flow's.

use std::path::Path;
use std::time::{Duration, Instant};

use anyhow::{bail, Context, Result};
use serde_json::Value;
use url::Url;

use crate::config::Config;

use super::{
    normalize_scope_arg, now_rfc3339, oauth_endpoint, oauth_error_message, register_client,
    require_secure_oauth_base, token_from_success_body, LoginOutcome,
};

/// The RFC 8628 grant type URN sent on every token poll.
const DEVICE_GRANT_TYPE: &str = "urn:ietf:params:oauth:grant-type:device_code";

/// Fallbacks when the server response omits the optional fields.
const DEFAULT_EXPIRES_IN_SECONDS: u64 = 900;
const DEFAULT_POLL_INTERVAL_SECONDS: u64 = 5;

/// Hard ceiling on the server-suggested code lifetime so a hostile/buggy
/// response can never park the CLI in a day-long poll loop.
const MAX_EXPIRES_IN_SECONDS: u64 = 1800;

/// Runs the device-code login end to end and persists the resulting token.
/// Same contract as [`super::login`]; used directly by `--device-code` and as
/// the automatic fallback when no browser can be launched.
pub async fn login_device(base_url: &str, scope: &str, config_path: &Path) -> Result<LoginOutcome> {
    let base = require_secure_oauth_base(base_url)?;

    let scopes = normalize_scope_arg(scope);
    anyhow::ensure!(
        !scopes.is_empty(),
        "login requires at least one --scope (or use the default)"
    );
    let scope_param = scopes.join(" ");

    let http = reqwest::Client::builder()
        .user_agent(concat!("dairo-cli/", env!("CARGO_PKG_VERSION")))
        .timeout(Duration::from_secs(30))
        .build()
        .context("failed to build OAuth HTTP client")?;

    // DCR with the device grant type (no redirect URI — this flow has none).
    let client_id = register_client(&http, &base, None, &[DEVICE_GRANT_TYPE]).await?;

    let authorization =
        request_device_authorization(&http, &base, &client_id, &scope_param).await?;

    // gh-style UX: the code is the ONE thing the user must carry to the other
    // device, so it goes first and stands alone on its line.
    println!();
    println!(
        "First, copy your one-time code: {}",
        authorization.user_code
    );
    println!();
    println!("Then open this page on any device and enter the code:");
    println!("  {}", authorization.verification_uri);
    println!();
    // Best-effort convenience ONLY on a machine that plainly has a desktop
    // browser (an explicit `--device-code` on a laptop). Never in a headless
    // environment: with BROWSER=w3m/lynx set — common on exactly the SSH boxes
    // this flow targets — `webbrowser::open` runs the text browser in the
    // FOREGROUND and blocks until the user quits it, so the poll loop would
    // never start. The URL is already printed either way.
    if !is_headless_environment() && !browser_env_is_text_browser() {
        if let Some(complete) = authorization
            .verification_uri_complete
            .as_deref()
            .filter(|uri| is_launchable_verification_uri(uri, &base))
        {
            let _ = webbrowser::open(complete);
        }
    }
    let expires_in = authorization.expires_in.clamp(60, MAX_EXPIRES_IN_SECONDS);
    println!(
        "Waiting for approval... (code expires in {} minutes)",
        expires_in.div_ceil(60)
    );

    let token = poll_for_token(
        &http,
        &base,
        &authorization.device_code,
        &client_id,
        authorization.interval,
        expires_in,
    )
    .await?;

    // Persist exactly like the browser flow: same config fields, token never
    // printed.
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
        account_email: token.account_email,
    })
}

/// The RFC 8628 §3.2 device-authorization response.
struct DeviceAuthorization {
    device_code: String,
    user_code: String,
    verification_uri: String,
    verification_uri_complete: Option<String>,
    expires_in: u64,
    interval: u64,
}

/// `POST /oauth/device/code` (form-encoded), returning the code pair.
async fn request_device_authorization(
    http: &reqwest::Client,
    base: &Url,
    client_id: &str,
    scope: &str,
) -> Result<DeviceAuthorization> {
    let endpoint = device_code_endpoint(base)?;
    let form = [("client_id", client_id), ("scope", scope)];
    let response = http
        .post(endpoint)
        .header("Accept", "application/json")
        .form(&form)
        .send()
        .await
        .context("device authorization request failed")?;
    let status = response.status();
    let value: Value = response
        .json()
        .await
        .context("device authorization returned a non-JSON response")?;
    if !status.is_success() {
        bail!(
            "device authorization failed ({status}): {}",
            oauth_error_message(&value)
        );
    }
    let field = |name: &str| -> Result<String> {
        value
            .get(name)
            .and_then(Value::as_str)
            .filter(|field| !field.is_empty())
            .map(str::to_string)
            .with_context(|| format!("device authorization response did not contain {name}"))
    };
    Ok(DeviceAuthorization {
        device_code: field("device_code")?,
        user_code: field("user_code")?,
        verification_uri: field("verification_uri")?,
        verification_uri_complete: value
            .get("verification_uri_complete")
            .and_then(Value::as_str)
            .filter(|uri| !uri.is_empty())
            .map(str::to_string),
        expires_in: value
            .get("expires_in")
            .and_then(Value::as_u64)
            .unwrap_or(DEFAULT_EXPIRES_IN_SECONDS),
        interval: value
            .get("interval")
            .and_then(Value::as_u64)
            .unwrap_or(DEFAULT_POLL_INTERVAL_SECONDS),
    })
}

/// One token-poll outcome, decoded from the RFC 8628 §3.5 error contract.
enum Poll {
    /// User has not decided yet — poll again after the interval.
    Pending,
    /// Server asked us to back off — add 5s to the interval (RFC 8628 §3.5).
    SlowDown,
    /// Approved: the minted token.
    Token(super::TokenResponse),
}

/// Polls `POST /oauth/token` until approval, denial, expiry, or the deadline.
async fn poll_for_token(
    http: &reqwest::Client,
    base: &Url,
    device_code: &str,
    client_id: &str,
    interval: u64,
    expires_in: u64,
) -> Result<super::TokenResponse> {
    let deadline = Instant::now() + Duration::from_secs(expires_in);
    // The server-advertised cadence, and the floor we return to. A `slow_down`
    // raises the gap, but it must not raise it FOREVER: the previous version
    // only ever ratcheted up, so one back-off (however transient) slowed every
    // remaining poll of the sign-in, adding dead time after the user had
    // already approved.
    let base_interval = interval.clamp(1, 60);
    let mut interval = base_interval;
    // Distinguish "the user never approved" from "we never got a usable answer"
    // — a full window of edge 429s/5xx would otherwise be reported as an expiry,
    // sending the user to re-run a command that is not the problem.
    let mut backed_off = false;
    loop {
        tokio::time::sleep(Duration::from_secs(interval)).await;
        if Instant::now() >= deadline {
            if backed_off {
                bail!(
                    "gave up waiting for approval: the sign-in service kept rate-limiting \
                     or failing this device's requests. Check https://status.dairo.app, \
                     then run `dairo login --device-code` again"
                );
            }
            bail!(
                "the device code expired before the sign-in was approved; \
                 run `dairo login --device-code` again"
            );
        }
        match poll_token_once(http, base, device_code, client_id).await? {
            // A clean answer means the earlier back-off is no longer needed;
            // step back toward the advertised cadence so one blip cannot cost
            // the rest of the sign-in.
            Poll::Pending => interval = interval.saturating_sub(1).max(base_interval),
            // Back off as RFC 8628 §3.5 asks, but keep the ceiling low enough
            // that an approval the user already gave is still noticed promptly
            // — unbounded growth could stretch the gap to ~90s inside the
            // 15-minute window.
            Poll::SlowDown => {
                backed_off = true;
                interval = (interval + 5).min(30);
            }
            Poll::Token(token) => return Ok(token),
        }
    }
}

/// A single `POST /oauth/token` poll. Transient transport failures are
/// returned as `Pending` so a flaky network cannot abort an otherwise-healthy
/// wait; real OAuth errors terminate with a clear message.
async fn poll_token_once(
    http: &reqwest::Client,
    base: &Url,
    device_code: &str,
    client_id: &str,
) -> Result<Poll> {
    let endpoint = oauth_endpoint(base, "token")?;
    let form = [
        ("grant_type", DEVICE_GRANT_TYPE),
        ("device_code", device_code),
        ("client_id", client_id),
    ];
    let response = match http
        .post(endpoint)
        .header("Accept", "application/json")
        .form(&form)
        .send()
        .await
    {
        Ok(response) => response,
        // Network blip mid-wait: keep polling rather than aborting the login.
        Err(_) => return Ok(Poll::Pending),
    };
    let status = response.status();
    let value: Value = match response.json().await {
        Ok(value) => value,
        Err(_) => return Ok(Poll::Pending),
    };
    if status.is_success() {
        return Ok(Poll::Token(token_from_success_body(&value)?));
    }
    match oauth_error_code(&value) {
        Some("authorization_pending") => Ok(Poll::Pending),
        Some("slow_down") => Ok(Poll::SlowDown),
        Some("expired_token") => bail!(
            "the device code expired before the sign-in was approved; \
             run `dairo login --device-code` again"
        ),
        Some("access_denied") => bail!("the sign-in request was denied on the verification page"),
        // A rate-limited or unavailable EDGE answers in the Dairo envelope, not
        // the RFC shape, so it lands here. Those are recoverable — several
        // logins behind one NAT/CI egress IP can share the per-IP /oauth
        // budget — so back off and keep waiting instead of killing a sign-in
        // the user is about to approve. The deadline still bounds the loop.
        _ if status.as_u16() == 429 || status.is_server_error() => Ok(Poll::SlowDown),
        _ => bail!(
            "device login failed ({status}): {}",
            oauth_error_message(&value)
        ),
    }
}

/// Reads the RFC 6749 machine `error` code from an error body (`{"error":
/// "authorization_pending", ...}`). The Dairo envelope shape (`{"error":
/// {...}}`) yields `None` and is handled by the prose fallback.
fn oauth_error_code(value: &Value) -> Option<&str> {
    value.get("error").and_then(Value::as_str)
}

/// True when `uri` is a URL this process may hand to the system browser: https
/// (or loopback http for a dev backend) on a host under the same registrable
/// suffix as the configured OAuth base.
///
/// The launch target comes from the SERVER's response, so this bounds what a
/// compromised or buggy authorization server can make `dairo login` open —
/// notably file:// and custom schemes, which macOS `open` hands to whatever
/// application claims them. It is NOT a defense against a hostile
/// `DAIRO_OAUTH_BASE_URL`: whoever sets the base also controls the response,
/// and can simply answer with a URL under their own suffix.
fn is_launchable_verification_uri(uri: &str, base: &Url) -> bool {
    let Ok(parsed) = Url::parse(uri) else {
        return false;
    };
    let host = parsed.host_str().unwrap_or_default();
    let is_loopback = matches!(host, "localhost" | "127.0.0.1" | "::1");
    if parsed.scheme() != "https" && !is_loopback {
        return false;
    }
    // The verification page is a sibling host of the OAuth base
    // (mcp.dairo.app -> platform.dairo.app), so compare the registrable
    // suffix rather than requiring an exact host match.
    let base_host = base.host_str().unwrap_or_default();
    registrable_suffix(host) == registrable_suffix(base_host)
}

/// Last two labels of a hostname (`platform.dairo.app` -> `dairo.app`).
/// Loopback names have no dots and compare as themselves.
fn registrable_suffix(host: &str) -> String {
    let labels: Vec<&str> = host.split('.').collect();
    if labels.len() < 2 {
        return host.to_ascii_lowercase();
    }
    labels[labels.len() - 2..].join(".").to_ascii_lowercase()
}

/// `{base}/oauth/device/code`, preserving any base path prefix.
fn device_code_endpoint(base: &Url) -> Result<Url> {
    let mut url = oauth_endpoint(base, "device")?;
    url.path_segments_mut()
        .map_err(|_| anyhow::anyhow!("OAuth base URL cannot be a base: {base}"))?
        .push("code");
    Ok(url)
}

/// True when this process almost certainly cannot show the user a browser:
/// an SSH session (any OS), or a Linux host with neither an X11 nor a Wayland
/// display. `dairo login` auto-selects the device flow in that case.
pub fn is_headless_environment() -> bool {
    if std::env::var_os("SSH_CONNECTION").is_some() || std::env::var_os("SSH_TTY").is_some() {
        return true;
    }
    #[cfg(target_os = "linux")]
    {
        let has_display = std::env::var("DISPLAY").is_ok_and(|v| !v.trim().is_empty())
            || std::env::var("WAYLAND_DISPLAY").is_ok_and(|v| !v.trim().is_empty());
        if !has_display {
            return true;
        }
    }
    false
}

/// Terminal browsers that `webbrowser` runs in the FOREGROUND, blocking this
/// process until the user quits them. `$BROWSER` wins over every other
/// launcher on unix, so a desktop Linux user who set it to one of these would
/// otherwise hand the whole terminal to a full-screen browser and the device
/// poll loop would never start. Mirrors webbrowser's own TEXT_BROWSERS list.
const TEXT_BROWSERS: &[&str] = &[
    "lynx", "links", "links2", "elinks", "w3m", "eww", "netrik", "retawq", "curl", "wget",
];

/// True when `$BROWSER` names a terminal browser (so we must not auto-open).
fn browser_env_is_text_browser() -> bool {
    std::env::var("BROWSER").is_ok_and(|value| names_text_browser(&value))
}

/// True when a `$BROWSER`-style value (colon-separated commands, each possibly
/// with arguments and a path) names a terminal browser. Pure so it is testable
/// without mutating process-wide environment state under parallel tests.
fn names_text_browser(browser: &str) -> bool {
    browser.split(':').any(|entry| {
        let command = entry.split_whitespace().next().unwrap_or_default();
        let basename = command.rsplit('/').next().unwrap_or_default();
        TEXT_BROWSERS.contains(&basename)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn device_code_endpoint_builds_nested_path() {
        let base = Url::parse("https://mcp.dairo.app").unwrap();
        assert_eq!(
            device_code_endpoint(&base).unwrap().as_str(),
            "https://mcp.dairo.app/oauth/device/code"
        );
        let prefixed = Url::parse("https://api.example.test/root").unwrap();
        assert_eq!(
            device_code_endpoint(&prefixed).unwrap().as_str(),
            "https://api.example.test/root/oauth/device/code"
        );
    }

    #[test]
    fn oauth_error_code_reads_rfc_shape_only() {
        let rfc = serde_json::json!({ "error": "authorization_pending" });
        assert_eq!(oauth_error_code(&rfc), Some("authorization_pending"));
        let dairo = serde_json::json!({ "error": { "message": "nope" } });
        assert_eq!(oauth_error_code(&dairo), None);
        assert_eq!(oauth_error_code(&serde_json::json!({})), None);
    }

    #[test]
    fn grant_type_is_the_rfc_urn() {
        assert_eq!(
            DEVICE_GRANT_TYPE,
            "urn:ietf:params:oauth:grant-type:device_code"
        );
    }

    #[test]
    fn only_launches_https_urls_related_to_the_configured_base() {
        let base = Url::parse("https://mcp.dairo.app").unwrap();
        // The real verification page: sibling host under the same site.
        assert!(is_launchable_verification_uri(
            "https://platform.dairo.app/activate?user_code=BCDF-GHJK",
            &base
        ));
        // A compromised/buggy server must not redirect the launch off-site.
        assert!(!is_launchable_verification_uri(
            "https://evil.example/activate",
            &base
        ));
        // Lookalike host that merely CONTAINS the base's name.
        assert!(!is_launchable_verification_uri(
            "https://dairo.app.evil.test/x",
            &base
        ));
        // Non-https, and schemes the OS would hand to a registered app.
        assert!(!is_launchable_verification_uri(
            "http://platform.dairo.app/x",
            &base
        ));
        assert!(!is_launchable_verification_uri("file:///etc/passwd", &base));
        assert!(!is_launchable_verification_uri("not a url", &base));
    }

    #[test]
    fn loopback_dev_base_allows_a_loopback_verification_page() {
        // A local dev backend serves its own verification page on loopback;
        // the https requirement is relaxed there exactly as in the transport
        // guard, so auto-open keeps working against a dev stack.
        let base = Url::parse("http://localhost:3000").unwrap();
        assert!(is_launchable_verification_uri(
            "http://localhost:3000/activate",
            &base
        ));
        assert!(!is_launchable_verification_uri(
            "https://evil.example/activate",
            &base
        ));
    }

    #[test]
    fn text_browser_env_suppresses_the_auto_open() {
        // `webbrowser` runs these in the foreground and blocks the poll loop.
        for value in ["w3m", "/usr/bin/lynx", "links2 -g", "elinks:firefox"] {
            assert!(
                names_text_browser(value),
                "expected `{value}` to be treated as a blocking text browser"
            );
        }
        for value in ["firefox", "/usr/bin/google-chrome", "open -a Safari", ""] {
            assert!(
                !names_text_browser(value),
                "expected `{value}` to be treated as a real browser"
            );
        }
    }
}
