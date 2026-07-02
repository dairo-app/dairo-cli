//! `dairo mcp catalog` — render the hosted MCP tool catalog.
//!
//! The catalog is fetched from `GET /v1/mcp/catalog` (see
//! [`crate::api::ApiClient::mcp_catalog`]), the single source of truth for the
//! tools exposed by Dairo's hosted MCP server at `api.dairo.app/mcp`. This module
//! only formats that response: a `name / family / scope / confirm` table by
//! default, the raw JSON with `--json`, an `allowed` column with `--for-me`, and
//! an optional `--family` filter.
//!
//! `--for-me` requests the server-annotated catalog (`?for=me`): each tool gains
//! an `allowed: bool` computed from the active key's scopes. In that mode we show
//! only the tools the key can actually call, so the table doubles as "what can I
//! do with this key".

use anyhow::Result;
use serde_json::Value;

use crate::output::OutputFormat;

/// Renders a catalog payload (already fetched) to stdout.
///
/// `for_me` reflects whether the catalog was requested with `?for=me`; when set,
/// the table is filtered to the tools the key is allowed to call and gains an
/// `ALLOWED` column. `family`, if present, restricts output to a single family.
pub fn render(
    catalog: &Value,
    format: OutputFormat,
    for_me: bool,
    family: Option<&str>,
) -> Result<()> {
    // `--json` (and `--family`/`--for-me` combined with it) emits the raw catalog,
    // narrowed to the requested family so the machine output matches the filter.
    if format == OutputFormat::Json {
        let value = match family {
            Some(family) => filter_catalog_by_family(catalog, family),
            None => catalog.clone(),
        };
        println!("{}", serde_json::to_string_pretty(&value)?);
        return Ok(());
    }

    let tools = collect_tools(catalog, for_me, family);

    print_header(catalog, for_me, family, tools.len());

    if tools.is_empty() {
        if let Some(family) = family {
            println!("No tools found in family \"{family}\".");
        } else if for_me {
            println!("The active API key cannot call any catalog tools.");
        } else {
            println!("No tools found.");
        }
        return Ok(());
    }

    print_table(&tools, for_me);
    Ok(())
}

/// A flattened view of one catalog tool for table rendering.
struct ToolRow {
    name: String,
    family: String,
    scope: String,
    confirm: bool,
    allowed: bool,
}

/// Extracts the rows to display: applies the `--family` filter and, when
/// `for_me` is set, keeps only the tools the key is allowed to call.
fn collect_tools(catalog: &Value, for_me: bool, family: Option<&str>) -> Vec<ToolRow> {
    let Some(tools) = catalog.get("tools").and_then(Value::as_array) else {
        return Vec::new();
    };
    tools
        .iter()
        .filter(|tool| match family {
            Some(family) => tool_str(tool, "family") == family,
            None => true,
        })
        .map(|tool| ToolRow {
            name: tool_str(tool, "name").to_string(),
            family: tool_str(tool, "family").to_string(),
            // `scope` is `null` for the local-only tools (docs/catalog); the
            // synthetic `__any__` means "any valid key".
            scope: scope_label(tool.get("scope")),
            confirm: tool
                .get("confirmRequired")
                .and_then(Value::as_bool)
                .unwrap_or(false),
            // Absent on the public catalog; defaults to allowed so a non-annotated
            // catalog renders every tool.
            allowed: tool.get("allowed").and_then(Value::as_bool).unwrap_or(true),
        })
        // With `--for-me`, hide tools the key cannot call.
        .filter(|row| !for_me || row.allowed)
        .collect()
}

/// Prints the catalog summary line (server, version, tool count, filters).
fn print_header(catalog: &Value, for_me: bool, family: Option<&str>, shown: usize) {
    let version = catalog
        .get("catalogVersion")
        .and_then(Value::as_str)
        .unwrap_or("unknown");
    let total = catalog
        .get("toolCount")
        .and_then(Value::as_u64)
        .map(|count| count as usize)
        .or_else(|| catalog.get("tools").and_then(Value::as_array).map(Vec::len));

    let mut summary = format!("Dairo MCP catalog (version {version})");
    match total {
        Some(total) if total != shown => summary.push_str(&format!(" — {shown}/{total} tools")),
        Some(total) => summary.push_str(&format!(" — {total} tools")),
        None => summary.push_str(&format!(" — {shown} tools")),
    }
    if let Some(family) = family {
        summary.push_str(&format!(", family={family}"));
    }
    if for_me {
        summary.push_str(", allowed by active key");
    }
    println!("{summary}");
}

/// Prints the aligned `name / family / scope / confirm [/ allowed]` table.
fn print_table(tools: &[ToolRow], for_me: bool) {
    if for_me {
        println!(
            "{:<34} {:<14} {:<16} {:<8} ALLOWED",
            "NAME", "FAMILY", "SCOPE", "CONFIRM"
        );
        for tool in tools {
            println!(
                "{:<34} {:<14} {:<16} {:<8} {}",
                tool.name,
                tool.family,
                tool.scope,
                yes_no(tool.confirm),
                yes_no(tool.allowed),
            );
        }
    } else {
        println!("{:<34} {:<14} {:<16} CONFIRM", "NAME", "FAMILY", "SCOPE");
        for tool in tools {
            println!(
                "{:<34} {:<14} {:<16} {}",
                tool.name,
                tool.family,
                tool.scope,
                yes_no(tool.confirm),
            );
        }
    }
}

/// Returns a copy of the catalog with `tools` (and `toolNames`/`toolCount`)
/// narrowed to a single family, so `--json --family` stays internally consistent.
fn filter_catalog_by_family(catalog: &Value, family: &str) -> Value {
    let mut value = catalog.clone();
    let Some(object) = value.as_object_mut() else {
        return value;
    };
    let filtered: Vec<Value> = catalog
        .get("tools")
        .and_then(Value::as_array)
        .map(|tools| {
            tools
                .iter()
                .filter(|tool| tool_str(tool, "family") == family)
                .cloned()
                .collect()
        })
        .unwrap_or_default();
    let names: Vec<Value> = filtered
        .iter()
        .map(|tool| Value::String(tool_str(tool, "name").to_string()))
        .collect();
    object.insert("toolCount".to_string(), Value::from(filtered.len()));
    object.insert("toolNames".to_string(), Value::Array(names));
    object.insert("tools".to_string(), Value::Array(filtered));
    value
}

/// Reads a string field from a tool object, defaulting to `""`.
fn tool_str<'a>(tool: &'a Value, key: &str) -> &'a str {
    tool.get(key).and_then(Value::as_str).unwrap_or("")
}

/// Human label for a tool's `scope` field. `null` → `none` (local-only tool),
/// `__any__` → `any` (any valid key), otherwise the scope verbatim.
fn scope_label(scope: Option<&Value>) -> String {
    match scope.and_then(Value::as_str) {
        None => "none".to_string(),
        Some("__any__") => "any".to_string(),
        Some(scope) => scope.to_string(),
    }
}

fn yes_no(value: bool) -> &'static str {
    if value {
        "yes"
    } else {
        "no"
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn sample_catalog() -> Value {
        json!({
            "catalogVersion": "abc123def456",
            "server": "dairo",
            "toolCount": 3,
            "toolNames": ["dairo.docs", "dairo.list.inboxes", "dairo.send"],
            "tools": [
                {
                    "name": "dairo.docs",
                    "family": "account",
                    "description": "Docs.",
                    "scope": null,
                    "confirmRequired": false
                },
                {
                    "name": "dairo.list.inboxes",
                    "family": "inboxes",
                    "description": "List inboxes.",
                    "scope": "messages:read",
                    "confirmRequired": false
                },
                {
                    "name": "dairo.send",
                    "family": "outbound",
                    "description": "Send.",
                    "scope": "messages:send",
                    "confirmRequired": false
                }
            ]
        })
    }

    #[test]
    fn collects_all_tools_without_filter() {
        let rows = collect_tools(&sample_catalog(), false, None);
        assert_eq!(rows.len(), 3);
        assert_eq!(rows[0].name, "dairo.docs");
    }

    #[test]
    fn family_filter_narrows_rows() {
        let rows = collect_tools(&sample_catalog(), false, Some("outbound"));
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].name, "dairo.send");
    }

    #[test]
    fn unknown_family_yields_no_rows() {
        let rows = collect_tools(&sample_catalog(), false, Some("nope"));
        assert!(rows.is_empty());
    }

    #[test]
    fn for_me_keeps_only_allowed_tools() {
        let mut catalog = sample_catalog();
        let tools = catalog["tools"].as_array_mut().unwrap();
        tools[0]["allowed"] = json!(true); // docs (local)
        tools[1]["allowed"] = json!(true); // list.inboxes (messages:read)
        tools[2]["allowed"] = json!(false); // send.email (messages:send, not held)

        let rows = collect_tools(&catalog, true, None);
        assert_eq!(rows.len(), 2);
        assert!(rows.iter().all(|row| row.name != "dairo.send"));
    }

    #[test]
    fn for_me_without_annotation_defaults_to_allowed() {
        // A catalog with no `allowed` field (e.g. served unannotated) must not
        // silently drop every tool under `--for-me`.
        let rows = collect_tools(&sample_catalog(), true, None);
        assert_eq!(rows.len(), 3);
    }

    #[test]
    fn scope_label_maps_special_values() {
        assert_eq!(scope_label(Some(&Value::Null)), "none");
        assert_eq!(scope_label(None), "none");
        assert_eq!(scope_label(Some(&json!("__any__"))), "any");
        assert_eq!(scope_label(Some(&json!("messages:read"))), "messages:read");
    }

    #[test]
    fn json_family_filter_rewrites_counts() {
        let filtered = filter_catalog_by_family(&sample_catalog(), "outbound");
        assert_eq!(filtered["toolCount"], json!(1));
        assert_eq!(filtered["tools"].as_array().unwrap().len(), 1);
        assert_eq!(filtered["toolNames"], json!(["dairo.send"]));
        // Untouched top-level fields are preserved.
        assert_eq!(filtered["server"], json!("dairo"));
        assert_eq!(filtered["catalogVersion"], json!("abc123def456"));
    }
}
