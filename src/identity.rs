//! Client identity shared by the REST and gateway layers.
//!
//! One rule above all: **what we claim must be consistent everywhere.**
//! The HTTP User-Agent, the gateway IDENTIFY properties and the websocket
//! handshake headers describe the same client, with an OS that matches the
//! machine we actually run on. A client that claims "Chrome on Windows" in
//! the gateway but ships a custom User-Agent over HTTP, or says "macos"
//! while running on Linux, is exactly the fingerprint mismatch that risk
//! systems flag.
//!
//! Shape follows what the reference open-source clients send (discordo,
//! Legcord): a current Chromium build on the real OS, a fresh client build
//! number, and a per-launch `client_launch_id`.

/// Chromium version we present. Keep in sync with the UA string below and
/// with the browser_version in the gateway IDENTIFY properties.
const CHROME_MAJOR: u32 = 152;

/// Discord client build number, extracted from the official login page
/// (`"BUILD_NUMBER":"..."` in discord.com/login). Fetched 2026-09-03;
/// refresh when Discord ships a noticeably different client.
pub const CLIENT_BUILD_NUMBER: u32 = 606_747;

/// User-Agent for the web-client identity (user-token sessions).
/// The OS section matches the real machine so the UA, the IDENTIFY
/// `os` field and reality all agree.
pub fn web_user_agent() -> String {
    let os = match std::env::consts::OS {
        "windows" => "Windows NT 10.0; Win64; x64",
        "macos" => "Macintosh; Intel Mac OS X 10_15_7",
        _ => "X11; Linux x86_64",
    };
    format!(
        "Mozilla/5.0 ({os}) AppleWebKit/537.36 (KHTML, like Gecko) \
         Chrome/{CHROME_MAJOR}.0.0.0 Safari/537.36"
    )
}

/// Placeholder UA installed as the reqwest *default* header (Cloudflare
/// wants one on every request); real requests override it per call.
pub const PLACEHOLDER_UA: &str = "Basalt (https://github.com/sandinok/basalt, 0.1.1)";

/// User-Agent for bot sessions. Discord's documented convention for API
/// clients; honest and expected for a bot token.
pub fn bot_user_agent() -> String {
    "Basalt (https://github.com/sandinok/basalt, 0.1.1)".to_string()
}

/// The `properties` object for a user-session gateway IDENTIFY: the same
/// fields the official web client sends, with the OS, browser version and
/// User-Agent consistent with the REST layer.
pub fn web_identify_properties() -> serde_json::Value {
    let os = match std::env::consts::OS {
        "windows" => "Windows",
        "macos" => "Mac OS X",
        _ => "Linux",
    };
    let os_version = match std::env::consts::OS {
        "windows" => "10",
        "macos" => "10.15.7",
        _ => "",
    };
    serde_json::json!({
        "os": os,
        "browser": "Chrome",
        "device": "",
        "system_locale": "en-US",
        "browser_user_agent": web_user_agent(),
        "browser_version": format!("{CHROME_MAJOR}.0.0.0"),
        "os_version": os_version,
        "referrer": "",
        "referring_domain": "",
        "referrer_current": "",
        "referring_domain_current": "",
        "release_channel": "stable",
        "client_build_number": CLIENT_BUILD_NUMBER,
        "client_event_source": serde_json::Value::Null,
        // Random per-launch id, like the official client's telemetry.
        "client_launch_id": launch_id(),
        "is_fast_connect": true,
        "has_client_mods": false,
    })
}

/// OS properties for the minimal bot IDENTIFY: the real OS and an honest
/// client name. Bot accounts are expected to be custom clients.
pub fn bot_identify_properties() -> serde_json::Value {
    let os = match std::env::consts::OS {
        "windows" => "Windows",
        "macos" => "Mac OS X",
        _ => "Linux",
    };
    serde_json::json!({
        "os": os,
        "browser": "Basalt",
        "device": "Basalt",
    })
}

/// Random v4-shaped id for `client_launch_id` (one per process, generated
/// once and reused so all connections from one launch share it, like the
/// official client does).
pub fn launch_id() -> String {
    use once_cell::sync::Lazy;
    use std::sync::atomic::{AtomicU64, Ordering};
    static CTR: AtomicU64 = AtomicU64::new(0);
    static ID: Lazy<String> = Lazy::new(|| {
        let mut b = [0u8; 16];
        let _ = getrandom::fill(&mut b);
        // Force into RFC 4122 v4 shape (version + variant bits).
        b[6] = (b[6] & 0x0F) | 0x40;
        b[8] = (b[8] & 0x3F) | 0x80;
        let _ = CTR.fetch_add(1, Ordering::Relaxed);
        format!(
            "{:02x}{:02x}{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}{:02x}{:02x}{:02x}{:02x}",
            b[0], b[1], b[2], b[3], b[4], b[5], b[6], b[7], b[8], b[9], b[10], b[11], b[12], b[13], b[14], b[15]
        )
    });
    ID.clone()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn web_ua_matches_os() {
        let ua = web_user_agent();
        match std::env::consts::OS {
            "windows" => assert!(ua.contains("Windows NT")),
            "macos" => assert!(ua.contains("Macintosh")),
            _ => assert!(ua.contains("X11; Linux")),
        }
        assert!(ua.contains(&format!("Chrome/{CHROME_MAJOR}.")));
    }

    #[test]
    fn identify_properties_are_consistent() {
        let props = web_identify_properties();
        assert_eq!(
            props["browser_user_agent"].as_str().unwrap(),
            web_user_agent()
        );
        assert_eq!(
            props["client_build_number"].as_u64().unwrap(),
            CLIENT_BUILD_NUMBER as u64
        );
        // The gateway os field must agree with the OS section in the UA.
        let ua_os = if web_user_agent().contains("Windows NT") {
            "Windows"
        } else if web_user_agent().contains("Macintosh") {
            "Mac OS X"
        } else {
            "Linux"
        };
        assert_eq!(props["os"].as_str().unwrap(), ua_os);
        assert!(props["client_launch_id"].as_str().unwrap().len() == 36);
    }

    #[test]
    fn launch_id_is_stable_per_process() {
        assert_eq!(launch_id(), launch_id());
    }
}
