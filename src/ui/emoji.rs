//! Color emoji rendering via Twemoji (jdecked/twemoji, CC-BY 4.0).
//!
//! egui's bundled fonts have no color emoji glyphs, so we segment text into
//! runs of plain text and emoji clusters, render text runs as normal
//! RichText, and render emoji clusters as images fetched once from the
//! Twemoji CDN (72x72 PNG, cached in the process-wide image cache).
//!
//! The segmentation handles:
//!   - single-codepoint emoji (fire, rocket, ...)
//!   - variation-selector sequences (VS16 / FE0F)
//!   - ZWJ sequences (family, profession, ... emoji)
//!   - skin-tone modifiers (1F3FB..1F3FF)
//!   - keycap sequences (digit + 20E3)
//!   - regional-indicator flags (pairs of 1F1E6..1F1FF)

use egui::{Color32, Ui};

use crate::image_loader;

/// CDN root for Twemoji PNGs (72x72 is crisp up to ~36 logical px).
const TWEMOJI_CDN: &str = "https://cdn.jsdelivr.net/gh/jdecked/twemoji@15.1.0/assets/72x72";

/// A segmented piece of a string.
#[derive(Debug, Clone, PartialEq)]
pub enum Seg {
    Text(String),
    Emoji(String),
}

/// True if the codepoint is an emoji base character (may still need VS16 to
/// be presented as emoji, but Twemoji has files for these).
fn is_emoji_base(c: char) -> bool {
    let u = c as u32;
    matches!(u,
        0x1F000..=0x1FAFF            // all SMP emoji planes (pictographs, emoticons, symbols, supplemental, extended)
        | 0x2600..=0x27BF            // misc symbols + dingbats
        | 0x2B00..=0x2BFF            // stars, arrows (2B50)
        | 0x2300..=0x23FF            // misc technical (watch, hourglass, media controls)
        | 0x25A0..=0x25FF            // geometric shapes (squares used as emoji)
        | 0x2190..=0x21FF            // arrows
        | 0x2934..=0x2935             // curved arrows
    )
}

fn is_regional_indicator(c: char) -> bool {
    (0x1F1E6..=0x1F1FF).contains(&(c as u32))
}

fn is_modifier(c: char) -> bool {
    let u = c as u32;
    u == 0xFE0F // variation selector-16
        || u == 0x20E3 // combining enclosing keycap
        || (0x1F3FB..=0x1F3FF).contains(&u) // skin tones
}

fn is_zwj(c: char) -> bool {
    c as u32 == 0x200D
}

/// Keycap sequences start with an ASCII digit, # or *.
fn is_keycap_base(c: char) -> bool {
    c.is_ascii_digit() || c == '#' || c == '*'
}

/// Segment a string into text runs and emoji clusters.
pub fn segment(s: &str) -> Vec<Seg> {
    let mut out: Vec<Seg> = Vec::new();
    let chars: Vec<char> = s.chars().collect();
    let mut i = 0usize;
    let mut text = String::new();

    while i < chars.len() {
        let c = chars[i];

        // Keycap: digit/#/* followed by FE0F? then 20E3.
        if is_keycap_base(c)
            && i + 2 < chars.len()
            && (chars[i + 1] as u32) == 0xFE0F
            && (chars[i + 2] as u32) == 0x20E3
        {
            let seq: String = chars[i..=i + 2].iter().collect();
            push_text(&mut out, &mut text);
            out.push(Seg::Emoji(seq));
            i += 3;
            continue;
        }
        if is_keycap_base(c) && i + 1 < chars.len() && (chars[i + 1] as u32) == 0x20E3 {
            let seq: String = chars[i..=i + 1].iter().collect();
            push_text(&mut out, &mut text);
            out.push(Seg::Emoji(seq));
            i += 2;
            continue;
        }

        // Regional indicator pair = flag.
        if is_regional_indicator(c) && i + 1 < chars.len() && is_regional_indicator(chars[i + 1]) {
            let seq: String = chars[i..=i + 1].iter().collect();
            push_text(&mut out, &mut text);
            out.push(Seg::Emoji(seq));
            i += 2;
            continue;
        }

        if is_emoji_base(c) {
            // Consume the full cluster: modifiers, then ZWJ + more base+mods.
            let start = i;
            let mut j = i + 1;
            loop {
                let mut consumed = false;
                while j < chars.len() && is_modifier(chars[j]) {
                    j += 1;
                    consumed = true;
                }
                if j < chars.len() && is_zwj(chars[j]) && j + 1 < chars.len() && (is_emoji_base(chars[j + 1]) || is_regional_indicator(chars[j + 1])) {
                    j += 2;
                    while j < chars.len() && is_modifier(chars[j]) {
                        j += 1;
                    }
                    consumed = true;
                }
                if !consumed {
                    break;
                }
            }
            let seq: String = chars[start..j].iter().collect();
            push_text(&mut out, &mut text);
            out.push(Seg::Emoji(seq));
            i = j;
            continue;
        }

        text.push(c);
        i += 1;
    }
    push_text(&mut out, &mut text);
    out
}

fn push_text(out: &mut Vec<Seg>, text: &mut String) {
    if !text.is_empty() {
        out.push(Seg::Text(std::mem::take(text)));
    }
}

/// Build the Twemoji CDN URL for an emoji cluster. Codepoints are joined
/// with '-' in lowercase hex, with FE0F stripped (Twemoji's file naming).
pub fn twemoji_url(emoji: &str) -> String {
    let codes: Vec<String> = emoji
        .chars()
        .filter(|c| *c as u32 != 0xFE0F)
        .map(|c| format!("{:x}", c as u32))
        .collect();
    format!("{TWEMOJI_CDN}/{}.png", codes.join("-"))
}

/// Fallback URL variant that keeps FE0F (a few Twemoji files use it).
pub fn twemoji_url_vs16(emoji: &str) -> String {
    let codes: Vec<String> = emoji.chars().map(|c| format!("{:x}", c as u32)).collect();
    format!("{TWEMOJI_CDN}/{}.png", codes.join("-"))
}

/// Render a label with inline color emoji. Text gets `font_size`/`color`;
/// emoji render at `font_size * 1.18` (slightly larger, like Discord).
/// Works inside horizontal layouts; wraps like a normal label.
pub fn render_label(ui: &mut Ui, text: &str, font_size: f32, color: Color32) {
    render_with_emojis(ui, text, &|rt| rt, font_size, color);
}

/// Like [`render_label`] but with a style closure (bold, italic, ...) that
/// gets applied to the plain-text segments.
pub fn render_with_emojis(
    ui: &mut Ui,
    text: &str,
    style: &dyn Fn(egui::RichText) -> egui::RichText,
    font_size: f32,
    color: Color32,
) {
    let segs = segment(text);
    if segs.len() == 1 && matches!(segs[0], Seg::Text(_)) {
        // Fast path: pure text.
        if let Seg::Text(t) = &segs[0] {
            ui.label(style(egui::RichText::new(t).size(font_size).color(color)));
        }
        return;
    }
    let emoji_px = (font_size * 1.18).round().max(14.0);
    for seg in segs {
        match seg {
            Seg::Text(t) => {
                ui.label(style(egui::RichText::new(t).size(font_size).color(color)));
            }
            Seg::Emoji(e) => {
                let url = twemoji_url(&e);
                image_loader::render_emoji_inline(ui, &url, &twemoji_url_vs16(&e), emoji_px, &e);
            }
        }
    }
}

/// Render an emoji cluster as an image at `size` px (no text fallback mix).
pub fn render_emoji_image(ui: &mut Ui, cluster: &str, size: f32) {
    let url = twemoji_url(cluster);
    image_loader::render_emoji_inline(ui, &url, &twemoji_url_vs16(cluster), size, cluster);
}

/// Measure a text+emoji label (approximate, for fixed layouts).
pub fn label_width(ui: &Ui, text: &str, font_size: f32) -> f32 {
    let mut w = 0.0;
    for seg in segment(text) {
        match seg {
            Seg::Text(t) => {
                let galley = ui.painter().layout(
                    t,
                    egui::FontId::proportional(font_size),
                    Color32::WHITE,
                    f32::INFINITY,
                );
                w += galley.size().x;
            }
            Seg::Emoji(_) => w += font_size * 1.18 + 2.0,
        }
    }
    w
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plain_text_is_single_segment() {
        let segs = segment("general");
        assert_eq!(segs, vec![Seg::Text("general".into())]);
    }

    #[test]
    fn single_emoji_detected() {
        let segs = segment("general💬");
        assert_eq!(segs, vec![Seg::Text("general".into()), Seg::Emoji("💬".into())]);
    }

    #[test]
    fn emoji_with_vs16() {
        let segs = segment("dev-log🛠️");
        assert_eq!(
            segs,
            vec![Seg::Text("dev-log".into()), Seg::Emoji("🛠️".into())]
        );
    }

    #[test]
    fn multiple_emoji() {
        let segs = segment("🔥 🚀 🧊");
        assert_eq!(
            segs,
            vec![
                Seg::Emoji("🔥".into()),
                Seg::Text(" ".into()),
                Seg::Emoji("🚀".into()),
                Seg::Text(" ".into()),
                Seg::Emoji("🧊".into()),
            ]
        );
    }

    #[test]
    fn keycap_sequence() {
        let segs = segment("rank 1️⃣ up");
        assert_eq!(
            segs,
            vec![
                Seg::Text("rank ".into()),
                Seg::Emoji("1️⃣".into()),
                Seg::Text(" up".into()),
            ]
        );
    }

    #[test]
    fn flag_sequence() {
        let segs = segment("flag 🇨🇴 here");
        assert_eq!(
            segs,
            vec![
                Seg::Text("flag ".into()),
                Seg::Emoji("🇨🇴".into()),
                Seg::Text(" here".into()),
            ]
        );
    }

    #[test]
    fn zwj_sequence_stays_one_emoji() {
        // Fire + ZWJ + fire = one cluster.
        let s = "🔥‍🔥";
        let segs = segment(s);
        assert_eq!(segs, vec![Seg::Emoji(s.into())]);
    }

    #[test]
    fn twemoji_url_strips_vs16() {
        assert_eq!(twemoji_url("🛠️"), format!("{TWEMOJI_CDN}/1f6e0.png"));
        assert_eq!(twemoji_url("🔥"), format!("{TWEMOJI_CDN}/1f525.png"));
        assert_eq!(twemoji_url("🇨🇴"), format!("{TWEMOJI_CDN}/1f1e8-1f1f4.png"));
    }

    #[test]
    fn ascii_stays_text() {
        let segs = segment("hello #world 123 *star");
        assert_eq!(segs, vec![Seg::Text("hello #world 123 *star".into())]);
    }

    #[test]
    fn mixed_channel_names() {
        assert_eq!(segment("memes😂"), vec![Seg::Text("memes".into()), Seg::Emoji("😂".into())]);
        assert_eq!(segment("🎧"), vec![Seg::Emoji("🎧".into())]);
        assert_eq!(
            segment("showcase✨"),
            vec![Seg::Text("showcase".into()), Seg::Emoji("✨".into())]
        );
    }
}
