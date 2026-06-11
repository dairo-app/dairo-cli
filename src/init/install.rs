//! Package-manager detection and the (optional) dependency install step.
//!
//! Detection looks for lockfiles/manifests in the target directory and falls
//! back to the language default. The install step shells out to the resolved
//! package manager; it is skipped entirely with `--no-install`, in which case
//! the caller prints the manual command from the README instead.

use std::path::Path;
use std::process::Command;

use anyhow::{Context, Result};

use crate::cli::Framework;
use crate::init::manifest::{node_dependencies, Language};

/// The outcome of the install step, for the report.
#[derive(Debug, Clone)]
pub struct InstallOutcome {
    /// The package manager that was used or would be used.
    pub package_manager: String,
    /// The command string (for display / manual-run guidance).
    pub command: String,
    /// What happened: ran, skipped (`--no-install`), or failed (best-effort).
    pub status: InstallStatus,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InstallStatus {
    Ran,
    Skipped,
    Failed(String),
}

/// Resolves the package manager to use: an explicit `--package-manager` wins;
/// otherwise detect from lockfiles in `dir`, falling back to the language
/// default.
pub fn resolve_package_manager(
    language: Language,
    dir: &Path,
    override_pm: Option<&str>,
) -> String {
    if let Some(pm) = override_pm {
        return pm.to_string();
    }
    match language {
        Language::Node => detect_node_pm(dir),
        Language::Python => detect_python_pm(dir),
        Language::Go => "go",
    }
    .to_string()
}

fn detect_node_pm(dir: &Path) -> &'static str {
    if dir.join("pnpm-lock.yaml").exists() {
        "pnpm"
    } else if dir.join("yarn.lock").exists() {
        "yarn"
    } else if dir.join("bun.lockb").exists() || dir.join("bun.lock").exists() {
        "bun"
    } else {
        // package-lock.json or nothing → npm
        "npm"
    }
}

fn detect_python_pm(dir: &Path) -> &'static str {
    if dir.join("poetry.lock").exists() {
        "poetry"
    } else if dir.join("uv.lock").exists() {
        "uv"
    } else {
        "pip"
    }
}

/// Runs the install step for the scaffolded project, unless `no_install`.
///
/// Returns the [`InstallOutcome`] for reporting. A failed install is *not* a hard
/// error (the files are already written and valid); it is reported as
/// [`InstallStatus::Failed`] so the user can run it manually.
pub fn run_install(
    language: Language,
    framework: Framework,
    dir: &Path,
    package_manager: &str,
    no_install: bool,
) -> InstallOutcome {
    let (program, args) = install_argv(language, framework, package_manager);
    let command = format!("{program} {}", args.join(" "));

    if no_install {
        return InstallOutcome {
            package_manager: package_manager.to_string(),
            command,
            status: InstallStatus::Skipped,
        };
    }

    match run_command(&program, &args, dir) {
        Ok(()) => InstallOutcome {
            package_manager: package_manager.to_string(),
            command,
            status: InstallStatus::Ran,
        },
        Err(error) => InstallOutcome {
            package_manager: package_manager.to_string(),
            command,
            status: InstallStatus::Failed(error.to_string()),
        },
    }
}

/// The `(program, args)` for the install command. Public for unit tests.
pub fn install_argv(
    language: Language,
    framework: Framework,
    package_manager: &str,
) -> (String, Vec<String>) {
    match language {
        Language::Node => {
            let deps: Vec<String> = node_dependencies(framework)
                .iter()
                .map(|(name, _)| name.to_string())
                .collect();
            let (program, verb) = match package_manager {
                "pnpm" => ("pnpm", "add"),
                "yarn" => ("yarn", "add"),
                "bun" => ("bun", "add"),
                _ => ("npm", "install"),
            };
            let mut args = vec![verb.to_string()];
            args.extend(deps);
            (program.to_string(), args)
        }
        Language::Python => match package_manager {
            "poetry" => (
                "poetry".to_string(),
                vec![
                    "add".to_string(),
                    "-r".to_string(),
                    "requirements.txt".to_string(),
                ],
            ),
            "uv" => (
                "uv".to_string(),
                vec![
                    "pip".to_string(),
                    "install".to_string(),
                    "-r".to_string(),
                    "requirements.txt".to_string(),
                ],
            ),
            _ => (
                "pip".to_string(),
                vec![
                    "install".to_string(),
                    "-r".to_string(),
                    "requirements.txt".to_string(),
                ],
            ),
        },
        Language::Go => (
            "go".to_string(),
            vec!["mod".to_string(), "tidy".to_string()],
        ),
    }
}

fn run_command(program: &str, args: &[String], dir: &Path) -> Result<()> {
    let status = Command::new(program)
        .args(args)
        .current_dir(dir)
        .status()
        .with_context(|| format!("failed to run `{program}` (is it installed and on PATH?)"))?;
    anyhow::ensure!(status.success(), "`{program}` exited with status {status}");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::tempdir;

    #[test]
    fn detects_pnpm_from_lockfile() {
        let dir = tempdir().unwrap();
        fs::write(dir.path().join("pnpm-lock.yaml"), "").unwrap();
        assert_eq!(
            resolve_package_manager(Language::Node, dir.path(), None),
            "pnpm"
        );
    }

    #[test]
    fn node_defaults_to_npm() {
        let dir = tempdir().unwrap();
        assert_eq!(
            resolve_package_manager(Language::Node, dir.path(), None),
            "npm"
        );
    }

    #[test]
    fn override_wins_over_detection() {
        let dir = tempdir().unwrap();
        fs::write(dir.path().join("yarn.lock"), "").unwrap();
        assert_eq!(
            resolve_package_manager(Language::Node, dir.path(), Some("bun")),
            "bun"
        );
    }

    #[test]
    fn python_detects_uv_and_defaults_pip() {
        let dir = tempdir().unwrap();
        assert_eq!(
            resolve_package_manager(Language::Python, dir.path(), None),
            "pip"
        );
        fs::write(dir.path().join("uv.lock"), "").unwrap();
        assert_eq!(
            resolve_package_manager(Language::Python, dir.path(), None),
            "uv"
        );
    }

    #[test]
    fn install_argv_includes_framework_dep() {
        let (program, args) = install_argv(Language::Node, Framework::Express, "pnpm");
        assert_eq!(program, "pnpm");
        assert_eq!(args[0], "add");
        assert!(args.contains(&"dairo".to_string()));
        assert!(args.contains(&"express".to_string()));
    }

    #[test]
    fn no_install_reports_skipped_without_running() {
        let dir = tempdir().unwrap();
        let outcome = run_install(Language::Go, Framework::GoHttp, dir.path(), "go", true);
        assert_eq!(outcome.status, InstallStatus::Skipped);
        assert_eq!(outcome.command, "go mod tidy");
    }
}
