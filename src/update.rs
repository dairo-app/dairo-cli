//! `dairo update`: a best-effort "is there a newer release?" check.
//!
//! It queries the GitHub releases `latest` API for `dairo-app/dairo-cli` and
//! prints the current vs latest version with upgrade instructions. It never
//! replaces the running binary (no self-update), and degrades gracefully when
//! offline — a network failure prints the current version plus the manual
//! upgrade instructions rather than erroring out.

use std::time::Duration;

use anyhow::Result;
use serde_json::json;

use crate::output::OutputFormat;

const CURRENT_VERSION: &str = env!("CARGO_PKG_VERSION");
const LATEST_RELEASE_API: &str = "https://api.github.com/repos/dairo-app/dairo-cli/releases/latest";
const USER_AGENT: &str = concat!("dairo-cli/", env!("CARGO_PKG_VERSION"));

const UPGRADE_HINT: &str = "Upgrade with `brew upgrade dairo`, or re-run the install script:\n  \
     curl -fsSL https://dairo.app/install.sh | sh";

/// Fetches the latest release tag (best effort). `None` on any network/parse
/// failure so the caller can degrade gracefully.
async fn fetch_latest_tag() -> Option<String> {
    let client = reqwest::Client::builder()
        .user_agent(USER_AGENT)
        .timeout(Duration::from_secs(10))
        .build()
        .ok()?;
    let response = client
        .get(LATEST_RELEASE_API)
        .header("Accept", "application/vnd.github+json")
        .send()
        .await
        .ok()?;
    if !response.status().is_success() {
        return None;
    }
    let value: serde_json::Value = response.json().await.ok()?;
    value
        .get("tag_name")
        .and_then(serde_json::Value::as_str)
        .map(|tag| tag.trim_start_matches('v').to_string())
        .filter(|tag| !tag.is_empty())
}

pub async fn run(format: OutputFormat) -> Result<()> {
    let latest = fetch_latest_tag().await;

    if format == OutputFormat::Json {
        let up_to_date = latest.as_deref().map(|latest| latest == CURRENT_VERSION);
        let payload = json!({
            "current": CURRENT_VERSION,
            "latest": latest,
            "upToDate": up_to_date,
            "upgrade": UPGRADE_HINT,
        });
        println!("{}", serde_json::to_string_pretty(&payload)?);
        return Ok(());
    }

    println!("Current version: {CURRENT_VERSION}");
    match latest {
        Some(latest) if latest == CURRENT_VERSION => {
            println!("Latest version:  {latest}");
            println!("You are on the latest Dairo CLI release.");
        }
        Some(latest) => {
            println!("Latest version:  {latest}");
            println!("A newer release is available.");
            println!("{UPGRADE_HINT}");
        }
        None => {
            println!("Latest version:  (could not be determined — offline or unreachable)");
            println!("{UPGRADE_HINT}");
        }
    }
    Ok(())
}
