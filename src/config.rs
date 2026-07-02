use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::{
    env, fs,
    path::{Path, PathBuf},
    sync::atomic::{AtomicU8, Ordering},
};

use crate::fsutil::{restrict_directory_permissions, write_atomic_0600};

const CONFIG_DIR_NAME: &str = "dairo";
const CONFIG_FILE_NAME: &str = "config.toml";

/// Keychain service name for the stored Dairo API token. A single stable account
/// name is used so the credential is addressable across runs.
const KEYRING_SERVICE: &str = "dairo";
const KEYRING_USER: &str = "api_key";

/// Process-wide credential-storage policy. The secret `api_key` is written to the
/// OS keychain in [`StorageMode::Auto`] (the default for the real binary); all
/// other config stays in the `0600` TOML file. [`StorageMode::FileOnly`] keeps
/// the legacy behavior — the token lives in the TOML file — and is selected by
/// the global `--insecure-storage` flag, when the keychain is unavailable, and
/// by default under `cfg(test)` so unit tests never touch the real OS keychain.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StorageMode {
    Auto,
    FileOnly,
}

// 0 = Auto, 1 = FileOnly. Set once at startup from the `--insecure-storage` flag.
static STORAGE_MODE: AtomicU8 = AtomicU8::new(DEFAULT_STORAGE_MODE);

#[cfg(test)]
const DEFAULT_STORAGE_MODE: u8 = 1; // FileOnly: tests never hit the real keychain.
#[cfg(not(test))]
const DEFAULT_STORAGE_MODE: u8 = 0; // Auto: keychain with file fallback.

/// Selects how the secret token is stored for the rest of the process. Called
/// once from `main` after parsing the global `--insecure-storage` flag; the
/// `load_from_path`/`save_to_path`/`resolve_api_key` signatures stay unchanged.
pub fn set_storage_mode(mode: StorageMode) {
    let value = match mode {
        StorageMode::Auto => 0,
        StorageMode::FileOnly => 1,
    };
    STORAGE_MODE.store(value, Ordering::Relaxed);
}

fn storage_mode() -> StorageMode {
    match STORAGE_MODE.load(Ordering::Relaxed) {
        1 => StorageMode::FileOnly,
        _ => StorageMode::Auto,
    }
}

/// Whether the secret token is currently stored in the OS keychain (Auto mode)
/// rather than the plaintext config file. Used by `dairo doctor` to report the
/// credential location.
pub fn keychain_in_use() -> bool {
    storage_mode() == StorageMode::Auto
}

/// The single keychain entry for the Dairo token, cached for the process.
///
/// A real OS keychain persists by `(service, user)`, so a fresh `Entry` each
/// call would work too; the mock credential store used in tests, however, keeps
/// its (in-memory) secret *inside the `Entry` instance*, so reusing one cached
/// `Entry` is what lets the keychain code path be exercised deterministically in
/// unit tests without touching the real OS keychain.
fn keychain_entry() -> Result<&'static keyring::Entry, keyring::Error> {
    use std::sync::OnceLock;
    static ENTRY: OnceLock<keyring::Entry> = OnceLock::new();
    // `OnceLock::get_or_try_init` is unstable, so build it once and cache, while
    // still surfacing a construction error on the first failed attempt.
    if let Some(entry) = ENTRY.get() {
        return Ok(entry);
    }
    let entry = keyring::Entry::new(KEYRING_SERVICE, KEYRING_USER)?;
    Ok(ENTRY.get_or_init(|| entry))
}

/// Reads the stored token from the OS keychain. Returns `Ok(None)` when there is
/// no entry (a fresh machine) and `Err` only on an actual keychain failure, so
/// callers can fall back to the file gracefully.
fn keychain_get() -> Result<Option<String>, keyring::Error> {
    match keychain_entry()?.get_password() {
        Ok(secret) => Ok(Some(secret)),
        Err(keyring::Error::NoEntry) => Ok(None),
        Err(err) => Err(err),
    }
}

/// Stores the token in the OS keychain.
fn keychain_set(secret: &str) -> Result<(), keyring::Error> {
    keychain_entry()?.set_password(secret)
}

/// Removes the stored token from the OS keychain. A missing entry is success.
fn keychain_delete() -> Result<(), keyring::Error> {
    match keychain_entry()?.delete_credential() {
        Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
        Err(err) => Err(err),
    }
}

#[derive(Debug, Default, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Config {
    #[serde(default)]
    pub api_key: Option<String>,
    #[serde(default)]
    pub api_url: Option<String>,
    /// How `api_key` was obtained. `Some("oauth")` marks a token minted by
    /// `dairo login`; a manually-set token (`dairo auth token set`) leaves this
    /// `None`. Persisted only when set so existing config files keep round-tripping.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub auth_method: Option<String>,
    /// Scopes granted to the stored OAuth token (for `dairo whoami`-style hints).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scopes: Option<Vec<String>>,
    /// RFC3339 timestamp recording when the OAuth token was obtained.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub obtained_at: Option<String>,
}

impl Config {
    /// Clears the stored credential and all OAuth provenance fields, leaving any
    /// non-credential settings (e.g. `api_url`) intact. Used by `dairo logout`.
    pub fn clear_credentials(&mut self) {
        self.api_key = None;
        self.auth_method = None;
        self.scopes = None;
        self.obtained_at = None;
    }
}

impl Config {
    pub fn path() -> Result<PathBuf> {
        let base = dirs::config_dir().context("could not determine user config directory")?;
        Ok(base.join(CONFIG_DIR_NAME).join(CONFIG_FILE_NAME))
    }

    pub fn load_from_path(path: &Path) -> Result<Self> {
        let mut config = if path.exists() {
            let contents = fs::read_to_string(path)
                .with_context(|| format!("failed to read config file {}", path.display()))?;
            toml::from_str::<Self>(&contents)
                .with_context(|| format!("failed to parse config file {}", path.display()))?
        } else {
            Self::default()
        };

        // In keychain mode, the secret lives in the OS keychain, not the TOML
        // file. Only consult the keychain when the file did not already carry a
        // (legacy / pre-migration) token, so a file-stored key always wins and
        // gets migrated on the next save. A keychain read failure is non-fatal:
        // we degrade to whatever the file held (possibly nothing).
        if config.api_key.is_none() && storage_mode() == StorageMode::Auto {
            if let Ok(Some(secret)) = keychain_get() {
                config.api_key = Some(secret);
            }
        }

        Ok(config)
    }

    pub fn save_to_path(&self, path: &Path) -> Result<()> {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).with_context(|| {
                format!("failed to create config directory {}", parent.display())
            })?;
            restrict_directory_permissions(parent)?;
        }

        // `to_persist` is what gets written to the TOML file. In keychain mode we
        // move the secret out of the file body (clearing it from `to_persist`)
        // and into the OS keychain — which also migrates any pre-existing
        // file-stored token. If the keychain write fails we fall back to writing
        // the token into the `0600` file so the user is never locked out.
        let mut to_persist = self.clone();
        if storage_mode() == StorageMode::Auto {
            match &self.api_key {
                Some(secret) if !secret.trim().is_empty() => {
                    if keychain_set(secret).is_ok() {
                        to_persist.api_key = None;
                    }
                    // else: keychain unavailable — leave the token in `to_persist`
                    // so it is written to the file as a fallback.
                }
                // No (or empty) token: clear any previously stored keychain secret
                // so `logout` actually removes the credential from the keychain.
                _ => {
                    let _ = keychain_delete();
                }
            }
        }

        let contents = toml::to_string_pretty(&to_persist).context("failed to serialize config")?;
        write_atomic_0600(path, contents.as_bytes())
            .with_context(|| format!("failed to write config file {}", path.display()))
    }

    pub fn resolve_api_key(&self) -> Result<String> {
        let token = env::var("DAIRO_API_KEY")
            .ok()
            .or_else(|| self.api_key.clone())
            .unwrap_or_default()
            .trim()
            .to_string();

        anyhow::ensure!(
            !token.is_empty(),
            "missing Dairo API token; run `dairo login` to sign in with your browser, \
             set DAIRO_API_KEY, or run `dairo auth token set`"
        );

        Ok(token)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Mutex, Once};
    use tempfile::tempdir;

    // The keyring mock credential builder and the `STORAGE_MODE` flag are both
    // process-global, so every storage-mode-sensitive test (keychain *and* the
    // file round-trip tests) must run serially under one lock. The RAII guard
    // sets the desired mode while held and always restores the cfg(test) default
    // (FileOnly) + clears the mock keychain on drop, so a test can never leak
    // Auto mode into a parallel test.
    static STORAGE_LOCK: Mutex<()> = Mutex::new(());
    static INSTALL_MOCK: Once = Once::new();

    struct StorageGuard(#[allow(dead_code)] std::sync::MutexGuard<'static, ()>);

    impl Drop for StorageGuard {
        fn drop(&mut self) {
            let _ = keychain_delete();
            set_storage_mode(StorageMode::FileOnly);
        }
    }

    /// Enters keychain (Auto) mode behind the global storage lock, installing the
    /// keyring mock store once and starting from a clean credential slot.
    fn keychain_test() -> StorageGuard {
        let guard = STORAGE_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        INSTALL_MOCK.call_once(|| {
            keyring::set_default_credential_builder(keyring::mock::default_credential_builder());
        });
        let _ = keychain_delete();
        set_storage_mode(StorageMode::Auto);
        StorageGuard(guard)
    }

    /// Enters FileOnly mode behind the same global storage lock, so the legacy
    /// file round-trip tests cannot observe Auto mode set by a parallel keychain
    /// test.
    fn file_only_test() -> StorageGuard {
        let guard = STORAGE_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        set_storage_mode(StorageMode::FileOnly);
        StorageGuard(guard)
    }

    #[test]
    fn keychain_mode_keeps_secret_out_of_the_toml_file() {
        let _guard = keychain_test();
        let dir = tempdir().unwrap();
        let path = dir.path().join("config.toml");
        let config = Config {
            api_key: Some("dairo_live_secret".to_string()),
            api_url: Some("https://example.test".to_string()),
            ..Config::default()
        };

        config.save_to_path(&path).unwrap();

        // The plaintext token must not appear in the on-disk file...
        let on_disk = fs::read_to_string(&path).unwrap();
        assert!(
            !on_disk.contains("dairo_live_secret"),
            "token leaked into the config file: {on_disk}"
        );
        assert!(on_disk.contains("https://example.test"));
        // ...but the keychain holds it.
        assert_eq!(
            keychain_get().unwrap().as_deref(),
            Some("dairo_live_secret")
        );

        // ...and a load transparently re-hydrates it from the keychain.
        let loaded = Config::load_from_path(&path).unwrap();
        assert_eq!(loaded.api_key.as_deref(), Some("dairo_live_secret"));
        assert_eq!(loaded.api_url.as_deref(), Some("https://example.test"));
    }

    #[test]
    fn keychain_mode_migrates_a_file_stored_token() {
        let _guard = keychain_test();
        let dir = tempdir().unwrap();
        let path = dir.path().join("config.toml");
        // Simulate a legacy config written before keychain storage existed.
        fs::write(&path, "api_key = \"dairo_legacy_token\"\n").unwrap();

        // First load picks the token up from the file (keychain is empty).
        let loaded = Config::load_from_path(&path).unwrap();
        assert_eq!(loaded.api_key.as_deref(), Some("dairo_legacy_token"));

        // Saving migrates it into the keychain and strips it from the file.
        loaded.save_to_path(&path).unwrap();
        let on_disk = fs::read_to_string(&path).unwrap();
        assert!(!on_disk.contains("dairo_legacy_token"));
        assert_eq!(
            keychain_get().unwrap().as_deref(),
            Some("dairo_legacy_token")
        );
    }

    #[test]
    fn keychain_mode_logout_clears_the_keychain_secret() {
        let _guard = keychain_test();
        let dir = tempdir().unwrap();
        let path = dir.path().join("config.toml");
        let config = Config {
            api_key: Some("dairo_live_secret".to_string()),
            ..Config::default()
        };
        config.save_to_path(&path).unwrap();
        assert!(keychain_get().unwrap().is_some());

        // Clearing the credential (logout) and saving wipes the keychain entry.
        let mut cleared = Config::load_from_path(&path).unwrap();
        cleared.clear_credentials();
        cleared.save_to_path(&path).unwrap();
        assert!(keychain_get().unwrap().is_none());
    }

    #[test]
    fn resolve_api_key_prefers_env_over_stored_value() {
        // DAIRO_API_KEY always wins, independent of where the stored key lives.
        let config = Config {
            api_key: Some("stored".to_string()),
            ..Config::default()
        };
        // Safety: single-threaded within this test; no other test reads this var.
        unsafe { env::set_var("DAIRO_API_KEY", "from-env") };
        let resolved = config.resolve_api_key().unwrap();
        unsafe { env::remove_var("DAIRO_API_KEY") };
        assert_eq!(resolved, "from-env");
    }

    #[test]
    fn parses_config_file() {
        let config: Config = toml::from_str(
            r#"
api_key = "dairo_test_123"
api_url = "https://example.test"
"#,
        )
        .unwrap();

        assert_eq!(config.api_key.as_deref(), Some("dairo_test_123"));
        assert_eq!(config.api_url.as_deref(), Some("https://example.test"));
    }

    #[test]
    fn saves_and_loads_config_file() {
        // This test relies on the token round-tripping through the TOML file, so
        // it pins FileOnly mode under the shared storage lock.
        let _guard = file_only_test();
        let dir = tempdir().unwrap();
        let path = dir.path().join("nested").join("config.toml");
        let config = Config {
            api_key: Some("dairo_test_123".to_string()),
            api_url: Some("https://example.test".to_string()),
            ..Config::default()
        };

        config.save_to_path(&path).unwrap();
        let loaded = Config::load_from_path(&path).unwrap();

        assert_eq!(loaded, config);
    }

    #[cfg(unix)]
    #[test]
    fn saved_config_uses_private_permissions() {
        use std::os::unix::fs::PermissionsExt;

        let _guard = file_only_test();
        let dir = tempdir().unwrap();
        let path = dir.path().join("nested").join("config.toml");
        let config = Config {
            api_key: Some("dairo_test_123".to_string()),
            api_url: None,
            ..Config::default()
        };

        config.save_to_path(&path).unwrap();

        let parent_mode = fs::metadata(path.parent().unwrap())
            .unwrap()
            .permissions()
            .mode()
            & 0o777;
        let file_mode = fs::metadata(&path).unwrap().permissions().mode() & 0o777;

        assert_eq!(parent_mode, 0o700);
        assert_eq!(file_mode, 0o600);
    }

    #[test]
    fn round_trips_oauth_provenance_fields() {
        let _guard = file_only_test();
        let dir = tempdir().unwrap();
        let path = dir.path().join("config.toml");
        let config = Config {
            api_key: Some("dairo_live_abc".to_string()),
            api_url: None,
            auth_method: Some("oauth".to_string()),
            scopes: Some(vec!["messages:read".to_string(), "messages:send".to_string()]),
            obtained_at: Some("2026-06-20T12:00:00Z".to_string()),
        };

        config.save_to_path(&path).unwrap();
        let loaded = Config::load_from_path(&path).unwrap();

        assert_eq!(loaded, config);
        assert_eq!(loaded.auth_method.as_deref(), Some("oauth"));
        assert_eq!(loaded.obtained_at.as_deref(), Some("2026-06-20T12:00:00Z"));
    }

    #[test]
    fn clear_credentials_wipes_token_and_oauth_fields_but_keeps_api_url() {
        let mut config = Config {
            api_key: Some("dairo_live_abc".to_string()),
            api_url: Some("https://example.test".to_string()),
            auth_method: Some("oauth".to_string()),
            scopes: Some(vec!["admin".to_string()]),
            obtained_at: Some("2026-06-20T12:00:00Z".to_string()),
        };

        config.clear_credentials();

        assert_eq!(config.api_key, None);
        assert_eq!(config.auth_method, None);
        assert_eq!(config.scopes, None);
        assert_eq!(config.obtained_at, None);
        // Non-credential settings are preserved.
        assert_eq!(config.api_url.as_deref(), Some("https://example.test"));
    }

    #[test]
    fn legacy_config_without_oauth_fields_still_parses() {
        // A config written before the OAuth fields existed must keep loading.
        let config: Config = toml::from_str(r#"api_key = "dairo_test_123""#).unwrap();
        assert_eq!(config.api_key.as_deref(), Some("dairo_test_123"));
        assert_eq!(config.auth_method, None);
        assert_eq!(config.scopes, None);
        assert_eq!(config.obtained_at, None);
    }

    #[test]
    fn missing_config_is_empty() {
        let _guard = file_only_test();
        let dir = tempdir().unwrap();
        let path = dir.path().join("missing.toml");

        assert_eq!(Config::load_from_path(&path).unwrap(), Config::default());
    }

    #[test]
    fn failed_atomic_replace_cleans_temp_file() {
        let _guard = file_only_test();
        let dir = tempdir().unwrap();
        let parent = dir.path().join("nested");
        fs::create_dir_all(&parent).unwrap();
        let path = parent.join("config.toml");
        fs::create_dir(&path).unwrap();
        let temp_path = parent.join(format!(".config.toml.tmp.{}", std::process::id()));
        let config = Config {
            api_key: Some("dairo_test_123".to_string()),
            api_url: None,
            ..Config::default()
        };

        assert!(config.save_to_path(&path).is_err());
        assert!(
            !temp_path.exists(),
            "failed rename should not leave a token-bearing temporary config file"
        );
    }
}
