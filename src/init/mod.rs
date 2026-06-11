//! `dairo init <framework>` — a client-only project scaffolder.
//!
//! Drops a working Dairo starter into a developer's project: a configured SDK
//! client, an inbound-webhook handler stub (raw body + signature verification
//! using the SDK's own verify helper), `DAIRO_API_KEY` env wiring, and a README
//! snippet. Templates are embedded in the binary (see `templates.rs`), so this
//! command works offline; the only optional network touch is a friendly
//! `GET /v1/whoami` connectivity check after scaffolding.
//!
//! ## Scale
//! This is a pure local file-I/O operation with zero server-side per-invocation
//! work — 10 or 10M developers running `init` create no backend load. The one
//! optional `whoami` call hits an existing, already-scaled, stateless GET. There
//! is deliberately no "scaffold service": inventing one would add an
//! availability dependency to something that is correctly a local operation.
//!
//! ## Idempotency
//! Files are never silently overwritten. Each planned file is classified
//! (create-if-absent / JSON-merge / append-with-marker) and reconciled with any
//! existing file, mirroring the repo's MCP-installer precedent. `--force`
//! overwrites create-if-absent files; merge/append modes always preserve
//! existing user content and only add what is missing.

mod install;
mod manifest;
mod render;
mod templates;

use std::io::IsTerminal;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde_json::{json, Value};

use crate::api::{self, ApiClient};
use crate::cli::{Framework, InitArgs};
use crate::config::Config;
use crate::fsutil::{ensure_parent_private, write_atomic_0600};
use crate::output::{self, OutputFormat};

use self::install::{InstallOutcome, InstallStatus};
use self::manifest::{build_spec, node_dependencies, FrameworkSpec, PlannedFile, WriteMode};

/// The action taken for a single scaffolded file, reported back to the user.
#[derive(Debug, Clone)]
pub struct InitReport {
    pub rel_path: String,
    /// One of: "created", "merged", "appended", "unchanged", "skipped (exists)",
    /// "overwritten".
    pub action: String,
}

/// Entry point for `dairo init`. Resolves the framework, writes the scaffold,
/// optionally installs dependencies and runs a `whoami` connectivity check, then
/// reports.
pub async fn run(args: InitArgs, json: bool) -> Result<()> {
    let format = OutputFormat::from_json_flag(json);
    let framework = resolve_framework(&args, json)?;

    // Resolve and create the target directory; all writes are confined to it.
    let target_dir = resolve_target_dir(&args.dir)?;

    let language = manifest::Language::for_framework(framework);
    let package_manager =
        install::resolve_package_manager(language, &target_dir, args.package_manager.as_deref());

    let spec = build_spec(
        framework,
        &target_dir,
        args.inbox_route.as_deref(),
        &package_manager,
    );

    // Materialize every planned file, honoring idempotency + --force.
    let mut reports = Vec::with_capacity(spec.files.len());
    for file in &spec.files {
        let report = write_planned_file(file, &target_dir, framework, args.force)?;
        reports.push(report);
    }

    // Optionally install dependencies (best-effort; failure is reported, not fatal).
    let install = install::run_install(
        language,
        framework,
        &target_dir,
        &package_manager,
        args.no_install,
    );

    // Optional best-effort connectivity check.
    let verify = if args.no_verify {
        None
    } else {
        run_whoami_check().await
    };

    let next_steps = next_steps(&spec, &install);

    if format == OutputFormat::Json {
        print_json_manifest(
            framework,
            &target_dir,
            &reports,
            &install,
            &verify,
            &next_steps,
        );
    } else {
        output::print_init(
            framework.as_str(),
            &target_dir.display().to_string(),
            &reports,
            &install_summary(&install),
            verify.as_deref(),
            &next_steps,
        );
    }

    Ok(())
}

/// Resolves the framework from the positional argument and the `--framework`
/// flag alias, erroring if both are set and disagree, or if neither is set and
/// stdout is not a TTY (so CI never hangs on a hidden prompt).
fn resolve_framework(args: &InitArgs, json: bool) -> Result<Framework> {
    match (args.framework, args.framework_flag) {
        (Some(positional), Some(flag)) if positional != flag => anyhow::bail!(
            "framework given twice and they disagree: positional `{positional}` vs --framework `{flag}`; pass it once"
        ),
        (Some(fw), _) | (None, Some(fw)) => Ok(fw),
        (None, None) => {
            // No interactive picker in v1 (dependency-free). Require an explicit
            // value rather than hang CI or guess.
            let _ = json;
            let valid = "next, express, hono, cloudflare-workers, fastapi, flask, go-http";
            if std::io::stdout().is_terminal() {
                anyhow::bail!(
                    "no framework given. Run `dairo init <framework>` with one of: {valid}"
                );
            }
            anyhow::bail!(
                "no framework given (non-interactive). Pass one of: {valid}"
            );
        }
    }
}

/// Creates `--dir` if missing and returns its canonical path. All subsequent
/// writes are checked to stay inside this directory.
fn resolve_target_dir(dir: &Path) -> Result<PathBuf> {
    std::fs::create_dir_all(dir)
        .with_context(|| format!("failed to create target directory {}", dir.display()))?;
    dir.canonicalize()
        .with_context(|| format!("failed to resolve target directory {}", dir.display()))
}

/// Writes one planned file according to its [`WriteMode`], never escaping the
/// target directory and never silently clobbering.
fn write_planned_file(
    file: &PlannedFile,
    target_dir: &Path,
    framework: Framework,
    force: bool,
) -> Result<InitReport> {
    // Path-traversal guard: the resolved parent must stay inside target_dir.
    guard_within_target(&file.path, target_dir)?;

    let action = match file.mode {
        WriteMode::CreateIfAbsent => write_create_if_absent(&file.path, &file.contents, force)?,
        WriteMode::MergeJson => merge_package_json(&file.path, framework)?,
        WriteMode::AppendLines => append_missing_lines(&file.path, &file.contents, false)?,
        WriteMode::AppendLinesPrivate => append_missing_lines(&file.path, &file.contents, true)?,
    };

    Ok(InitReport {
        rel_path: file.rel_path.clone(),
        action,
    })
}

/// Ensures `path`'s parent directory, once created, is still a descendant of
/// `target_dir` (defends against a malicious framework path with `..`).
fn guard_within_target(path: &Path, target_dir: &Path) -> Result<()> {
    let parent = path.parent().unwrap_or(target_dir);
    std::fs::create_dir_all(parent).with_context(|| format!("creating {}", parent.display()))?;
    let canonical_parent = parent
        .canonicalize()
        .with_context(|| format!("resolving {}", parent.display()))?;
    anyhow::ensure!(
        canonical_parent.starts_with(target_dir),
        "refusing to write outside the target directory: {}",
        path.display()
    );
    Ok(())
}

fn write_create_if_absent(path: &Path, contents: &str, force: bool) -> Result<String> {
    if path.exists() {
        let existing =
            std::fs::read(path).with_context(|| format!("reading existing {}", path.display()))?;
        if existing == contents.as_bytes() {
            return Ok("unchanged".to_string());
        }
        if !force {
            return Ok("skipped (exists)".to_string());
        }
        write_file(path, contents)?;
        return Ok("overwritten".to_string());
    }
    write_file(path, contents)?;
    Ok("created".to_string())
}

/// Merges the required `dairo` (and framework) dependencies into an existing
/// `package.json`, preserving all other keys; creates a minimal one if absent.
fn merge_package_json(path: &Path, framework: Framework) -> Result<String> {
    let deps = node_dependencies(framework);

    if !path.exists() {
        let project = path
            .parent()
            .and_then(|p| p.file_name())
            .and_then(|n| n.to_str())
            .unwrap_or("dairo-app");
        let mut value = json!({
            "name": project,
            "private": true,
            "type": "module",
            "dependencies": {},
        });
        set_dependencies(&mut value, &deps);
        write_file(
            path,
            &format!("{}\n", serde_json::to_string_pretty(&value)?),
        )?;
        return Ok("created".to_string());
    }

    let text =
        std::fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
    let mut value: Value = serde_json::from_str(&text).with_context(|| {
        format!(
            "{} is not valid JSON; fix or remove it and re-run",
            path.display()
        )
    })?;

    let before = value.clone();
    set_dependencies(&mut value, &deps);
    if value == before {
        return Ok("unchanged".to_string());
    }
    write_file(
        path,
        &format!("{}\n", serde_json::to_string_pretty(&value)?),
    )?;
    Ok("merged".to_string())
}

/// Sets each `(name, version)` under `dependencies`, only adding a dependency
/// that is not already present (never downgrading/overwriting a user's pin).
fn set_dependencies(value: &mut Value, deps: &[(&str, &str)]) {
    if !value.get("dependencies").is_some_and(Value::is_object) {
        value["dependencies"] = json!({});
    }
    let object = value["dependencies"].as_object_mut().expect("just set");
    for (name, version) in deps {
        object
            .entry((*name).to_string())
            .or_insert_with(|| Value::String((*version).to_string()));
    }
}

/// Appends any candidate line from `contents` that is not already present in the
/// file (matched by env-key prefix for `KEY=` lines, exact-trimmed otherwise),
/// creating the file if absent. Never overwrites an existing value. `private`
/// uses the `0600` atomic writer for secret-capable files.
fn append_missing_lines(path: &Path, contents: &str, private: bool) -> Result<String> {
    let candidate_lines: Vec<&str> = contents.lines().filter(|l| !l.trim().is_empty()).collect();

    let existing = if path.exists() {
        std::fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?
    } else {
        String::new()
    };
    let existing_lines: Vec<String> = existing.lines().map(|l| l.to_string()).collect();

    let mut to_add: Vec<&str> = Vec::new();
    for line in &candidate_lines {
        if !line_present(&existing_lines, line) {
            to_add.push(line);
        }
    }

    if to_add.is_empty() {
        // File already covers every needed line (or did not need changes).
        return Ok(if path.exists() {
            "unchanged".to_string()
        } else {
            // Nothing to add and no file → create empty? Only happens if
            // candidate set is empty; treat as created with the candidates.
            write_lines(path, &candidate_lines, private)?;
            "created".to_string()
        });
    }

    let created = !path.exists();
    let mut merged = existing;
    if !merged.is_empty() && !merged.ends_with('\n') {
        merged.push('\n');
    }
    for line in &to_add {
        merged.push_str(line);
        merged.push('\n');
    }

    if private {
        ensure_parent_private(path)?;
        write_atomic_0600(path, merged.as_bytes())?;
    } else {
        write_file(path, &merged)?;
    }

    Ok(if created {
        "created".to_string()
    } else {
        "appended".to_string()
    })
}

/// Whether `candidate` is already represented in `existing_lines`. For `KEY=...`
/// lines, a matching key (any value) counts as present so we never overwrite a
/// user's existing value; otherwise an exact trimmed match is required.
fn line_present(existing_lines: &[String], candidate: &str) -> bool {
    if let Some((key, _)) = candidate.split_once('=') {
        let key = key.trim();
        if !key.is_empty() && candidate.trim_start().starts_with(|c: char| c != '#') {
            return existing_lines.iter().any(|l| {
                l.split_once('=')
                    .map(|(k, _)| k.trim() == key)
                    .unwrap_or(false)
            });
        }
    }
    existing_lines.iter().any(|l| l.trim() == candidate.trim())
}

fn write_lines(path: &Path, lines: &[&str], private: bool) -> Result<()> {
    let mut body = String::new();
    for line in lines {
        body.push_str(line);
        body.push('\n');
    }
    if private {
        ensure_parent_private(path)?;
        write_atomic_0600(path, body.as_bytes())
    } else {
        write_file(path, &body)
    }
}

/// Writes `contents` to `path`, creating parents. Used for non-secret files.
fn write_file(path: &Path, contents: &str) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("creating {}", parent.display()))?;
    }
    std::fs::write(path, contents).with_context(|| format!("writing {}", path.display()))
}

/// Best-effort `GET /v1/whoami` after scaffolding. Returns a friendly one-line
/// summary, or `None` when there is no key configured or the call fails (offline
/// etc.) — `init` always succeeds at producing files regardless.
async fn run_whoami_check() -> Option<String> {
    let config_path = Config::path().ok()?;
    let config = Config::load_from_path(&config_path).ok()?;
    let api_key = config.resolve_api_key().ok()?;
    let base_url = std::env::var("DAIRO_API_URL")
        .ok()
        .or(config.api_url)
        .unwrap_or_else(|| api::DEFAULT_BASE_URL.to_string());
    let client = ApiClient::new(&base_url, &api_key).ok()?;
    let response = client.whoami().await.ok()?;
    Some(format!(
        "Connected as {} (plan {})",
        response.user_id, response.plan
    ))
}

/// The "Next steps" guidance, tailored to the framework + install state.
fn next_steps(spec: &FrameworkSpec, install: &InstallOutcome) -> Vec<String> {
    let mut steps = Vec::new();
    let env_file = if spec.framework == Framework::CloudflareWorkers {
        ".dev.vars"
    } else {
        ".env"
    };
    steps.push(format!(
        "Set DAIRO_API_KEY (and DAIRO_WEBHOOK_SECRET) in {env_file} — copy from the generated example file. Uses the {} SDK ({}).",
        match spec.language {
            manifest::Language::Node => "JavaScript/TypeScript",
            manifest::Language::Python => "Python",
            manifest::Language::Go => "Go",
        },
        spec.sdk_package,
    ));
    if install.status != InstallStatus::Ran {
        steps.push(format!("Install dependencies: {}", install.command));
    }
    steps.push(format!(
        "Register the webhook: dairo webhook create --url https://<your-host>{} --event message.received",
        spec.inbox_route
    ));
    steps.push(format!(
        "Local dev: dairo listen --forward-to http://localhost:<port>{}",
        spec.inbox_route
    ));
    steps.push(
        "Send a test: POST /api/dairo/send with { inboxId, to, text } (see DAIRO.md).".to_string(),
    );
    steps
}

/// One-line human summary of the install outcome.
fn install_summary(install: &InstallOutcome) -> String {
    match &install.status {
        InstallStatus::Ran => format!("installed dependencies with {}", install.package_manager),
        InstallStatus::Skipped => format!("install skipped — run: {}", install.command),
        InstallStatus::Failed(reason) => {
            format!(
                "install failed ({reason}) — run manually: {}",
                install.command
            )
        }
    }
}

/// Emits the `--json` manifest.
fn print_json_manifest(
    framework: Framework,
    target_dir: &Path,
    reports: &[InitReport],
    install: &InstallOutcome,
    verify: &Option<String>,
    next_steps: &[String],
) {
    let files: Vec<Value> = reports
        .iter()
        .map(|r| json!({ "path": r.rel_path, "action": r.action }))
        .collect();
    let install_status = match &install.status {
        InstallStatus::Ran => "ran",
        InstallStatus::Skipped => "skipped",
        InstallStatus::Failed(_) => "failed",
    };
    let mut install_json = json!({
        "packageManager": install.package_manager,
        "command": install.command,
        "status": install_status,
    });
    if let InstallStatus::Failed(reason) = &install.status {
        install_json["error"] = json!(reason);
    }
    let payload = json!({
        "framework": framework.as_str(),
        "dir": target_dir.display().to_string(),
        "files": files,
        "install": install_json,
        "verify": verify,
        "nextSteps": next_steps,
    });
    println!(
        "{}",
        serde_json::to_string_pretty(&payload).unwrap_or_else(|_| "{}".to_string())
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    fn args(framework: Option<Framework>, dir: PathBuf) -> InitArgs {
        InitArgs {
            framework,
            framework_flag: None,
            dir,
            force: false,
            no_install: true,
            package_manager: None,
            inbox_route: None,
            no_verify: true,
        }
    }

    #[test]
    fn resolve_framework_errors_on_conflict() {
        let mut a = args(Some(Framework::Next), PathBuf::from("."));
        a.framework_flag = Some(Framework::Express);
        let err = resolve_framework(&a, false).expect_err("conflict should error");
        assert!(err.to_string().contains("disagree"));
    }

    #[test]
    fn resolve_framework_flag_only() {
        let mut a = args(None, PathBuf::from("."));
        a.framework_flag = Some(Framework::Hono);
        assert_eq!(resolve_framework(&a, false).unwrap(), Framework::Hono);
    }

    #[tokio::test]
    async fn scaffolds_next_and_is_idempotent() {
        let dir = tempdir().unwrap();
        // First run creates everything.
        run(args(Some(Framework::Next), dir.path().to_path_buf()), false)
            .await
            .unwrap();

        let route = dir.path().join("app/api/dairo/webhook/route.ts");
        assert!(route.exists(), "webhook route should be created");
        let lib = std::fs::read_to_string(dir.path().join("lib/dairo.ts")).unwrap();
        assert!(lib.contains("new Dairo"));
        let pkg = std::fs::read_to_string(dir.path().join("package.json")).unwrap();
        assert!(pkg.contains("\"dairo\""));
        let env = std::fs::read_to_string(dir.path().join(".env.example")).unwrap();
        assert!(env.contains("DAIRO_API_KEY="));
        assert!(!env.contains("dairo_")); // no real secret

        // Mutate a generated file, then re-run without --force: it must NOT be
        // clobbered (idempotent skip).
        std::fs::write(&route, "// user edits\n").unwrap();
        run(args(Some(Framework::Next), dir.path().to_path_buf()), false)
            .await
            .unwrap();
        let after = std::fs::read_to_string(&route).unwrap();
        assert_eq!(after, "// user edits\n", "re-run must not clobber");
    }

    #[tokio::test]
    async fn force_overwrites_existing_files() {
        let dir = tempdir().unwrap();
        run(
            args(Some(Framework::Flask), dir.path().to_path_buf()),
            false,
        )
        .await
        .unwrap();
        let app = dir.path().join("app.py");
        std::fs::write(&app, "# stale\n").unwrap();

        let mut forced = args(Some(Framework::Flask), dir.path().to_path_buf());
        forced.force = true;
        run(forced, false).await.unwrap();
        let after = std::fs::read_to_string(&app).unwrap();
        assert!(after.contains("verify_webhook"), "--force should rewrite");
    }

    #[test]
    fn merge_preserves_existing_package_json_keys() {
        let dir = tempdir().unwrap();
        let pkg = dir.path().join("package.json");
        std::fs::write(
            &pkg,
            r#"{ "name": "mine", "scripts": { "dev": "next dev" }, "dependencies": { "react": "^18.0.0" } }"#,
        )
        .unwrap();

        let action = merge_package_json(&pkg, Framework::Next).unwrap();
        assert_eq!(action, "merged");
        let value: Value = serde_json::from_str(&std::fs::read_to_string(&pkg).unwrap()).unwrap();
        assert_eq!(value["name"], "mine");
        assert_eq!(value["scripts"]["dev"], "next dev");
        assert_eq!(value["dependencies"]["react"], "^18.0.0");
        assert!(value["dependencies"]["dairo"].is_string());

        // Re-merge is a no-op.
        let action = merge_package_json(&pkg, Framework::Next).unwrap();
        assert_eq!(action, "unchanged");
    }

    #[test]
    fn append_env_does_not_overwrite_existing_value() {
        let dir = tempdir().unwrap();
        let env = dir.path().join(".env.example");
        std::fs::write(&env, "DAIRO_API_KEY=already_set\n").unwrap();

        let action =
            append_missing_lines(&env, "DAIRO_API_KEY=\nDAIRO_WEBHOOK_SECRET=\n", true).unwrap();
        assert_eq!(action, "appended");
        let body = std::fs::read_to_string(&env).unwrap();
        assert!(body.contains("DAIRO_API_KEY=already_set"));
        assert!(body.contains("DAIRO_WEBHOOK_SECRET="));
        // The existing API key value was preserved (only one API_KEY line).
        assert_eq!(body.matches("DAIRO_API_KEY").count(), 1);
    }

    #[test]
    fn guard_rejects_paths_outside_target() {
        let dir = tempdir().unwrap();
        let target = dir.path().canonicalize().unwrap();
        let outside = dir.path().parent().unwrap().join("escape.txt");
        let err = guard_within_target(&outside, &target)
            .expect_err("a path outside the target must be rejected");
        assert!(err.to_string().contains("outside the target"));
    }
}
