//! Configuration for Basalt.
//!
//! A small TOML-backed config that stores appearance preferences and a
//! few flags. The Discord token lives in the OS secret store (see
//! [`secrets`]): DPAPI on Windows, Keychain on macOS, a 0600 file on
//! Linux. `config.toml` only keeps an envelope marker.

pub mod cli;
pub mod secrets;

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

/// Basalt's persistent configuration.
///
/// Stored as TOML at the path returned by [`default_path`]. The `token`
/// field, when present, is an envelope ("dpapi:...", "keychain:1",
/// "file:1") or - only for pre-0.2 configs - a legacy plaintext value
/// that gets migrated to the OS store on first load.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Config {
    /// Sealed-token envelope, `None` means "not signed in".
    pub token: Option<String>,

    /// Session-only token override (`--token` / `DISCORD_TOKEN`).
    /// Never serialized, never persisted.
    #[serde(skip)]
    pub session_override: Option<String>,

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

    /// DM channels discovered in past sessions, by id. Bot accounts get no
    /// DM list from READY or REST, so the client remembers the DMs it has
    /// seen (via gateway events) and re-fetches them on startup. User
    /// accounts get the full list from READY anyway; the cache is harmless.
    #[serde(default)]
    pub dm_channel_ids: Vec<String>,

    /// DMs pinned to the top of the home list (context menu "Pin DM").
    #[serde(default)]
    pub pinned_dms: Vec<String>,

    /// Server folders: each folder groups guild ids.
    #[serde(default)]
    pub guild_folders: Vec<GuildFolder>,

    /// Sidebar width in px (resizable, persisted).
    #[serde(default = "default_sidebar_width")]
    pub sidebar_width: f32,

    /// Members panel width in px (resizable, persisted).
    #[serde(default = "default_members_width")]
    pub members_width: f32,

    /// Enter sends the message (Discord default). Off = Enter is a
    /// newline, Ctrl+Enter sends.
    #[serde(default = "default_true")]
    pub enter_to_send: bool,

    /// Check for updates on startup + manual check button.
    #[serde(default = "default_true")]
    pub auto_updates: bool,

    /// Notification sounds (mention ping).
    #[serde(default = "default_true")]
    pub notification_sounds: bool,

    /// Desktop-style toasts for mentions while the window is unfocused.
    #[serde(default = "default_true")]
    pub desktop_notifications: bool,

    /// One-shot startup notice (updater result), never persisted.
    #[serde(skip)]
    pub startup_notice: Option<String>,
}

/// A server folder in the guild bar.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GuildFolder {
    pub name: String,
    #[serde(default)]
    pub color: Option<String>,
    pub guild_ids: Vec<String>,
}

fn default_sidebar_width() -> f32 {
    240.0
}
fn default_members_width() -> f32 {
    240.0
}
fn default_true() -> bool {
    true
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
            session_override: None,
            use_legacy_status_dots: false,
            density: default_density(),
            show_members: default_show_members(),
            font_size: default_font_size(),
            show_unread_badges: default_badges(),
            title_mentions: default_title_mentions(),
            dm_channel_ids: Vec::new(),
            pinned_dms: Vec::new(),
            guild_folders: Vec::new(),
            sidebar_width: default_sidebar_width(),
            members_width: default_members_width(),
            enter_to_send: default_true(),
            auto_updates: default_true(),
            notification_sounds: default_true(),
            desktop_notifications: default_true(),
            startup_notice: None,
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
                    cfg.session_override = Some(trimmed);
                }
            }
        }

        // CLI override wins.
        if let Some(t) = &args.token {
            cfg.session_override = Some(t.clone());
        }

        // Migrate a legacy plaintext token (pre-0.2 config) into the OS
        // secret store right now, so nothing plaintext survives on disk.
        if let Some(stored) = cfg.token.clone() {
            if !stored.starts_with("dpapi:")
                && !stored.starts_with("keychain:")
                && !stored.starts_with("file:")
            {
                match secrets::seal(stored.trim()) {
                    Ok(envelope) => {
                        cfg.token = Some(envelope);
                        tracing::info!("legacy plaintext token migrated to the OS secret store");
                    }
                    Err(e) => {
                        tracing::warn!(error = %e, "could not migrate token to the OS store");
                    }
                }
            }
        }

        Ok(cfg)
    }

    /// The usable (unsealed) token, or `None` when signed out. The
    /// session override (`--token` / env) wins over the stored envelope.
    pub fn plain_token(&self) -> Option<String> {
        if let Some(t) = self.session_override.as_deref() {
            let t = t.trim();
            if !t.is_empty() {
                return Some(t.to_string());
            }
        }
        let stored = self.token.as_deref()?;
        match secrets::unseal(stored) {
            Ok(plain) => {
                let plain = plain.trim().to_string();
                if plain.is_empty() {
                    None
                } else {
                    Some(plain)
                }
            }
            Err(e) => {
                tracing::warn!(error = %e, "stored token could not be unsealed");
                None
            }
        }
    }

    /// Seal and remember a token (sign-in).
    pub fn set_plain_token(&mut self, plain: &str) {
        match secrets::seal(plain.trim()) {
            Ok(envelope) => self.token = Some(envelope),
            Err(e) => {
                // Never fall back to plaintext-on-disk: keep it in memory
                // for this session and say why.
                tracing::warn!(error = %e, "OS secret store unavailable; token kept in memory only");
                self.session_override = Some(plain.trim().to_string());
            }
        }
    }

    /// Forget the stored token entirely (sign out).
    pub fn clear_token(&mut self) {
        self.token = None;
        self.session_override = None;
        secrets::wipe();
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
    fn legacy_plain_token_migrates_to_envelope() {
        // A pre-0.2 config with a plaintext token must come back sealed.
        let toml_str = "token = \"definitely-a-legacy-plaintext-value\"\nshow_members = true\n";
        let mut c: Config = toml::from_str(toml_str).unwrap_or_default();
        // Simulate load()'s migration branch.
        if let Some(stored) = c.token.clone() {
            if !stored.starts_with("dpapi:")
                && !stored.starts_with("keychain:")
                && !stored.starts_with("file:")
            {
                if let Ok(env) = secrets::seal(stored.trim()) {
                    c.token = Some(env);
                }
            }
        }
        let sealed = c.token.unwrap();
        assert!(
            sealed.starts_with("dpapi:")
                || sealed.starts_with("keychain:")
                || sealed.starts_with("file:"),
            "sealed envelope expected, got: {sealed}"
        );
    }

    #[test]
    fn config_round_trips_through_toml() {
        let mut c = Config::default();
        c.token = Some("dpapi:ZmFrZQ==".to_string());
        c.density = "compact".to_string();
        c.use_legacy_status_dots = true;
        c.show_members = false;
        let s = toml::to_string_pretty(&c).unwrap();
        let back: Config = toml::from_str(&s).unwrap();
        assert_eq!(back.token.as_deref(), Some("dpapi:ZmFrZQ=="));
        assert_eq!(back.session_override, None, "session override never persists");
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
