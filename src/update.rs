//! `dairo update`: download, verify, and install the latest CLI release.
//!
//! The update path is deliberately fail-closed: the current binary is left
//! untouched until the release metadata, archive checksum, extraction, and
//! downloaded binary version have all been verified.

use std::{
    ffi::OsStr,
    fs,
    io::Cursor,
    path::{Path, PathBuf},
    process::Command,
    time::Duration,
};

use anyhow::{bail, Context, Result};
use flate2::read::GzDecoder;
use semver::Version;
use serde_json::json;
use sha2::{Digest, Sha256};
use tempfile::tempdir;

use crate::{cli::UpdateArgs, output::OutputFormat};

const CURRENT_VERSION: &str = env!("CARGO_PKG_VERSION");
const LATEST_RELEASE_API: &str = "https://api.github.com/repos/dairo-app/dairo-cli/releases/latest";
const DOWNLOAD_BASE_URL: &str = "https://dairo.app/downloads/cli/latest";
const USER_AGENT: &str = concat!("dairo-cli/", env!("CARGO_PKG_VERSION"));

#[derive(Debug, Clone)]
struct ReleaseInfo {
    version: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ArchiveKind {
    TarGz,
    Zip,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct PlatformAsset {
    name: &'static str,
    binary_name: &'static str,
    kind: ArchiveKind,
}

struct UpdateOutcome {
    current: String,
    latest: String,
    updated: bool,
    asset: Option<&'static str>,
    installed_path: Option<PathBuf>,
    reason: Option<&'static str>,
}

pub async fn run(args: UpdateArgs, format: OutputFormat) -> Result<()> {
    let outcome = run_update(args.force, format).await?;
    print_outcome(&outcome, format)
}

async fn run_update(force: bool, format: OutputFormat) -> Result<UpdateOutcome> {
    if let Some(installed_path) = current_exe_path() {
        if is_homebrew_cellar_path(&installed_path) {
            if format != OutputFormat::Json {
                println!("Current version: {CURRENT_VERSION}");
                println!("Dairo was installed with Homebrew.");
                println!("Update with:");
                println!("  brew update && brew upgrade dairo");
            }
            return Ok(UpdateOutcome {
                current: CURRENT_VERSION.to_string(),
                latest: CURRENT_VERSION.to_string(),
                updated: false,
                asset: None,
                installed_path: Some(installed_path),
                reason: Some("managed_by_homebrew"),
            });
        }
    }

    let client = http_client()?;
    let release = fetch_latest_release(&client).await?;
    let current = normalized_version(CURRENT_VERSION)?;
    let latest = normalized_version(&release.version)?;

    if latest <= current && !force {
        let reason = if latest == current {
            "up_to_date"
        } else {
            "current_newer"
        };
        if format != OutputFormat::Json {
            println!("Current version: {CURRENT_VERSION}");
            println!("Latest version:  {}", release.version);
            if reason == "up_to_date" {
                println!("You are on the latest Dairo CLI release.");
            } else {
                println!("This Dairo CLI build is newer than the latest published release.");
            }
        }
        return Ok(UpdateOutcome {
            current: CURRENT_VERSION.to_string(),
            latest: release.version,
            updated: false,
            asset: None,
            installed_path: None,
            reason: Some(reason),
        });
    }

    let asset = platform_asset()?;
    if format != OutputFormat::Json {
        println!("Current version: {CURRENT_VERSION}");
        println!("Latest version:  {}", release.version);
        println!("Downloading {}...", asset.name);
    }

    let archive = download(&client, &format!("{DOWNLOAD_BASE_URL}/{}", asset.name)).await?;
    let checksums = download_text(&client, &format!("{DOWNLOAD_BASE_URL}/checksums.txt")).await?;
    verify_checksum(asset.name, &archive, &checksums)?;

    if format != OutputFormat::Json {
        println!("Verifying checksum...");
        println!("Extracting update...");
    }

    let staging = tempdir().context("failed to create update staging directory")?;
    let extracted = extract_binary(&archive, asset, staging.path())?;
    verify_downloaded_binary_version(&extracted, &release.version)?;

    if format != OutputFormat::Json {
        println!("Installing...");
    }

    self_replace::self_replace(&extracted).context(
        "failed to replace the current dairo binary; check permissions for the installed executable",
    )?;
    let installed_path = current_exe_path();

    if format != OutputFormat::Json {
        println!("Dairo CLI updated to {}", release.version);
    }

    Ok(UpdateOutcome {
        current: CURRENT_VERSION.to_string(),
        latest: release.version,
        updated: true,
        asset: Some(asset.name),
        installed_path,
        reason: None,
    })
}

fn print_outcome(outcome: &UpdateOutcome, format: OutputFormat) -> Result<()> {
    if format == OutputFormat::Json {
        let payload = json!({
            "current": outcome.current,
            "latest": outcome.latest,
            "updated": outcome.updated,
            "upToDate": !outcome.updated
                && matches!(outcome.reason, Some("up_to_date" | "current_newer")),
            "asset": outcome.asset,
            "installedPath": outcome.installed_path.as_ref().map(|path| path.display().to_string()),
            "reason": outcome.reason,
        });
        println!("{}", serde_json::to_string_pretty(&payload)?);
    }
    Ok(())
}

fn current_exe_path() -> Option<PathBuf> {
    std::env::current_exe().ok()
}

fn is_homebrew_cellar_path(path: &Path) -> bool {
    let path_string = path.to_string_lossy();
    if path_string == "/opt/homebrew/bin/dairo"
        || path_string == "/usr/local/bin/dairo"
        || path_string == "/home/linuxbrew/.linuxbrew/bin/dairo"
    {
        return true;
    }

    let mut previous_was_cellar = false;
    for component in path.components() {
        let name = component.as_os_str();
        if previous_was_cellar && name == OsStr::new("dairo") {
            return true;
        }
        previous_was_cellar = name == OsStr::new("Cellar");
    }
    false
}

fn http_client() -> Result<reqwest::Client> {
    reqwest::Client::builder()
        .user_agent(USER_AGENT)
        .timeout(Duration::from_secs(60))
        .build()
        .context("failed to build update HTTP client")
}

async fn fetch_latest_release(client: &reqwest::Client) -> Result<ReleaseInfo> {
    let response = client
        .get(LATEST_RELEASE_API)
        .header("Accept", "application/vnd.github+json")
        .send()
        .await
        .context("failed to query the latest Dairo CLI release")?;
    if !response.status().is_success() {
        bail!(
            "latest Dairo CLI release lookup failed with HTTP {}",
            response.status()
        );
    }
    let value: serde_json::Value = response
        .json()
        .await
        .context("failed to parse latest Dairo CLI release metadata")?;
    let version = value
        .get("tag_name")
        .and_then(serde_json::Value::as_str)
        .map(normalize_tag)
        .filter(|tag| !tag.is_empty())
        .context("latest Dairo CLI release metadata did not include tag_name")?;
    Ok(ReleaseInfo { version })
}

async fn download(client: &reqwest::Client, url: &str) -> Result<Vec<u8>> {
    let response = client
        .get(url)
        .send()
        .await
        .with_context(|| format!("failed to download {url}"))?;
    if !response.status().is_success() {
        bail!("download failed with HTTP {} for {url}", response.status());
    }
    response
        .bytes()
        .await
        .map(|bytes| bytes.to_vec())
        .with_context(|| format!("failed to read downloaded bytes from {url}"))
}

async fn download_text(client: &reqwest::Client, url: &str) -> Result<String> {
    let bytes = download(client, url).await?;
    String::from_utf8(bytes).with_context(|| format!("downloaded text from {url} was not UTF-8"))
}

fn verify_checksum(asset_name: &str, archive: &[u8], checksums: &str) -> Result<()> {
    let expected = checksum_for(asset_name, checksums)
        .with_context(|| format!("checksums.txt did not contain {asset_name}"))?;
    let actual = hex_sha256(archive);
    if actual != expected {
        bail!("checksum mismatch for {asset_name}: expected {expected}, got {actual}");
    }
    Ok(())
}

fn checksum_for(asset_name: &str, checksums: &str) -> Option<String> {
    checksums.lines().find_map(|line| {
        let mut parts = line.split_whitespace();
        let hash = parts.next()?;
        let name = parts.next()?;
        if name == asset_name && hash.len() == 64 && hash.chars().all(|ch| ch.is_ascii_hexdigit()) {
            Some(hash.to_ascii_lowercase())
        } else {
            None
        }
    })
}

fn hex_sha256(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    digest.iter().map(|byte| format!("{byte:02x}")).collect()
}

fn extract_binary(archive: &[u8], asset: PlatformAsset, staging_dir: &Path) -> Result<PathBuf> {
    match asset.kind {
        ArchiveKind::TarGz => extract_tar_gz(archive, asset.binary_name, staging_dir),
        ArchiveKind::Zip => extract_zip(archive, asset.binary_name, staging_dir),
    }
}

fn extract_tar_gz(archive: &[u8], binary_name: &str, staging_dir: &Path) -> Result<PathBuf> {
    let decoder = GzDecoder::new(Cursor::new(archive));
    let mut archive = tar::Archive::new(decoder);
    for entry in archive.entries().context("failed to read tar.gz archive")? {
        let mut entry = entry.context("failed to read tar.gz entry")?;
        let path = entry.path().context("failed to read tar.gz entry path")?;
        if path.file_name() != Some(OsStr::new(binary_name)) {
            continue;
        }
        let out_path = staging_dir.join(binary_name);
        entry
            .unpack(&out_path)
            .with_context(|| format!("failed to extract {binary_name}"))?;
        make_executable(&out_path)?;
        return Ok(out_path);
    }
    bail!("release archive did not contain {binary_name}")
}

fn extract_zip(archive: &[u8], binary_name: &str, staging_dir: &Path) -> Result<PathBuf> {
    let reader = Cursor::new(archive);
    let mut archive = zip::ZipArchive::new(reader).context("failed to read zip archive")?;
    for index in 0..archive.len() {
        let mut file = archive
            .by_index(index)
            .context("failed to read zip entry")?;
        let enclosed = match file.enclosed_name() {
            Some(path) => path.to_owned(),
            None => continue,
        };
        if enclosed.file_name() != Some(OsStr::new(binary_name)) {
            continue;
        }
        let out_path = staging_dir.join(binary_name);
        let mut out = fs::File::create(&out_path)
            .with_context(|| format!("failed to create {}", out_path.display()))?;
        std::io::copy(&mut file, &mut out)
            .with_context(|| format!("failed to extract {binary_name}"))?;
        make_executable(&out_path)?;
        return Ok(out_path);
    }
    bail!("release archive did not contain {binary_name}")
}

#[cfg(unix)]
fn make_executable(path: &Path) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;

    let mut permissions = fs::metadata(path)
        .with_context(|| format!("failed to stat {}", path.display()))?
        .permissions();
    permissions.set_mode(0o755);
    fs::set_permissions(path, permissions)
        .with_context(|| format!("failed to mark {} executable", path.display()))
}

#[cfg(not(unix))]
fn make_executable(_path: &Path) -> Result<()> {
    Ok(())
}

fn verify_downloaded_binary_version(binary: &Path, expected: &str) -> Result<()> {
    let output = Command::new(binary)
        .arg("--version")
        .output()
        .with_context(|| format!("failed to execute downloaded binary {}", binary.display()))?;
    if !output.status.success() {
        bail!(
            "downloaded binary failed --version with status {}",
            output.status
        );
    }
    let stdout =
        String::from_utf8(output.stdout).context("downloaded binary --version was not UTF-8")?;
    let reported = stdout.trim();
    let expected_line = format!("dairo {expected}");
    if reported != expected_line {
        bail!("downloaded binary reported `{reported}`, expected `{expected_line}`");
    }
    Ok(())
}

fn platform_asset() -> Result<PlatformAsset> {
    // A musl build must self-update to the musl asset (and vice versa);
    // the libc flavor is a compile-time property of this binary.
    let env = if cfg!(target_env = "musl") {
        "musl"
    } else {
        "gnu"
    };
    platform_asset_for(std::env::consts::OS, std::env::consts::ARCH, env)
}

fn platform_asset_for(os: &str, arch: &str, env: &str) -> Result<PlatformAsset> {
    match (os, arch, env) {
        ("macos", "aarch64", _) => Ok(PlatformAsset {
            name: "dairo-aarch64-apple-darwin.tar.gz",
            binary_name: "dairo",
            kind: ArchiveKind::TarGz,
        }),
        ("macos", "x86_64", _) => Ok(PlatformAsset {
            name: "dairo-x86_64-apple-darwin.tar.gz",
            binary_name: "dairo",
            kind: ArchiveKind::TarGz,
        }),
        ("linux", "aarch64", "musl") => Ok(PlatformAsset {
            name: "dairo-aarch64-unknown-linux-musl.tar.gz",
            binary_name: "dairo",
            kind: ArchiveKind::TarGz,
        }),
        ("linux", "aarch64", _) => Ok(PlatformAsset {
            name: "dairo-aarch64-unknown-linux-gnu.tar.gz",
            binary_name: "dairo",
            kind: ArchiveKind::TarGz,
        }),
        ("linux", "x86_64", "musl") => Ok(PlatformAsset {
            name: "dairo-x86_64-unknown-linux-musl.tar.gz",
            binary_name: "dairo",
            kind: ArchiveKind::TarGz,
        }),
        ("linux", "x86_64", _) => Ok(PlatformAsset {
            name: "dairo-x86_64-unknown-linux-gnu.tar.gz",
            binary_name: "dairo",
            kind: ArchiveKind::TarGz,
        }),
        ("windows", "x86_64", _) => Ok(PlatformAsset {
            name: "dairo-x86_64-pc-windows-msvc.zip",
            binary_name: "dairo.exe",
            kind: ArchiveKind::Zip,
        }),
        ("windows", "aarch64", _) => Ok(PlatformAsset {
            name: "dairo-aarch64-pc-windows-msvc.zip",
            binary_name: "dairo.exe",
            kind: ArchiveKind::Zip,
        }),
        _ => bail!("Dairo CLI self-update is not available for {os}/{arch}"),
    }
}

fn normalized_version(version: &str) -> Result<Version> {
    Version::parse(&normalize_tag(version))
        .with_context(|| format!("release version `{version}` is not valid semver"))
}

fn normalize_tag(tag: &str) -> String {
    tag.trim().trim_start_matches('v').to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn selects_release_asset_for_supported_platforms() {
        assert_eq!(
            platform_asset_for("macos", "aarch64", "gnu").unwrap().name,
            "dairo-aarch64-apple-darwin.tar.gz"
        );
        assert_eq!(
            platform_asset_for("macos", "x86_64", "gnu").unwrap().name,
            "dairo-x86_64-apple-darwin.tar.gz"
        );
        assert_eq!(
            platform_asset_for("linux", "aarch64", "gnu").unwrap().name,
            "dairo-aarch64-unknown-linux-gnu.tar.gz"
        );
        assert_eq!(
            platform_asset_for("linux", "x86_64", "gnu").unwrap().name,
            "dairo-x86_64-unknown-linux-gnu.tar.gz"
        );
        assert_eq!(
            platform_asset_for("linux", "x86_64", "musl").unwrap().name,
            "dairo-x86_64-unknown-linux-musl.tar.gz"
        );
        assert_eq!(
            platform_asset_for("linux", "aarch64", "musl").unwrap().name,
            "dairo-aarch64-unknown-linux-musl.tar.gz"
        );
        assert_eq!(
            platform_asset_for("windows", "x86_64", "gnu").unwrap().name,
            "dairo-x86_64-pc-windows-msvc.zip"
        );
        assert_eq!(
            platform_asset_for("windows", "aarch64", "gnu")
                .unwrap()
                .name,
            "dairo-aarch64-pc-windows-msvc.zip"
        );
    }

    #[test]
    fn rejects_unsupported_platforms() {
        assert!(platform_asset_for("freebsd", "x86_64", "gnu").is_err());
        assert!(platform_asset_for("linux", "riscv64", "gnu").is_err());
    }

    #[test]
    fn parses_checksums_for_exact_asset_name() {
        let checksums = "\
aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa  dairo-x86_64-unknown-linux-gnu.tar.gz
bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb  dairo-aarch64-apple-darwin.tar.gz
";
        assert_eq!(
            checksum_for("dairo-aarch64-apple-darwin.tar.gz", checksums).as_deref(),
            Some("bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb")
        );
        assert_eq!(checksum_for("missing.tar.gz", checksums), None);
    }

    #[test]
    fn compares_versions_as_semver() {
        assert!(normalized_version("v0.1.0").unwrap() > normalized_version("0.0.1").unwrap());
        assert_eq!(
            normalized_version("0.0.1").unwrap(),
            normalized_version("v0.0.1").unwrap()
        );
    }

    #[test]
    fn detects_homebrew_cellar_installs() {
        assert!(is_homebrew_cellar_path(Path::new(
            "/opt/homebrew/Cellar/dairo/0.0.3/bin/dairo"
        )));
        assert!(is_homebrew_cellar_path(Path::new(
            "/home/linuxbrew/.linuxbrew/Cellar/dairo/0.0.3/bin/dairo"
        )));
        assert!(!is_homebrew_cellar_path(Path::new(
            "/Users/luka/.dairo/bin/dairo"
        )));
        assert!(is_homebrew_cellar_path(Path::new(
            "/opt/homebrew/bin/dairo"
        )));
        assert!(is_homebrew_cellar_path(Path::new("/usr/local/bin/dairo")));
        assert!(is_homebrew_cellar_path(Path::new(
            "/home/linuxbrew/.linuxbrew/bin/dairo"
        )));
    }
}
