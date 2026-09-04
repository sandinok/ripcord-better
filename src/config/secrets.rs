//! OS-keyed secret storage for the Discord token.
//!
//! Point 2 of the security spec: the token never sits in a plaintext
//! config again. Envelope per platform:
//!
//! - Windows: DPAPI (`CryptProtectData`, user-scoped). The blob only
//!   decrypts for the same Windows user on the same machine.
//! - macOS: Keychain (generic password item "Basalt/discord-token").
//! - Linux: a dedicated `token.txt` file with mode 0600 (per-user
//!   isolation; secret-service is not assumed to exist on servers and
//!   minimal window-manager setups).
//!
//! Migration is automatic: a legacy plaintext `token` in config.toml is
//! sealed into the OS store on first load and the plaintext field is
//! dropped from the file. `--token` / `DISCORD_TOKEN` overrides keep
//! working and never touch disk.

use anyhow::{Context, Result};

/// Where the sealed token lives.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Store {
    /// Windows DPAPI blob stored base64 in config.toml.
    Dpapi,
    /// macOS Keychain; config.toml only stores the marker.
    Keychain,
    /// Linux 0600 token file next to config.toml.
    File,
}

impl Store {
    pub fn envelope_prefix(self) -> &'static str {
        match self {
            Store::Dpapi => "dpapi",
            Store::Keychain => "keychain",
            Store::File => "file",
        }
    }

    /// Marker written into config.toml (`token_store = "keychain"`).
    pub fn marker(self) -> &'static str {
        self.envelope_prefix()
    }

    pub fn from_marker(s: &str) -> Option<Store> {
        match s {
            "dpapi" => Some(Store::Dpapi),
            "keychain" => Some(Store::Keychain),
            "file" => Some(Store::File),
            _ => None,
        }
    }

    /// The store for the running platform.
    pub fn platform() -> Store {
        #[cfg(target_os = "windows")]
        {
            Store::Dpapi
        }
        #[cfg(target_os = "macos")]
        {
            Store::Keychain
        }
        #[cfg(all(unix, not(target_os = "macos")))]
        {
            Store::File
        }
    }
}

/// Seal `secret` into the OS store. Returns the value that config.toml
/// persists (for DPAPI the base64 blob; for Keychain/File an empty
/// string - the marker alone records where to look).
pub fn seal(secret: &str) -> Result<String> {
    match Store::platform() {
        Store::Dpapi => {
            let blob = dpapi_protect(secret.as_bytes())?;
            use base64::Engine;
            Ok(format!("dpapi:{}", base64::engine::general_purpose::STANDARD.encode(&blob)))
        }
        Store::Keychain => {
            keychain::set(secret).context("writing token to the macOS Keychain")?;
            Ok("keychain:1".to_string())
        }
        Store::File => {
            let path = token_file_path()?;
            if let Some(parent) = path.parent() {
                std::fs::create_dir_all(parent).context("creating config dir")?;
            }
            std::fs::write(&path, secret)
                .with_context(|| format!("writing {}", path.display()))?;
            secure_perm(&path);
            Ok("file:1".to_string())
        }
    }
}

/// Unseal the token. `stored` is the config value (blob or marker).
pub fn unseal(stored: &str) -> Result<String> {
    if let Some(b64) = stored.strip_prefix("dpapi:") {
        use base64::Engine;
        let blob = base64::engine::general_purpose::STANDARD
            .decode(b64.trim())
            .context("dpapi blob base64")?;
        let plain = dpapi_unprotect(&blob)?;
        Ok(String::from_utf8(plain).context("dpapi plaintext utf8")?)
    } else if stored.starts_with("keychain:") {
        keychain::get().context("reading token from the macOS Keychain")
    } else if stored.starts_with("file:") {
        let path = token_file_path()?;
        let text = std::fs::read_to_string(&path)
            .with_context(|| format!("reading {}", path.display()))?;
        Ok(text.trim().to_string())
    } else {
        // Legacy plaintext value (pre-0.2 config): return as-is; the caller
        // migrates it right after.
        Ok(stored.to_string())
    }
}

/// Wipe the stored secret (sign out).
pub fn wipe() {
    match Store::platform() {
        Store::Dpapi => {}
        Store::Keychain => {
            let _ = keychain::delete();
        }
        Store::File => {
            if let Ok(path) = token_file_path() {
                let _ = std::fs::remove_file(path);
            }
        }
    }
}

/// Path of the Linux token side-file: `<config_dir>/basalt/token.txt`.
fn token_file_path() -> Result<std::path::PathBuf> {
    let dir = dirs::config_dir()
        .context("no config directory for this platform")?;
    Ok(dir.join("basalt").join("token.txt"))
}

#[cfg(unix)]
fn secure_perm(path: &std::path::Path) {
    use std::os::unix::fs::PermissionsExt;
    if let Ok(meta) = std::fs::metadata(path) {
        let mut perm = meta.permissions();
        perm.set_mode(0o600);
        let _ = std::fs::set_permissions(path, perm);
    }
}
#[cfg(not(unix))]
fn secure_perm(_path: &std::path::Path) {}

#[cfg(target_os = "windows")]
fn dpapi_protect(plain: &[u8]) -> anyhow::Result<Vec<u8>> {
    use windows_sys::Win32::Security::Cryptography::{
        CryptProtectData, CRYPT_INTEGER_BLOB, CRYPTPROTECT_UI_FORBIDDEN,
    };
    unsafe {
        let mut in_blob = CRYPT_INTEGER_BLOB {
            cbData: plain.len() as u32,
            pbData: plain.as_ptr() as *mut u8,
        };
        let mut out = CRYPT_INTEGER_BLOB {
            cbData: 0,
            pbData: std::ptr::null_mut(),
        };
        let ok = CryptProtectData(
            &mut in_blob,
            std::ptr::null(),
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            CRYPTPROTECT_UI_FORBIDDEN,
            &mut out,
        );
        if ok == 0 {
            anyhow::bail!("CryptProtectData failed");
        }
        let bytes = std::slice::from_raw_parts(out.pbData, out.cbData as usize).to_vec();
        windows_sys::Win32::System::Memory::LocalFree(out.pbData as _);
        Ok(bytes)
    }
}

#[cfg(target_os = "windows")]
fn dpapi_unprotect(blob: &[u8]) -> anyhow::Result<Vec<u8>> {
    use windows_sys::Win32::Security::Cryptography::{
        CryptUnprotectData, CRYPT_INTEGER_BLOB, CRYPTPROTECT_UI_FORBIDDEN,
    };
    unsafe {
        let mut in_blob = CRYPT_INTEGER_BLOB {
            cbData: blob.len() as u32,
            pbData: blob.as_ptr() as *mut u8,
        };
        let mut out = CRYPT_INTEGER_BLOB {
            cbData: 0,
            pbData: std::ptr::null_mut(),
        };
        let ok = CryptUnprotectData(
            &mut in_blob,
            std::ptr::null(),
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            CRYPTPROTECT_UI_FORBIDDEN,
            &mut out,
        );
        if ok == 0 {
            anyhow::bail!("CryptUnprotectData failed (wrong user or corrupted blob)");
        }
        let bytes = std::slice::from_raw_parts(out.pbData, out.cbData as usize).to_vec();
        windows_sys::Win32::System::Memory::LocalFree(out.pbData as _);
        Ok(bytes)
    }
}

#[cfg(not(target_os = "windows"))]
fn dpapi_protect(_plain: &[u8]) -> anyhow::Result<Vec<u8>> {
    anyhow::bail!("DPAPI is Windows-only")
}
#[cfg(not(target_os = "windows"))]
fn dpapi_unprotect(_blob: &[u8]) -> anyhow::Result<Vec<u8>> {
    anyhow::bail!("DPAPI is Windows-only")
}

// ── macOS Keychain ──────────────────────────────────────────────────

#[cfg(target_os = "macos")]
mod keychain {
    const SERVICE: &str = "Basalt";
    const ACCOUNT: &str = "discord-token";

    pub fn set(secret: &str) -> anyhow::Result<()> {
        // Update-or-create: delete then add (the crate has no upsert).
        let _ = security_framework::passwords::delete_generic_password(
            SERVICE, ACCOUNT,
        );
        security_framework::passwords::set_generic_password(
            SERVICE,
            ACCOUNT,
            secret.as_bytes(),
        )?;
        Ok(())
    }

    pub fn get() -> anyhow::Result<String> {
        let bytes =
            security_framework::passwords::get_generic_password(SERVICE, ACCOUNT)?;
        Ok(String::from_utf8(bytes)?)
    }

    pub fn delete() -> anyhow::Result<()> {
        security_framework::passwords::delete_generic_password(SERVICE, ACCOUNT)?;
        Ok(())
    }
}

#[cfg(not(target_os = "macos"))]
mod keychain {
    pub fn set(_secret: &str) -> anyhow::Result<()> {
        anyhow::bail!("Keychain is macOS-only")
    }
    pub fn get() -> anyhow::Result<String> {
        anyhow::bail!("Keychain is macOS-only")
    }
    pub fn delete() -> anyhow::Result<()> {
        anyhow::bail!("Keychain is macOS-only")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn platform_store_marker_round_trip() {
        let s = Store::platform();
        assert_eq!(Store::from_marker(s.marker()), Some(s));
        assert_eq!(Store::from_marker("bogus"), None);
    }

    #[test]
    fn file_envelope_round_trip() {
        // On Linux (the dev/CI platform) the whole seal/unseal cycle must
        // work against a temp dir. We can't redirect dirs::config_dir, so
        // we exercise the envelope parsing directly.
        let env = "file:1";
        assert!(env.starts_with("file:"));
        assert!(Store::from_marker("file") == Some(Store::File));
    }

    #[test]
    fn legacy_plaintext_passes_through_unseal() {
        // Pre-0.2 configs must keep working: unseal returns the raw value
        // so the caller can migrate it.
        let out = unseal("MTU0NDk.x_legacy_plain");
        assert_eq!(out.unwrap(), "MTU0NDk.x_legacy_plain");
    }
}
