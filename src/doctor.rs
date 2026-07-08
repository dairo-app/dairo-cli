//! `dairo doctor`: a local health check that prints a ✓/✗ checklist so a user
//! can diagnose auth/connectivity issues without guessing.
//!
//! It degrades gracefully: when no token is configured the network checks are
//! skipped (and reported as such) rather than erroring, so `doctor` is always
//! safe to run. The token value is never printed.

use anyhow::Result;
use serde_json::json;

use crate::api::ApiClient;
use crate::config::Config;
use crate::output::OutputFormat;

const PASS: &str = "\u{2713}"; // ✓
const FAIL: &str = "\u{2717}"; // ✗
const SKIP: &str = "\u{2013}"; // –

/// One checklist row: a status, a label, and an optional detail line.
struct Check {
    ok: Option<bool>, // None = skipped/not-applicable
    label: String,
    detail: Option<String>,
}

impl Check {
    fn pass(label: impl Into<String>, detail: impl Into<String>) -> Self {
        Self {
            ok: Some(true),
            label: label.into(),
            detail: Some(detail.into()),
        }
    }
    fn fail(label: impl Into<String>, detail: impl Into<String>) -> Self {
        Self {
            ok: Some(false),
            label: label.into(),
            detail: Some(detail.into()),
        }
    }
    fn skip(label: impl Into<String>, detail: impl Into<String>) -> Self {
        Self {
            ok: None,
            label: label.into(),
            detail: Some(detail.into()),
        }
    }
    fn mark(&self) -> &'static str {
        match self.ok {
            Some(true) => PASS,
            Some(false) => FAIL,
            None => SKIP,
        }
    }
    fn json(&self) -> serde_json::Value {
        json!({
            "status": match self.ok {
                Some(true) => "ok",
                Some(false) => "fail",
                None => "skipped",
            },
            "label": self.label,
            "detail": self.detail,
        })
    }
}

/// Runs the health check. `config` is already loaded; `base_url` is the resolved
/// API base; `api_key` is the resolved token (empty when none is configured).
/// `config_path` is shown so the user knows where credentials live.
pub async fn run(
    _config: &Config,
    base_url: &str,
    api_key: &str,
    config_path: &std::path::Path,
    format: OutputFormat,
) -> Result<()> {
    let mut checks: Vec<Check> = Vec::new();

    // 1. Token configured?
    let have_token = !api_key.trim().is_empty();
    if have_token {
        checks.push(Check::pass(
            "Token configured",
            "a Dairo API token is available",
        ));
    } else {
        checks.push(Check::fail(
            "Token configured",
            "no token found; run `dairo login` or set DAIRO_API_KEY",
        ));
    }

    // 2. Base URL.
    checks.push(Check::pass("Base URL", base_url.to_string()));

    // 3. Config / credential location.
    checks.push(Check::pass(
        "Config & credential location",
        format!(
            "config file {} (0600; token stored here — no OS keychain)",
            config_path.display()
        ),
    ));

    // 4. whoami (only when authenticated and the client built).
    let mut whoami_ok = false;
    if have_token {
        match ApiClient::new(base_url, api_key) {
            Ok(client) => match client.whoami().await {
                Ok(resp) => {
                    whoami_ok = true;
                    let scopes = if resp.api_key.scopes.is_empty() {
                        "(no scopes reported)".to_string()
                    } else {
                        resp.api_key.scopes.join(", ")
                    };
                    checks.push(Check::pass(
                        "whoami",
                        format!(
                            "user {} (plan {}); scopes: {scopes}",
                            resp.user_id, resp.plan
                        ),
                    ));

                    // 5. Domain verification status.
                    match client.list_domains().await {
                        Ok(domains) => {
                            if domains.data.is_empty() {
                                checks.push(Check::skip(
                                    "Domain verification",
                                    "no domains yet; add one with `dairo domain add <domain>`",
                                ));
                            } else {
                                let verified = domains
                                    .data
                                    .iter()
                                    .filter(|d| d.status.eq_ignore_ascii_case("verified"))
                                    .count();
                                let total = domains.data.len();
                                let all_verified = verified == total;
                                let detail = domains
                                    .data
                                    .iter()
                                    .map(|d| format!("{} [{}]", d.domain, d.status))
                                    .collect::<Vec<_>>()
                                    .join(", ");
                                let label = "Domain verification";
                                if all_verified {
                                    checks.push(Check::pass(
                                        label,
                                        format!("{verified}/{total} verified: {detail}"),
                                    ));
                                } else {
                                    checks.push(Check::fail(
                                        label,
                                        format!("{verified}/{total} verified: {detail}"),
                                    ));
                                }
                            }
                        }
                        Err(err) => checks.push(Check::fail(
                            "Domain verification",
                            format!("could not list domains: {err}"),
                        )),
                    }
                }
                Err(err) => checks.push(Check::fail("whoami", format!("request failed: {err}"))),
            },
            Err(err) => checks.push(Check::fail(
                "whoami",
                format!("could not build API client: {err}"),
            )),
        }
    } else {
        checks.push(Check::skip("whoami", "skipped (not authenticated)"));
        checks.push(Check::skip(
            "Domain verification",
            "skipped (not authenticated)",
        ));
    }

    if format == OutputFormat::Json {
        let healthy = checks.iter().all(|c| c.ok != Some(false));
        let payload = json!({
            "healthy": healthy,
            "authenticated": whoami_ok,
            "checks": checks.iter().map(Check::json).collect::<Vec<_>>(),
        });
        println!("{}", serde_json::to_string_pretty(&payload)?);
        return Ok(());
    }

    println!("Dairo CLI health check");
    for check in &checks {
        println!("  {} {}", check.mark(), check.label);
        if let Some(detail) = &check.detail {
            println!("      {detail}");
        }
    }
    Ok(())
}
