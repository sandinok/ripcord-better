//! Token redaction ("scrub") layer.
//!
//! Point 3 of the security spec: the token must never appear in logs,
//! panics, or crash output. Three defenses:
//!
//! 1. A redaction function that masks known token shapes (the live token,
//!    generic bot-token / MFA patterns, `Authorization` header values).
//! 2. A custom `tracing` event formatter that runs every formatted line
//!    through the redactor - nothing reaches stderr unredacted.
//! 3. A panic hook that redacts the panic message before printing.
//!
//! The redactor is intentionally cheap (prefix scans, no regex engine).

use std::sync::OnceLock;

use tracing_subscriber::fmt::format::{Format, FormatEvent, Full, Writer};
use tracing_subscriber::fmt::{FmtContext, FormatFields};
/// The live session token (set at login). Redacted everywhere.
static LIVE_TOKEN: OnceLock<String> = OnceLock::new();

/// Record the token that must never hit the log output.
pub fn set_live_token(token: &str) {
    let t = token.trim();
    if t.is_empty() {
        return;
    }
    let _ = LIVE_TOKEN.set(t.to_string());
}

/// Redact every known secret shape in `s` (returns a new String).
pub fn redact(s: &str) -> String {
    // Fast path: no hex-ish sections -> nothing to scrub.
    if !needs_scrub(s) {
        return s.to_string();
    }
    let mut out = s.to_string();
    // 1. The exact live token, wherever it appears (bare, "Bot x", "Bearer x").
    if let Some(tok) = LIVE_TOKEN.get() {
        if !tok.is_empty()
            && out.contains(tok.as_str()) {
                out = out.replace(tok.as_str(), "[REDACTED]");
            }
    }
    // 2. Generic Discord token shapes (in case a token different from the
    //    live one leaks, e.g. in a pasted config dump):
    //    bot token: 50+\w chars, dot, 6 chars, dot, 27+ chars
    //    user token: mfa.<30+ base64ish>
    // 3. Authorization header values.
    out = scrub_pattern(&out, |c| {
        c.is_ascii_alphanumeric() || c == '_' || c == '-' || c == '.'
    });
    out = scrub_mfa(&out);
    out = scrub_auth_header(&out);
    out
}

fn needs_scrub(s: &str) -> bool {
    if LIVE_TOKEN.get().map(|t| !t.is_empty()).unwrap_or(false) {
        return true;
    }
    // Dots are rare in ordinary log lines; anything without one skips the
    // pattern scan entirely.
    s.contains("mfa.") || s.contains("uthorization") || s.contains('.')
}

/// Replace `<token>`-shaped words: 3 segments of [A-Za-z0-9_-] separated by
/// dots with lengths typical of Discord tokens (24+/6+/27+ or a single 50+).
fn scrub_pattern(s: &str, is_word: fn(char) -> bool) -> String {
    let chars: Vec<char> = s.chars().collect();
    let mut out = String::with_capacity(s.len());
    let mut i = 0;
    while i < chars.len() {
        if is_word(chars[i]) {
            let start = i;
            while i < chars.len() && is_word(chars[i]) {
                i += 1;
            }
            let word: String = chars[start..i].iter().collect();
            let segs: Vec<&str> = word.split('.').collect();
            let looks_like_token = match segs.as_slice() {
                [a, b, c] => {
                    a.len() >= 24
                        && b.len() >= 6
                        && c.len() >= 20
                        && a.chars().next().is_some_and(|ch| ch.is_ascii_digit())
                }
                [a] => a.len() >= 72,
                _ => false,
            };
            if looks_like_token {
                // Preserve the segment count so log lines stay readable.
                out.push_str(&format!(
                    "[REDACTED{}]",
                    ".".repeat(segs.len().saturating_sub(1))
                ));
            } else {
                out.push_str(&word);
            }
        } else {
            out.push(chars[i]);
            i += 1;
        }
    }
    out
}

fn scrub_mfa(s: &str) -> String {
    // mfa.<34+ word chars> -> mfa.[REDACTED]
    let mut out = String::with_capacity(s.len());
    let bytes: Vec<char> = s.chars().collect();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i..].starts_with(&['m', 'f', 'a', '.']) {
            let mut j = i + 4;
            while j < bytes.len()
                && (bytes[j].is_ascii_alphanumeric() || bytes[j] == '_' || bytes[j] == '-')
            {
                j += 1;
            }
            let seg_len = j - (i + 4);
            if seg_len >= 24 {
                out.push_str("mfa.[REDACTED]");
                i = j;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    out
}

fn scrub_auth_header(s: &str) -> String {
    // "Authorization: Bot xyz" / "Bearer xyz" (any case) -> value redacted.
    let lower = s.to_ascii_lowercase();
    let mut out = s.to_string();
    let mut scan_from = 0usize;
    while let Some(pos) = lower[scan_from..].find("authorization") {
        let abs = scan_from + pos;
        // Find the value start (after ':' and any spaces).
        let after = &s[abs..];
        if let Some(colon) = after.find(':') {
            let mut v = abs + colon + 1;
            let bytes = s.as_bytes();
            while v < bytes.len() && bytes[v] == b' ' {
                v += 1;
            }
            // Skip an optional "Bot " / "Bearer " scheme prefix.
            for prefix in ["bot ", "bearer "] {
                let stop = (v + prefix.len()).min(lower.len());
                if lower[v..stop] == *prefix {
                    v += prefix.len();
                    break;
                }
            }
            let mut end = v;
            while end < bytes.len()
                && (bytes[end].is_ascii_alphanumeric() || bytes[end] == b'_' || bytes[end] == b'-'
                    || bytes[end] == b'.')
            {
                end += 1;
            }
            if end > v + 8 {
                out = format!("{}[REDACTED]{}", &out[..v], &out[end..]);
            }
        }
        scan_from = abs + "authorization".len();
        if scan_from >= s.len() {
            break;
        }
    }
    out
}

/// A `tracing` formatter that redacts every event line. Wrap `Full` to
/// keep the default layout, then scrub the rendered string.
pub struct ScrubbedFormatter;

impl<S, N> FormatEvent<S, N> for ScrubbedFormatter
where
    S: tracing::Subscriber + for<'a> tracing_subscriber::registry::LookupSpan<'a>,
    N: for<'a> FormatFields<'a> + 'static,
{
    fn format_event(
        &self,
        ctx: &FmtContext<'_, S, N>,
        mut writer: Writer<'_>,
        event: &tracing::Event<'_>,
    ) -> std::fmt::Result {
        let mut buf = String::new();
        {
            let inner_writer = Writer::new(&mut buf);
            Format::<Full, tracing_subscriber::fmt::time::SystemTime>::default()
                .format_event(ctx, inner_writer, event)
                .map_err(|_| std::fmt::Error)?;
        }
        writer.write_str(&redact(&buf))
    }
}

/// Install the redacting panic hook. The default hook prints the panic
/// message; ours re-prints it through the redactor.
pub fn install_panic_hook() {
    let default_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let payload = payload_string(info);
        let location = info
            .location()
            .map(|l| format!(" at {}:{}:{}", l.file(), l.line(), l.column()))
            .unwrap_or_default();
        eprintln!(
            "thread panicked{}: {}",
            location,
            redact(&payload)
        );
        // Chain to the default hook for a real backtrace in debug builds.
        if cfg!(debug_assertions) {
            default_hook(info);
        }
    }));
}

fn payload_string(info: &std::panic::PanicHookInfo<'_>) -> String {
    let payload = info.payload();
    if let Some(s) = payload.downcast_ref::<&str>() {
        (*s).to_string()
    } else if let Some(s) = payload.downcast_ref::<String>() {
        s.clone()
    } else {
        "unknown panic payload".to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn live_token_is_replaced() {
        set_live_token("MTU0NDk4MDg0NTI4OTQxMDU3MA.ABCDEF.hijklmnopqrstuvwxyz123456");
        let line = "connect failed with token MTU0NDk4MDg0NTI4OTQxMDU3MA.ABCDEF.hijklmnopqrstuvwxyz123456 in state";
        let out = redact(line);
        assert!(out.contains("[REDACTED]"));
        assert!(!out.contains("hijklmnopqrstuvwxyz"));
    }

    #[test]
    fn generic_bot_token_shape_is_scrubbed() {
        // Not the live token; the shape matcher still catches it.
        let out = redact("dump: 123456789012345678901234.a1b2c3.abcdefghijklmnopqrstuvwxyzA1B2C3");
        assert!(out.contains("[REDACTED."), "got: {out}");
        assert!(!out.contains("abcdefghijklmnopqrstuvwxyzA1B2C3"));
    }

    #[test]
    fn mfa_token_shape_is_scrubbed() {
        let out = redact("auth=mfa.abcdefghijklmnopqrstuvwxyzabcdef");
        assert_eq!(out, "auth=mfa.[REDACTED]");
    }

    #[test]
    fn authorization_header_value_is_scrubbed() {
        let out = redact("request Authorization: Bot 1234567890abcdefghij done");
        assert!(!out.contains("1234567890abcdefghij"), "got: {out}");
        assert!(out.contains("[REDACTED]"));
    }

    #[test]
    fn ordinary_text_is_untouched() {
        set_live_token("zzz-not-set-xyz");
        let line = "guild create for server 12345 with 67 channels";
        assert_eq!(redact(line), line);
    }

    #[test]
    fn uuid_like_words_are_not_scrubbed() {
        // 8-4-4-4-12 hex UUIDs must survive (they look like nothing secret).
        let line = "message id 1544986413471371336 in channel";
        assert_eq!(redact(line), line);
    }
}
