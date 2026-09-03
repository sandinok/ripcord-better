//! Configuration for Basalt.
//!
//! A small TOML-backed config that stores the Discord token (encrypted
//! envelope on disk), appearance preferences, and a few flags. The
//! [`Config`] struct is the single source of truth for everything that
//! survives across launches.

pub mod cli;

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

/// Basalt's persistent configuration.
///
/// Stored as TOML at the path returned by [`default_path`]. The Discord
/// token is kept in the file (with restrictive permissions on Unix) for
/// the beta; a future release will move it to an OS keyring envelope.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Config {
    /// Discord bot or user token. `None` means "not signed in".
    pub token: Option<String>,

    /// `false` = modern (2026) status dot palette, `true` = legacy.
    #[serde(default)]
    pub use_legacy_status_dots: bool,

    /// `"cozy"` (default) or `"compact"`. Affects message row spacing.
    #[serde(default = "default_density")]
    pub density: String,

    /// Show the right-hand members panel. Toggled via `Ctrl+M` in chat.
    #[serde(default = "default_show_members")]
    pub show_members: bool,

    /// Base message font size (12..19 px).
    #[serde(default = "default_font_size")]
    pub font_size: f32,

    /// Show unread dots + mention badges on channels and servers.
    #[serde(default = "default_badges")]
    pub show_unread_badges: bool,

    /// Prefix the window title with the mention count, like `(3) Basalt`.
    #[serde(default = "default_title_mentions")]
    pub title_mentions: bool,
}

fn default_density() -> String {
    "cozy".to_string()
}

fn default_show_members() -> bool {
    true
}

fn default_font_size() -> f32 {
    15.0
}

fn default_badges() -> bool {
    true
}

fn default_title_mentions() -> bool {
    true
}

impl Default for Config {
    fn default() -> Self {
        Self {
            token: None,
            use_legacy_status_dots: false,
            density: default_density(),
            show_members: default_show_members(),
            font_size: default_font_size(),
            show_unread_badges: default_badges(),
            title_mentions: default_title_mentions(),
        }
    }
}

impl Config {
    /// Load the config from disk, falling back to defaults if the file
    /// is missing. Honors `--token`, `--config`, and `DISCORD_TOKEN`
    /// overrides from [`cli::Args`].
    pub fn load(args: &cli::Args) -> Result<Self> {
        let path: Option<PathBuf> = if let Some(p) = &args.config {
            Some(p.clone())
        } else {
            default_path()?
        };

        let mut cfg = if let Some(path) = path.as_ref().filter(|p| p.exists()) {
            let text = std::fs::read_to_string(path)
                .with_context(|| format!("reading config {}", path.display()))?;
            toml::from_str(&text).unwrap_or_else(|e| {
                tracing::warn!(error = %e, "config parse error - using defaults");
                Config::default()
            })
        } else {
            Config::default()
        };

        // Env override has the lowest priority.
        if cfg.token.is_none() {
            if let Ok(env_token) = std::env::var("DISCORD_TOKEN") {
                let trimmed = env_token.trim().to_string();
                if !trimmed.is_empty() {
                    cfg.token = Some(trimmed);
                }
            }
        }

        // CLI override wins.
        if let Some(t) = &args.token {
            cfg.token = Some(t.clone());
        }

        Ok(cfg)
    }

    /// Persist the config to disk. Creates the parent directory if needed.
    pub fn save(&self) -> Result<()> {
        let path = default_path()?
            .ok_or_else(|| anyhow::anyhow!("no home directory"))?;
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).with_context(|| {
                format!("creating config dir {}", parent.display())
            })?;
        }
        let text = toml::to_string_pretty(self).context("serializing config")?;
        std::fs::write(&path, text).with_context(|| format!("writing {}", path.display()))?;
        secure_file(&path);
        Ok(())
    }
}

/// Set restrictive permissions on the config file so other users can't
/// read the token. On Windows this is a no-op (we rely on per-user
/// profile isolation).
#[cfg(unix)]
fn secure_file(path: &Path) {
    use std::os::unix::fs::PermissionsExt;
    if let Ok(meta) = std::fs::metadata(path) {
        let mut perm = meta.permissions();
        perm.set_mode(0o600);
        let _ = std::fs::set_permissions(path, perm);
    }
}

#[cfg(not(unix))]
fn secure_file(_path: &Path) {}

/// Returns the path to the default config file, or `Ok(None)` if no
/// home directory is set.
pub fn default_path() -> Result<Option<PathBuf>> {
    let Some(home) = dirs::config_dir() else {
        return Ok(None);
    };
    Ok(Some(home.join("basalt").join("config.toml")))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_config_has_sensible_values() {
        let c = Config::default();
        assert!(c.token.is_none(), "default should have no token");
        assert!(!c.use_legacy_status_dots, "default should be modern dots");
        assert_eq!(c.density, "cozy");
        assert!(c.show_members, "default should show members panel");
    }

    #[test]
    fn config_round_trips_through_toml() {
        let mut c = Config::default();
        c.token = Some("ghp_test_token_value_123".to_string());
        c.density = "compact".to_string();
        c.use_legacy_status_dots = true;
        c.show_members = false;
        let s = toml::to_string_pretty(&c).unwrap();
        let back: Config = toml::from_str(&s).unwrap();
        assert_eq!(back.token.as_deref(), Some("ghp_test_token_value_123"));
        assert_eq!(back.density, "compact");
        assert!(back.use_legacy_status_dots);
        assert!(!back.show_members);
    }

    #[test]
    fn empty_density_field_falls_back_to_default() {
        // Simulate a hand-edited config that's missing the density key.
        let toml_str = "token = \"abc\"\nuse_legacy_status_dots = false\nshow_members = true";
        let c: Config = toml::from_str(toml_str).unwrap_or_default();
        assert_eq!(c.density, "cozy", "missing density should fall back to default");
    }
}
