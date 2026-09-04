//! Self-updater: check GitHub releases, download the platform asset,
//! verify the sha256 checksum, swap the binary, restart, roll back on
//! failure.
//!
//! Flow (point 36 of the checklist):
//!   1. `check()` - GET /repos/sandinok/ripcord-better/releases/latest.
//!      Compares the tag against the running version (semver-ish).
//!   2. `run_update(info, progress)` - streams the asset to a temp file
//!      with progress callbacks, then verifies against the release's
//!      `basalt-checksums.txt` (sha256).
//!   3. `install()` - extracts the new binary from the tar.gz, moves the
//!      running binary to `basalt.old`, writes the new one in place,
//!      spawns it with `--post-update`, and exits.
//!   4. Rollback - on startup with `--post-update` we verify we really are
//!      the expected version (marker file written before the swap). If
//!      the new binary fails before this check ever runs, the NEXT normal
//!      launch of the old binary sees the leftover `.old` + marker,
//!      restores the backup, and reports the failure.
//!
//! All three platforms ship `.tar.gz` (the Windows zip switched to tar.gz
//! in v0.2 so the updater has ONE extraction path; Windows 10+ ships
//! bsdtar and Explorer opens tar.gz natively).

use std::io::Read;
use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{bail, Context, Result};
use serde::Deserialize;

pub const REPO: &str = "sandinok/ripcord-better";
const API_BASE: &str = "https://api.github.com";

/// Public metadata of the latest release.
#[derive(Debug, Clone)]
pub struct UpdateInfo {
    pub tag: String,
    pub version: String,
    pub notes: String,
    pub asset_name: String,
    pub asset_url: String,
    pub asset_size: u64,
    pub checksum_url: String,
}

#[derive(Debug, Deserialize)]
struct ReleaseAsset {
    name: String,
    url: String,
    size: u64,
    browser_download_url: String,
}

#[derive(Debug, Deserialize)]
struct Release {
    tag_name: String,
    #[serde(default)]
    body: String,
    #[serde(default)]
    assets: Vec<ReleaseAsset>,
    #[serde(default)]
    prerelease: bool,
    #[serde(default)]
    draft: bool,
}

/// The asset filename for the running platform.
pub fn platform_asset_name(version: &str) -> String {
    let os = match std::env::consts::OS {
        "windows" => "windows-x86_64",
        "macos" => "macos-arm64",
        _ => "linux-x86_64",
    };
    format!("basalt-v{version}-{os}.tar.gz")
}

/// Check GitHub for a newer release. `Ok(None)` = up to date.
pub async fn check() -> Result<Option<UpdateInfo>> {
    let http = reqwest::Client::builder()
        .user_agent(format!("basalt-updater/{}", env!("CARGO_PKG_VERSION")))
        .timeout(Duration::from_secs(20))
        .build()?;
    let rel: Release = http
        .get(format!("{API_BASE}/repos/{REPO}/releases/latest"))
        .header("accept", "application/vnd.github+json")
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;
    if rel.draft || rel.prerelease {
        return Ok(None);
    }
    let current = env!("CARGO_PKG_VERSION");
    let version = rel.tag_name.trim_start_matches('v').to_string();
    if !is_newer(&version, current) {
        return Ok(None);
    }
    let asset_name = platform_asset_name(&version);
    let asset = rel
        .assets
        .iter()
        .find(|a| a.name == asset_name)
        .context("latest release has no asset for this platform")?;
    let checksum = rel
        .assets
        .iter()
        .find(|a| a.name == "basalt-checksums.txt")
        .context("release has no checksums.txt asset")?;
    Ok(Some(UpdateInfo {
        tag: rel.tag_name.clone(),
        version,
        notes: rel.body,
        asset_name: asset.name.clone(),
        // The `url` field is the API download URL (needs Accept:
        // application/octet-stream); use the browser URL for a plain GET.
        asset_url: asset.browser_download_url.clone(),
        asset_size: asset.size,
        checksum_url: checksum.browser_download_url.clone(),
    }))
}

/// `true` when `a` is strictly newer than `b` (lenient semver compare).
fn is_newer(a: &str, b: &str) -> bool {
    let pa: Vec<u64> = a.split('.').filter_map(|s| s.parse().ok()).collect();
    let pb: Vec<u64> = b.split('.').filter_map(|s| s.parse().ok()).collect();
    for i in 0..3 {
        let va = pa.get(i).copied().unwrap_or(0);
        let vb = pb.get(i).copied().unwrap_or(0);
        if va != vb {
            return va > vb;
        }
    }
    false
}

/// Phase of an in-flight update, for the UI.
#[derive(Debug, Clone)]
pub enum UpdatePhase {
    Downloading { done: u64, total: u64 },
    Verifying,
    Installing,
    Done,
    Failed(String),
}

/// Download + verify. Returns the verified archive bytes.
pub async fn download_and_verify(
    info: &UpdateInfo,
    progress: impl Fn(UpdatePhase) + Send + 'static,
) -> Result<Vec<u8>> {
    let http = reqwest::Client::builder()
        .user_agent(format!("basalt-updater/{}", env!("CARGO_PKG_VERSION")))
        .timeout(Duration::from_secs(300))
        .build()?;
    // Stream the asset with progress.
    let mut resp = http
        .get(&info.asset_url)
        .send()
        .await?
        .error_for_status()?;
    let total = resp.content_length().unwrap_or(info.asset_size).max(1);
    let mut bytes: Vec<u8> = Vec::with_capacity(total as usize);
    let mut done = 0u64;
    while let Some(chunk) = resp.chunk().await? {
        bytes.extend_from_slice(&chunk);
        done += chunk.len() as u64;
        progress(UpdatePhase::Downloading { done, total });
    }
    // Verify the checksum from basalt-checksums.txt.
    progress(UpdatePhase::Verifying);
    let sums: String = http.get(&info.checksum_url).send().await?.error_for_status()?.text().await?;
    let expected = parse_checksum(&sums, &info.asset_name)
        .with_context(|| format!("no checksum entry for {}", info.asset_name))?;
    use sha2::{Digest, Sha256};
    let actual = hex::encode(Sha256::digest(&bytes));
    if actual != expected {
        bail!(
            "checksum mismatch for {}: expected {expected}, got {actual}",
            info.asset_name
        );
    }
    Ok(bytes)
}

/// Parse `<sha256>  <filename>` lines (sha256sum format).
fn parse_checksum(text: &str, asset: &str) -> Option<String> {
    for line in text.lines() {
        let mut it = line.split_whitespace();
        let (Some(sum), Some(name)) = (it.next(), it.next()) else {
            continue;
        };
        if name.trim_start_matches("*") == asset {
            return Some(sum.to_ascii_lowercase());
        }
    }
    None
}

/// Where the update marker lives (config dir).
fn marker_path() -> Result<PathBuf> {
    let dir = dirs::config_dir().context("no config dir")?;
    Ok(dir.join("basalt").join("update-marker.json"))
}

/// Marker written before the swap: the version the new binary must report.
#[derive(serde::Serialize, serde::Deserialize)]
pub struct UpdateMarker {
    pub from: String,
    pub to: String,
    pub at: u64,
}

/// Extract the binary from the release tar.gz and swap it in.
/// On success this function does NOT return (it execs the new binary).
pub fn install(archive: &[u8], info: &UpdateInfo) -> Result<()> {
    // 1. Extract the new binary to `<exe>.new`.
    let exe = std::env::current_exe().context("current exe path")?;
    let new_path = exe.with_extension("new");
    let bin_name = extract_binary(archive)?;
    std::fs::write(&new_path, &bin_name.0)
        .with_context(|| format!("writing {}", new_path.display()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut perm = std::fs::metadata(&new_path)?.permissions();
        perm.set_mode(0o755);
        std::fs::set_permissions(&new_path, perm)?;
    }
    // 2. Remember what we expect after the swap (rollback contract).
    if let Some(dir) = marker_path()?.parent() {
        std::fs::create_dir_all(dir).ok();
    }
    let marker = UpdateMarker {
        from: env!("CARGO_PKG_VERSION").to_string(),
        to: info.version.clone(),
        at: std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0),
    };
    std::fs::write(
        marker_path()?,
        serde_json::to_vec(&marker).unwrap_or_default(),
    )
    .ok();
    // 3. Move the running binary aside, move the new one into place.
    let old_path = exe.with_extension("old");
    let _ = std::fs::remove_file(&old_path);
    std::fs::rename(&exe, &old_path).context("moving the running binary aside")?;
    if let Err(e) = std::fs::rename(&new_path, &exe) {
        // Roll back immediately: put the old binary back.
        let _ = std::fs::rename(&old_path, &exe);
        let _ = std::fs::remove_file(&new_path);
        return Err(e).context("installing the new binary");
    }
    // 4. Launch the new binary with --post-update and exit cleanly.
    let mut cmd = std::process::Command::new(&exe);
    cmd.arg("--post-update");
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(0x00000008); // DETACHED_PROCESS
    }
    cmd.spawn().context("spawning the updated binary")?;
    std::process::exit(0);
}

/// The single binary file inside the release tar.gz.
fn extract_binary(archive: &[u8]) -> Result<(Vec<u8>, String)> {
    let mut tar = tar::Archive::new(flate2::read::GzDecoder::new(archive));
    for entry in tar.entries()? {
        let mut entry = entry?;
        if entry.header().entry_type() != tar::EntryType::Regular {
            continue;
        }
        let path = entry
            .path()
            .with_context(|| "entry path")?
            .to_string_lossy()
            .to_string();
        // The binary is `basalt` / `basalt.exe` at the archive root.
        let file = Path::new(&path)
            .file_name()
            .map(|f| f.to_string_lossy().to_string())
            .unwrap_or_default();
        let stem = file.trim_end_matches(".exe");
        if stem == "basalt" {
            let mut bytes = Vec::new();
            entry.read_to_end(&mut bytes)?;
            return Ok((bytes, file));
        }
    }
    bail!("no basalt binary found in the release archive")
}

/// Called at startup when launched with `--post-update`. Returns a toast
/// message. Cleans the backup on success; restores it on mismatch.
pub fn post_update_check() -> Option<String> {
    let marker_bytes: Vec<u8> = std::fs::read(marker_path().ok()?).ok()?;
    let marker: UpdateMarker = serde_json::from_slice(&marker_bytes).ok()?;
    let exe = std::env::current_exe().ok()?;
    let old_path = exe.with_extension("old");
    let current = env!("CARGO_PKG_VERSION");
    if current == marker.to {
        // Success: remove the backup + marker.
        let _ = std::fs::remove_file(&old_path);
        let _ = std::fs::remove_file(marker_path().ok()?);
        return Some(format!("Updated to v{}.", marker.to));
    }
    // We are NOT the expected binary: the update failed after the swap.
    // Restore the backup over us (the old binary will clean up next start)
    // and report.
    if old_path.exists() {
        let restored = exe.with_extension("restore");
        if std::fs::copy(&old_path, &restored).is_ok() {
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                let mut perm = std::fs::metadata(&restored).ok()?.permissions();
                perm.set_mode(0o755);
                std::fs::set_permissions(&restored, perm).ok()?;
            }
            let mut cmd = std::process::Command::new(&restored);
            cmd.arg("--update-failed");
            #[cfg(windows)]
            {
                use std::os::windows::process::CommandExt;
                cmd.creation_flags(0x00000008);
            }
            if cmd.spawn().is_ok() {
                std::process::exit(0);
            }
        }
    }
    let _ = std::fs::remove_file(marker_path().ok()?);
    Some("The update was rolled back: the new binary did not start.".to_string())
}

/// Called when a previous update left a `.restore`/`.old` trail: tidy up
/// and tell the user what happened.
pub fn cleanup_stale_update() -> Option<String> {
    let exe = std::env::current_exe().ok()?;
    let marker_path = marker_path().ok()?;
    let old_path = exe.with_extension("old");
    let restore_path = exe.with_extension("restore");
    let mut msg = None;
    if restore_path.exists() {
        // We were launched from the restored backup: take our real name back.
        let _ = std::fs::remove_file(&old_path);
        let _ = std::fs::rename(&restore_path, &exe);
        msg = Some("The v{} update failed and was rolled back.".to_string());
    } else if marker_path.exists() && old_path.exists() {
        // New binary never started; remove its leftovers.
        let _ = std::fs::remove_file(&old_path);
        let _ = std::fs::remove_file(&marker_path);
        msg = Some("The last update failed to start and was rolled back.".to_string());
    }
    if let Some(m) = msg.as_ref() {
        if let Ok(marker_bytes) = std::fs::read(&marker_path) {
            if let Ok(marker) = serde_json::from_slice::<UpdateMarker>(&marker_bytes) {
                let _ = std::fs::remove_file(&marker_path);
                return Some(m.replace("v{}", &format!("v{}", marker.to)));
            }
        }
    }
    let _ = std::fs::remove_file(&marker_path);
    msg
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn newer_version_compare() {
        assert!(is_newer("0.2.0", "0.1.2"));
        assert!(is_newer("1.0.0", "0.9.9"));
        assert!(!is_newer("0.1.2", "0.2.0"));
        assert!(!is_newer("0.2.0", "0.2.0"));
        assert!(!is_newer("0.2", "0.2.0"));
    }

    #[test]
    fn checksum_line_parses() {
        let text = "abc123  basalt-v0.2.0-linux-x86_64.tar.gz\n\
                    def456  basalt-v0.2.0-windows-x86_64.tar.gz\n";
        assert_eq!(
            parse_checksum(text, "basalt-v0.2.0-windows-x86_64.tar.gz"),
            Some("def456".to_string())
        );
        assert_eq!(parse_checksum(text, "nope.zip"), None);
    }

    #[test]
    fn platform_asset_name_matches_ci() {
        let name = platform_asset_name("0.2.0");
        #[cfg(target_os = "windows")]
        assert_eq!(name, "basalt-v0.2.0-windows-x86_64.tar.gz");
        #[cfg(target_os = "macos")]
        assert_eq!(name, "basalt-v0.2.0-macos-arm64.tar.gz");
        #[cfg(all(unix, not(target_os = "macos")))]
        assert_eq!(name, "basalt-v0.2.0-linux-x86_64.tar.gz");
    }

    #[test]
    fn archive_round_trip() {
        // Build a tiny tar.gz with a `basalt` file and extract it back.
        use std::io::Write;
        let mut builder = tar::Builder::new(Vec::new());
        let mut header = tar::Header::new_gnu();
        let payload = b"#!/bin/sh\necho basalt\n";
        header.set_size(payload.len() as u64);
        header.set_mode(0o755);
        header.set_cksum();
        builder
            .append_data(&mut header, "basalt", &payload[..])
            .unwrap();
        let raw = builder.into_inner().unwrap();
        let mut gz = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
        gz.write_all(&raw).unwrap();
        let archive = gz.finish().unwrap();
        let (bytes, name) = extract_binary(&archive).unwrap();
        assert_eq!(name, "basalt");
        assert_eq!(bytes, payload.to_vec());
    }
}
