use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::{
    env, fs,
    path::{Path, PathBuf},
};

use crate::fsutil::{restrict_directory_permissions, write_atomic_0600};

const CONFIG_DIR_NAME: &str = "dairo";
const CONFIG_FILE_NAME: &str = "config.toml";

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
        if !path.exists() {
            return Ok(Self::default());
        }

        let contents = fs::read_to_string(path)
            .with_context(|| format!("failed to read config file {}", path.display()))?;
        toml::from_str(&contents)
            .with_context(|| format!("failed to parse config file {}", path.display()))
    }

    pub fn save_to_path(&self, path: &Path) -> Result<()> {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).with_context(|| {
                format!("failed to create config directory {}", parent.display())
            })?;
            restrict_directory_permissions(parent)?;
        }
        let contents = toml::to_string_pretty(self).context("failed to serialize config")?;
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
            "missing Dairo API token; set DAIRO_API_KEY or run `dairo auth token set`"
        );

        Ok(token)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

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
        let dir = tempdir().unwrap();
        let path = dir.path().join("config.toml");
        let config = Config {
            api_key: Some("dairo_live_abc".to_string()),
            api_url: None,
            auth_method: Some("oauth".to_string()),
            scopes: Some(vec!["mail:read".to_string(), "mail:send".to_string()]),
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
        let dir = tempdir().unwrap();
        let path = dir.path().join("missing.toml");

        assert_eq!(Config::load_from_path(&path).unwrap(), Config::default());
    }

    #[test]
    fn failed_atomic_replace_cleans_temp_file() {
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
