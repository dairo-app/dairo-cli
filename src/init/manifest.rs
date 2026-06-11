//! Per-framework scaffold manifests for `dairo init`.
//!
//! A [`FrameworkSpec`] turns a [`Framework`] plus the resolved options (target
//! directory, inbox route, package manager) into a concrete list of
//! [`PlannedFile`]s — each with its rendered bytes and a [`WriteMode`] that says
//! how to reconcile it with any existing file in the user's project.
//!
//! ## SDK version pins (OWNER-GATED)
//!
//! The generated `package.json` / `requirements.txt` / `go.mod` pin specific SDK
//! versions. Those SDK packages must be **published** at the pinned versions for
//! the install step to resolve:
//!
//! - npm `dairo` → [`NPM_SDK_VERSION`]
//! - PyPI `dairo` → [`PYPI_SDK_VERSION`]
//! - Go `github.com/dairo-app/dairo-go` → [`GO_SDK_VERSION`]
//!
//! These are placeholders pinned to the SDKs' current local `0.1.0`. Before a CLI
//! release ships, confirm each package is published at its pinned version (the
//! publish itself is the owner-gated step — see the design doc's "Owner-gated
//! steps"). Bumping a pin here is a one-line change.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use crate::cli::Framework;
use crate::init::render::render;
use crate::init::templates::templates_for;

/// Pinned npm `dairo` version range for generated `package.json` files.
/// OWNER-GATED: npm publish of `dairo` at this version.
pub const NPM_SDK_VERSION: &str = "^0.1.0";
/// Pinned PyPI `dairo` version range for generated `requirements.txt` files.
/// OWNER-GATED: PyPI publish of `dairo` at this version.
pub const PYPI_SDK_VERSION: &str = ">=0.1.0,<0.2.0";
/// Pinned Go module version tag for generated `go.mod` files.
/// OWNER-GATED: tagged release of `github.com/dairo-app/dairo-go`.
pub const GO_SDK_VERSION: &str = "v0.1.0";

/// The default event the webhook stub branches on across all frameworks.
const DEFAULT_WEBHOOK_EVENT: &str = "message.received";

/// The default webhook mount path when `--inbox-route` is not given.
const DEFAULT_INBOX_ROUTE: &str = "/api/dairo/webhook";

/// Language family of a framework, which drives manifest generation
/// (`package.json` vs `requirements.txt` vs `go.mod`) and package-manager
/// detection/defaults.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Language {
    Node,
    Python,
    Go,
}

/// How a planned file should be reconciled with an existing file on disk.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WriteMode {
    /// Write only if absent. If it exists with the same bytes → "unchanged";
    /// with different bytes → "skipped (exists)" unless `--force`.
    CreateIfAbsent,
    /// JSON dependency/scripts merge that preserves all other user keys
    /// (`package.json`).
    MergeJson,
    /// Append missing lines only, never overwriting existing values
    /// (`requirements.txt`, `.env*`, `.gitignore`). The planned `contents` are
    /// the candidate lines to ensure are present.
    AppendLines,
    /// Secret-capable file (`.env*`, `.dev.vars*`): created/append `0600`, never
    /// containing a real secret (placeholders only).
    AppendLinesPrivate,
}

/// A single file the scaffold wants to materialize.
#[derive(Debug, Clone)]
pub struct PlannedFile {
    /// Absolute output path inside the target directory.
    pub path: PathBuf,
    /// Output path relative to the target dir, for reporting.
    pub rel_path: String,
    /// Rendered file contents (or candidate lines, for append modes).
    pub contents: String,
    pub mode: WriteMode,
}

/// A fully-resolved scaffold plan for one framework.
pub struct FrameworkSpec {
    pub framework: Framework,
    pub language: Language,
    /// Files to materialize, in a stable order.
    pub files: Vec<PlannedFile>,
    /// The webhook mount path used by this scaffold.
    pub inbox_route: String,
    /// The SDK package id referenced (for the report/next-steps).
    pub sdk_package: &'static str,
}

impl Language {
    pub fn for_framework(framework: Framework) -> Self {
        match framework {
            Framework::Next
            | Framework::Express
            | Framework::Hono
            | Framework::CloudflareWorkers => Self::Node,
            Framework::Fastapi | Framework::Flask => Self::Python,
            Framework::GoHttp => Self::Go,
        }
    }
}

/// Builds the resolved [`FrameworkSpec`] for `framework` in `target_dir`, with an
/// optional `--inbox-route` override and a resolved `package_manager` (used only
/// to render the README's install command).
pub fn build_spec(
    framework: Framework,
    target_dir: &Path,
    inbox_route_override: Option<&str>,
    package_manager: &str,
) -> FrameworkSpec {
    let language = Language::for_framework(framework);
    let inbox_route = normalize_route(inbox_route_override.unwrap_or(DEFAULT_INBOX_ROUTE));
    let project_name = project_name(target_dir);
    let install_cmd = install_command_for(language, package_manager, framework);

    let mut vars: BTreeMap<&str, String> = BTreeMap::new();
    vars.insert("inbox_route", inbox_route.clone());
    vars.insert("webhook_event", DEFAULT_WEBHOOK_EVENT.to_string());
    vars.insert("project_name", project_name.clone());
    vars.insert("install_cmd", install_cmd);
    vars.insert("sdk_version", go_sdk_version_string());

    let mut files = Vec::new();

    // 1. Embedded code/doc templates (verbatim, create-if-absent).
    for template in templates_for(framework) {
        let rendered = render(template.contents, &vars);
        files.push(planned(
            target_dir,
            template.out_path,
            rendered,
            WriteMode::CreateIfAbsent,
        ));
    }

    // 2. Language-specific dependency manifest (merge/append).
    match language {
        Language::Node => {
            files.push(planned(
                target_dir,
                "package.json",
                node_package_json(&project_name, framework),
                WriteMode::MergeJson,
            ));
        }
        Language::Python => {
            files.push(planned(
                target_dir,
                "requirements.txt",
                python_requirements(framework),
                WriteMode::AppendLines,
            ));
        }
        Language::Go => {
            // go.mod is an embedded template (create-if-absent); nothing extra.
        }
    }

    // 3. Env wiring: DAIRO_API_KEY (+ DAIRO_WEBHOOK_SECRET) placeholders, never a
    //    real secret. Workers uses .dev.vars; everyone else uses .env.example.
    let env_keys = env_keys_for(framework);
    let env_body = env_placeholder_body(&env_keys);
    if framework == Framework::CloudflareWorkers {
        files.push(planned(
            target_dir,
            ".dev.vars.example",
            env_body,
            WriteMode::AppendLinesPrivate,
        ));
    } else {
        files.push(planned(
            target_dir,
            ".env.example",
            env_body,
            WriteMode::AppendLinesPrivate,
        ));
    }

    // 4. .gitignore: ensure real secret files are ignored.
    files.push(planned(
        target_dir,
        ".gitignore",
        gitignore_lines(framework),
        WriteMode::AppendLines,
    ));

    FrameworkSpec {
        framework,
        language,
        files,
        inbox_route,
        sdk_package: sdk_package(language),
    }
}

fn planned(target_dir: &Path, rel: &str, contents: String, mode: WriteMode) -> PlannedFile {
    PlannedFile {
        path: target_dir.join(rel),
        rel_path: rel.to_string(),
        contents,
        mode,
    }
}

/// Environment variable keys a framework needs wired into its env file. All
/// frameworks need the API key and the webhook secret.
pub fn env_keys_for(_framework: Framework) -> Vec<&'static str> {
    vec!["DAIRO_API_KEY", "DAIRO_WEBHOOK_SECRET"]
}

fn env_placeholder_body(keys: &[&str]) -> String {
    let mut body = String::new();
    for key in keys {
        body.push_str(key);
        body.push_str("=\n");
    }
    body
}

fn gitignore_lines(framework: Framework) -> String {
    if framework == Framework::CloudflareWorkers {
        ".dev.vars\n.env\n".to_string()
    } else {
        ".env\n.env.local\n".to_string()
    }
}

/// Minimal `package.json` for a fresh Node project (only used when none exists;
/// otherwise this is merged into the existing one).
fn node_package_json(project_name: &str, framework: Framework) -> String {
    let mut deps = format!("    \"dairo\": \"{NPM_SDK_VERSION}\"");
    if framework == Framework::Express {
        deps.push_str(",\n    \"express\": \"^4.19.0\"");
    } else if framework == Framework::Hono {
        deps.push_str(",\n    \"hono\": \"^4.6.0\"");
    }
    format!(
        "{{\n  \"name\": \"{project_name}\",\n  \"private\": true,\n  \"type\": \"module\",\n  \"dependencies\": {{\n{deps}\n  }}\n}}\n"
    )
}

/// Dependency lines for a Python project's `requirements.txt`.
fn python_requirements(framework: Framework) -> String {
    let mut lines = format!("dairo{PYPI_SDK_VERSION}\n");
    match framework {
        Framework::Fastapi => {
            lines.push_str("fastapi>=0.110.0\nuvicorn>=0.29.0\n");
        }
        Framework::Flask => {
            lines.push_str("flask>=3.0.0\n");
        }
        _ => {}
    }
    lines
}

fn go_sdk_version_string() -> String {
    GO_SDK_VERSION.to_string()
}

/// The dependencies a framework's package.json must contain after a merge:
/// `(name, version)` pairs. Used by the JSON-merge writer in `mod.rs`.
pub fn node_dependencies(framework: Framework) -> Vec<(&'static str, &'static str)> {
    let mut deps = vec![("dairo", NPM_SDK_VERSION)];
    match framework {
        Framework::Express => deps.push(("express", "^4.19.0")),
        Framework::Hono => deps.push(("hono", "^4.6.0")),
        _ => {}
    }
    deps
}

fn sdk_package(language: Language) -> &'static str {
    match language {
        Language::Node => "dairo (npm)",
        Language::Python => "dairo (PyPI)",
        Language::Go => "github.com/dairo-app/dairo-go",
    }
}

/// The README install command line for the resolved package manager.
fn install_command_for(language: Language, package_manager: &str, framework: Framework) -> String {
    match language {
        Language::Node => {
            let deps = node_dependencies(framework)
                .iter()
                .map(|(name, _)| *name)
                .collect::<Vec<_>>()
                .join(" ");
            match package_manager {
                "pnpm" => format!("pnpm add {deps}"),
                "yarn" => format!("yarn add {deps}"),
                "bun" => format!("bun add {deps}"),
                _ => format!("npm install {deps}"),
            }
        }
        Language::Python => match package_manager {
            "poetry" => "poetry add -r requirements.txt".to_string(),
            "uv" => "uv pip install -r requirements.txt".to_string(),
            _ => "pip install -r requirements.txt".to_string(),
        },
        Language::Go => "go mod tidy".to_string(),
    }
}

/// Normalizes an inbox route so it always starts with `/` and has no trailing
/// slash (except the root). Keeps generated handlers and README consistent.
fn normalize_route(route: &str) -> String {
    let trimmed = route.trim();
    let with_slash = if trimmed.starts_with('/') {
        trimmed.to_string()
    } else {
        format!("/{trimmed}")
    };
    let cleaned = with_slash.trim_end_matches('/');
    if cleaned.is_empty() {
        "/".to_string()
    } else {
        cleaned.to_string()
    }
}

/// Derives a project name from the target directory's file name, sanitized to a
/// safe package identifier. Falls back to `dairo-app`.
fn project_name(target_dir: &Path) -> String {
    let raw = target_dir
        .canonicalize()
        .ok()
        .as_deref()
        .and_then(|p| p.file_name().map(|n| n.to_owned()))
        .or_else(|| target_dir.file_name().map(|n| n.to_owned()))
        .and_then(|n| n.to_str().map(str::to_string))
        .unwrap_or_default();
    let sanitized: String = raw
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                c.to_ascii_lowercase()
            } else {
                '-'
            }
        })
        .collect();
    let trimmed = sanitized.trim_matches('-').to_string();
    if trimmed.is_empty() {
        "dairo-app".to_string()
    } else {
        trimmed
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn normalize_route_adds_leading_slash_and_trims_trailing() {
        assert_eq!(normalize_route("api/dairo/webhook"), "/api/dairo/webhook");
        assert_eq!(normalize_route("/api/dairo/webhook/"), "/api/dairo/webhook");
        assert_eq!(normalize_route("  /hooks  "), "/hooks");
        assert_eq!(normalize_route("/"), "/");
        assert_eq!(normalize_route(""), "/");
    }

    #[test]
    fn project_name_sanitizes_and_falls_back() {
        assert_eq!(project_name(Path::new("/tmp/My App!")), "my-app");
        assert_eq!(project_name(Path::new("/tmp/clean_name")), "clean_name");
    }

    #[test]
    fn next_spec_lists_expected_files_and_modes() {
        let dir = tempdir().unwrap();
        let spec = build_spec(Framework::Next, dir.path(), None, "pnpm");

        let by_rel: BTreeMap<_, _> = spec
            .files
            .iter()
            .map(|f| (f.rel_path.as_str(), f.mode))
            .collect();

        assert_eq!(by_rel.get("lib/dairo.ts"), Some(&WriteMode::CreateIfAbsent));
        assert_eq!(
            by_rel.get("app/api/dairo/webhook/route.ts"),
            Some(&WriteMode::CreateIfAbsent)
        );
        assert_eq!(by_rel.get("package.json"), Some(&WriteMode::MergeJson));
        assert_eq!(
            by_rel.get(".env.example"),
            Some(&WriteMode::AppendLinesPrivate)
        );
        assert_eq!(by_rel.get(".gitignore"), Some(&WriteMode::AppendLines));
        assert_eq!(spec.language, Language::Node);
        assert_eq!(spec.inbox_route, "/api/dairo/webhook");
    }

    #[test]
    fn route_override_flows_into_rendered_handler() {
        let dir = tempdir().unwrap();
        let spec = build_spec(Framework::Fastapi, dir.path(), Some("hooks/in"), "pip");
        assert_eq!(spec.inbox_route, "/hooks/in");
        let handler = spec
            .files
            .iter()
            .find(|f| f.rel_path == "main.py")
            .expect("main.py present");
        assert!(handler.contents.contains("\"/hooks/in\""));
        // requirements.txt should pin the SDK and FastAPI.
        let reqs = spec
            .files
            .iter()
            .find(|f| f.rel_path == "requirements.txt")
            .expect("requirements.txt present");
        assert!(reqs.contents.contains("dairo>="));
        assert!(reqs.contents.contains("fastapi"));
    }

    #[test]
    fn cloudflare_workers_uses_dev_vars_and_pins_go_free() {
        let dir = tempdir().unwrap();
        let spec = build_spec(Framework::CloudflareWorkers, dir.path(), None, "npm");
        assert!(spec.files.iter().any(|f| f.rel_path == ".dev.vars.example"));
        assert!(!spec.files.iter().any(|f| f.rel_path == ".env.example"));
        // wrangler.toml should carry the sanitized project name.
        let wrangler = spec
            .files
            .iter()
            .find(|f| f.rel_path == "wrangler.toml")
            .expect("wrangler.toml present");
        assert!(wrangler.contents.contains("name = "));
    }

    #[test]
    fn go_spec_pins_module_version() {
        let dir = tempdir().unwrap();
        let spec = build_spec(Framework::GoHttp, dir.path(), None, "go");
        let go_mod = spec
            .files
            .iter()
            .find(|f| f.rel_path == "go.mod")
            .expect("go.mod present");
        assert!(go_mod.contents.contains("github.com/dairo-app/dairo-go"));
        assert!(go_mod.contents.contains(GO_SDK_VERSION));
        assert_eq!(spec.language, Language::Go);
    }

    #[test]
    fn env_placeholder_never_contains_a_real_secret() {
        let body = env_placeholder_body(&env_keys_for(Framework::Next));
        assert!(body.contains("DAIRO_API_KEY=\n"));
        assert!(body.contains("DAIRO_WEBHOOK_SECRET=\n"));
        // No value after '=' on any line.
        for line in body.lines() {
            assert!(line.ends_with('='), "env line leaked a value: {line}");
        }
    }
}
