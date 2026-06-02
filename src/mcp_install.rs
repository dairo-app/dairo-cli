use anyhow::{Context, Result};
use serde_json::{json, Value};
use std::{
    fs,
    path::{Path, PathBuf},
};

#[cfg(unix)]
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};

use crate::cli::McpClient;

#[derive(Debug, Clone)]
pub struct McpInstallReport {
    pub client: String,
    pub path: PathBuf,
    pub action: String,
    pub verify: String,
}

pub fn install(
    client: McpClient,
    name: &str,
    api_url: &str,
    api_key: &str,
) -> Result<Vec<McpInstallReport>> {
    let endpoint = mcp_endpoint(api_url)?;
    let clients = match client {
        McpClient::Auto => vec![
            McpClient::Hermes,
            McpClient::Codex,
            McpClient::Cursor,
            McpClient::Claude,
        ],
        other => vec![other],
    };
    let mut reports = Vec::new();
    for client in clients {
        let report = match client {
            McpClient::Auto => unreachable!(),
            McpClient::Hermes => install_hermes(name, &endpoint, api_key)?,
            McpClient::Codex => install_codex(name, &endpoint, api_key)?,
            McpClient::Cursor => install_cursor(name, &endpoint, api_key)?,
            McpClient::Claude => install_claude_project(name, &endpoint, api_key)?,
        };
        reports.push(report);
    }
    Ok(reports)
}

fn mcp_endpoint(api_url: &str) -> Result<String> {
    let trimmed = api_url.trim_end_matches('/');
    let endpoint = if trimmed.ends_with("/mcp") {
        trimmed.to_string()
    } else {
        format!("{trimmed}/mcp")
    };
    anyhow::ensure!(
        endpoint.starts_with("https://") || endpoint.starts_with("http://"),
        "API URL must be absolute"
    );
    Ok(endpoint)
}

fn install_hermes(name: &str, endpoint: &str, api_key: &str) -> Result<McpInstallReport> {
    let path = home()?.join(".hermes").join("config.yaml");
    ensure_parent_private(&path)?;
    let mut contents = read_optional(&path)?;
    let block = format!(
        "\n  {name}:\n    url: \"{endpoint}\"\n    headers:\n      Authorization: \"Bearer {api_key}\"\n    timeout: 120\n    connect_timeout: 60\n"
    );
    let action =
        if contents.contains(&format!("\n  {name}:")) || contents.contains(&format!("\n{name}:")) {
            "already configured".to_string()
        } else {
            if contents.trim().is_empty() {
                contents.push_str("mcp_servers:\n");
            } else if !contents.contains("mcp_servers:") {
                if !contents.ends_with('\n') {
                    contents.push('\n');
                }
                contents.push_str("\nmcp_servers:\n");
            }
            contents.push_str(&block);
            write_private(&path, contents.as_bytes())?;
            "configured".to_string()
        };
    Ok(McpInstallReport {
        client: "hermes".to_string(),
        path,
        action,
        verify: "restart Hermes or run /reload-mcp, then ask Max to call dairo.whoami".to_string(),
    })
}

fn install_codex(name: &str, endpoint: &str, api_key: &str) -> Result<McpInstallReport> {
    let path = home()?.join(".codex").join("config.toml");
    ensure_parent_private(&path)?;
    let mut contents = read_optional(&path)?;
    let header = format!("[mcp_servers.{name}]");
    let action = if contents.contains(&header) {
        "already configured".to_string()
    } else {
        if !contents.ends_with('\n') && !contents.is_empty() {
            contents.push('\n');
        }
        contents.push_str(&format!(
            "\n[mcp_servers.{name}]\nurl = \"{endpoint}\"\nheaders = {{ Authorization = \"Bearer {api_key}\" }}\n"
        ));
        write_private(&path, contents.as_bytes())?;
        "configured".to_string()
    };
    Ok(McpInstallReport {
        client: "codex".to_string(),
        path,
        action,
        verify: "restart Codex and run /mcp; use dairo.whoami as a smoke test".to_string(),
    })
}

fn install_cursor(name: &str, endpoint: &str, api_key: &str) -> Result<McpInstallReport> {
    let path = home()?.join(".cursor").join("mcp.json");
    ensure_parent_private(&path)?;
    let mut value = if path.exists() {
        let text =
            fs::read_to_string(&path).with_context(|| format!("reading {}", path.display()))?;
        serde_json::from_str::<Value>(&text).unwrap_or_else(|_| json!({}))
    } else {
        json!({})
    };
    if !value.get("mcpServers").is_some_and(Value::is_object) {
        value["mcpServers"] = json!({});
    }
    let action = if value["mcpServers"].get(name).is_some() {
        "already configured".to_string()
    } else {
        value["mcpServers"][name] = json!({
            "type": "http",
            "url": endpoint,
            "headers": { "Authorization": format!("Bearer {api_key}") }
        });
        write_private(&path, serde_json::to_string_pretty(&value)?.as_bytes())?;
        "configured".to_string()
    };
    Ok(McpInstallReport {
        client: "cursor".to_string(),
        path,
        action,
        verify: "restart Cursor or reload MCP tools; ask agent to call dairo.whoami".to_string(),
    })
}

fn install_claude_project(name: &str, endpoint: &str, api_key: &str) -> Result<McpInstallReport> {
    let path = std::env::current_dir()?.join(".mcp.json");
    let mut value = if path.exists() {
        let text =
            fs::read_to_string(&path).with_context(|| format!("reading {}", path.display()))?;
        serde_json::from_str::<Value>(&text).unwrap_or_else(|_| json!({}))
    } else {
        json!({})
    };
    if !value.get("mcpServers").is_some_and(Value::is_object) {
        value["mcpServers"] = json!({});
    }
    let action = if value["mcpServers"].get(name).is_some() {
        "already configured".to_string()
    } else {
        value["mcpServers"][name] = json!({
            "type": "http",
            "url": endpoint,
            "headers": { "Authorization": format!("Bearer {api_key}") }
        });
        write_private(&path, serde_json::to_string_pretty(&value)?.as_bytes())?;
        "configured".to_string()
    };
    Ok(McpInstallReport {
        client: "claude".to_string(),
        path,
        action,
        verify: "run claude in this project, then /mcp and select dairo if prompted".to_string(),
    })
}

fn read_optional(path: &PathBuf) -> Result<String> {
    if path.exists() {
        fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))
    } else {
        Ok(String::new())
    }
}

fn ensure_parent_private(path: &Path) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).with_context(|| format!("creating {}", parent.display()))?;
        #[cfg(unix)]
        fs::set_permissions(parent, fs::Permissions::from_mode(0o700)).ok();
    }
    Ok(())
}

#[cfg(unix)]
fn write_private(path: &PathBuf, contents: &[u8]) -> Result<()> {
    let mut file = fs::OpenOptions::new()
        .create(true)
        .truncate(true)
        .write(true)
        .mode(0o600)
        .open(path)
        .with_context(|| format!("opening {}", path.display()))?;
    use std::io::Write;
    file.write_all(contents)
        .with_context(|| format!("writing {}", path.display()))
}

#[cfg(not(unix))]
fn write_private(path: &PathBuf, contents: &[u8]) -> Result<()> {
    fs::write(path, contents).with_context(|| format!("writing {}", path.display()))
}

fn home() -> Result<PathBuf> {
    dirs::home_dir().context("could not determine home directory")
}
