//! Inline markdown renderer for Discord messages.
//!
//! Supported syntax (matches Discord's own):
//!   **bold**   *italic*   ***bolditalic***   ~~strike~~   __underline__
//!   ||spoiler||   > quote   `code`   ```codeblock```
//!   <@&role>   <@user>   <#channel>   <a?:name:id> custom emoji   :emoji: (unicode)
//!   http(s)://... bare URLs
//!
//! Implementation strategy: tokenise the input into a Vec<Token>. Each
//! token knows its style (bold/italic/...). The egui renderer then walks
//! the token list and lays out each as a RichText in a horizontal flow
//! with text-wrap. (egui wraps automatically via `Label::new` + ui.end_row.)
//!
//! Multi-line messages split at the newline. Block constructs (```code blocks
//! and > quotes) get their own egui containers.

use egui::{Color32, FontId, Label, RichText, Ui};

use crate::colors;
use crate::model::Snowflake;

#[derive(Debug, Clone, PartialEq)]
pub enum Token {
    Text(String),
    Bold(String),
    Italic(String),
    BoldItalic(String),
    Strike(String),
    Underline(String),
    /// Inline code span.
    Code(String),
    /// Block code (```lang\ncode\n```) — gets its own pre-formatted block.
    CodeBlock { lang: Option<String>, code: String },
    /// Spoiler text — hidden until clicked.
    Spoiler(String),
    /// Quote (line-prefixed with `> `).
    Quote(String),
    /// Bare URL → clickable.
    Url { url: String, label: Option<String> },
    /// User mention.
    UserMention { id: Snowflake, label: String },
    /// Role mention.
    RoleMention { id: Snowflake, label: String, color: Color32 },
    /// Channel mention.
    ChannelMention { id: Snowflake, label: String },
    /// Custom emoji (`<:name:id>` static or `<a:name:id>` animated).
    CustomEmoji { animated: bool, name: String, id: Snowflake, url: String },
    /// Unicode emoji (`:emoji:` form, e.g. `:fire:` → `🔥` if we have a lookup).
    UnicodeEmoji { unicode: String, name: String },
    /// Soft line break (treated as <br>).
    LineBreak,
    /// Hard paragraph break (treated as <p>).
    ParagraphBreak,
}

/// Parse Discord markdown into tokens.
pub fn parse(content: &str, lookups: &dyn MentionLookup) -> Vec<Token> {
    let mut out = Vec::new();
    let lines: Vec<&str> = content.split('\n').collect();
    let mut in_block = false;
    let mut block_buf = String::new();
    let mut block_lang: Option<String> = None;
    for (li, line) in lines.iter().enumerate() {
        if in_block {
            if line.trim_end() == "```" {
                // Close the block.
                out.push(Token::CodeBlock {
                    lang: block_lang.take(),
                    code: std::mem::take(&mut block_buf),
                });
                in_block = false;
            } else {
                if !block_buf.is_empty() {
                    block_buf.push('\n');
                }
                block_buf.push_str(line);
            }
            continue;
        }
        // Detect ``` opening.
        if let Some(stripped) = line.trim_start().strip_prefix("```") {
            // Single-line code block: ```lang code``` (close on same line)
            if let Some(end_idx) = stripped.find("```") {
                let before_end = &stripped[..end_idx];
                // Split off the optional language tag (alphanumeric + '+').
                let (lang, code) = match before_end
                    .find(|c: char| !(c.is_alphanumeric() || c == '+' || c == ' '))
                {
                    Some(idx) => {
                        let lang_str = before_end[..idx].trim_end().to_string();
                        let code_str = before_end[idx..].trim_start().to_string();
                        let lang = if lang_str.is_empty() { None } else { Some(lang_str) };
                        (lang, code_str)
                    }
                    None => {
                        let lang_str = before_end.trim().to_string();
                        let lang = if lang_str.is_empty() { None } else { Some(lang_str) };
                        (lang, String::new())
                    }
                };
                out.push(Token::CodeBlock { lang, code });
                in_block = false;
                let _ = li;
                continue;
            }
            // Multi-line code block: ```lang\n ... \n```
            let lang_str = stripped.trim().to_string();
            let lang = if lang_str.is_empty() { None } else { Some(lang_str) };
            in_block = true;
            block_lang = lang;
            continue;
        }
        // Quote: lines starting with `> ` are quoted.
        if line.trim_start().starts_with("> ") || line.trim_start() == ">" {
            let text = line.trim_start().trim_start_matches('>').trim_start();
            out.push(Token::Quote(text.to_string()));
            if li + 1 < lines.len() {
                out.push(Token::ParagraphBreak);
            }
            continue;
        }
        // Inline parse for everything else.
        parse_inline(line, &mut out, lookups);
        if li + 1 < lines.len() {
            out.push(Token::LineBreak);
        }
    }
    if in_block {
        // Reached end of input while still in a code block — flush it raw.
        out.push(Token::CodeBlock { lang: block_lang.take(), code: std::mem::take(&mut block_buf) });
    }
    out
}

pub trait MentionLookup {
    fn user_label(&self, id: Snowflake) -> String;
    fn role_label(&self, id: Snowflake) -> (String, Color32);
    fn channel_label(&self, id: Snowflake) -> String;
}

pub struct NoLookup;
impl MentionLookup for NoLookup {
    fn user_label(&self, id: Snowflake) -> String { format!("@{}", id) }
    fn role_label(&self, id: Snowflake) -> (String, Color32) {
        (format!("@role-{}", id), Color32::from_rgb(0xB5, 0xBA, 0xC1))
    }
    fn channel_label(&self, id: Snowflake) -> String { format!("#{}", id) }
}

fn parse_inline(line: &str, out: &mut Vec<Token>, lookups: &dyn MentionLookup) {
    let chars: Vec<char> = line.chars().collect();
    let mut i = 0;
    let mut text_buf = String::new();
    let flush_text = |buf: &mut String, out: &mut Vec<Token>| {
        if !buf.is_empty() {
            out.push(Token::Text(std::mem::take(buf)));
        }
    };
    while i < chars.len() {
        let c = chars[i];

        // Custom emoji: `<:name:id>` or `<a:name:id>`.
        if c == '<' {
            // Try to match <a?:name:id>
            let rest = &chars[i..];
            let animated = rest.len() > 2 && rest[1] == 'a' && rest[2] == ':';
            let _start = if animated { 3 } else { 1 };
            let (animated, start) = if rest.len() > 1 && rest[1] == ':' { (false, 2) }
                else if animated { (true, 3) } else { (false, usize::MAX) };
            if start != usize::MAX && rest.len() > start {
                if let Some(close_idx) = rest[start..].iter().position(|&c| c == '>') {
                    let inner = &rest[start..start + close_idx];
                    let inner_str: String = inner.iter().collect();
                    if let Some((name, id_str)) = inner_str.rsplit_once(':') {
                        if let Ok(id) = id_str.parse::<u64>() {
                            flush_text(&mut text_buf, out);
                            let ext = if animated { "gif" } else { "webp" };
                            let url = format!(
                                "https://cdn.discordapp.com/emojis/{}.{}?size=48&quality=lossless",
                                id, ext
                            );
                            out.push(Token::CustomEmoji {
                                animated,
                                name: name.to_string(),
                                id: Snowflake::from_u64(id),
                                url,
                            });
                            i += start + close_idx + 1;
                            continue;
                        }
                    }
                }
            }
        }

        // User/role/channel mention: `<@id>`, `<@!id>`, `<@&id>`, `<#id>`.
        if c == '<' && i + 1 < chars.len() {
            let rest = &chars[i..];
            if let Some(close_idx) = rest.iter().skip(1).position(|&c| c == '>') {
                let inner: String = rest[1..1 + close_idx].iter().collect();
                // Strip optional Discord mention prefix:
                //   `@`   → user (bare form `<@id>`)
                //   `@!`  → user (nickname form, treated same as `@`)
                //   `@&`  → role
                //   `#`   → channel
                let (kind, id_str) = if let Some(s) = inner.strip_prefix("@&") { ("role", s) }
                    else if let Some(s) = inner.strip_prefix("@!") { ("user", s) }
                    else if let Some(s) = inner.strip_prefix('@') { ("user", s) }
                    else if let Some(s) = inner.strip_prefix('#') { ("channel", s) }
                    else if let Some(s) = inner.strip_prefix("&") { ("role", s) }
                    else if let Some(s) = inner.strip_prefix("!") { ("user", s) }
                    else { ("user", inner.as_str()) };
                if let Ok(id) = id_str.parse::<u64>() {
                    let id = Snowflake::from_u64(id);
                    flush_text(&mut text_buf, out);
                    match kind {
                        "user" => {
                            let label = lookups.user_label(id);
                            out.push(Token::UserMention { id, label });
                        }
                        "role" => {
                            let (label, color) = lookups.role_label(id);
                            out.push(Token::RoleMention { id, label, color });
                        }
                        "channel" => {
                            let label = lookups.channel_label(id);
                            out.push(Token::ChannelMention { id, label });
                        }
                        _ => unreachable!(),
                    }
                    i += 1 + close_idx + 1;
                    continue;
                }
            }
        }

        // URLs (bare, leading http(s)://).
        if c == 'h' && i + 7 <= chars.len() && (chars[i..].iter().take(7).collect::<String>() == "http://" || chars[i..].iter().take(8).collect::<String>() == "https://") {
            let url_end = chars[i..]
                .iter()
                .position(|&c| c.is_whitespace() || c == '<' || c == '>' || c == '|')
                .unwrap_or(chars.len() - i);
            let url: String = chars[i..i + url_end].iter().collect();
            if let Ok(parsed) = url::Url::parse(&url) {
                if parsed.host_str().is_some() {
                    flush_text(&mut text_buf, out);
                    out.push(Token::Url { url: url.clone(), label: None });
                    i += url_end;
                    continue;
                }
            }
        }

        // Inline code: `code`
        if c == '`' {
            // Find the closing backtick (single, not triple — handled above).
            if let Some(close_idx) = chars[i + 1..].iter().position(|&c| c == '`') {
                let code: String = chars[i + 1..i + 1 + close_idx].iter().collect();
                flush_text(&mut text_buf, out);
                out.push(Token::Code(code));
                i += 1 + close_idx + 1;
                continue;
            }
        }

        // Spoiler: ||text||
        if c == '|' && i + 1 < chars.len() && chars[i + 1] == '|' {
            if let Some(close_idx) = chars[i + 2..].windows(2).position(|w| w[0] == '|' && w[1] == '|') {
                let text: String = chars[i + 2..i + 2 + close_idx].iter().collect();
                flush_text(&mut text_buf, out);
                out.push(Token::Spoiler(text));
                i += 2 + close_idx + 2;
                continue;
            }
        }

        // Bold-italic: ***text***
        if c == '*' && i + 2 < chars.len() && chars[i + 1] == '*' && chars[i + 2] == '*' {
            if let Some(close_idx) = chars[i + 3..].windows(3).position(|w| w[0] == '*' && w[1] == '*' && w[2] == '*') {
                let text: String = chars[i + 3..i + 3 + close_idx].iter().collect();
                flush_text(&mut text_buf, out);
                out.push(Token::BoldItalic(text));
                i += 3 + close_idx + 3;
                continue;
            }
        }

        // Bold: **text**
        if c == '*' && i + 1 < chars.len() && chars[i + 1] == '*' {
            if let Some(close_idx) = chars[i + 2..].windows(2).position(|w| w[0] == '*' && w[1] == '*') {
                let text: String = chars[i + 2..i + 2 + close_idx].iter().collect();
                flush_text(&mut text_buf, out);
                out.push(Token::Bold(text));
                i += 2 + close_idx + 2;
                continue;
            }
        }

        // Italic: *text*  (single asterisk, not **).
        if c == '*' && (i == 0 || chars[i - 1].is_whitespace()) {
            if let Some(close_idx) = chars[i + 1..].iter().position(|&c| c == '*') {
                // Ensure this close_idx is not part of **.
                let text: String = chars[i + 1..i + 1 + close_idx].iter().collect();
                if !text.is_empty() {
                    flush_text(&mut text_buf, out);
                    out.push(Token::Italic(text));
                    i += 1 + close_idx + 1;
                    continue;
                }
            }
        }

        // Strike: ~~text~~
        if c == '~' && i + 1 < chars.len() && chars[i + 1] == '~' {
            if let Some(close_idx) = chars[i + 2..].windows(2).position(|w| w[0] == '~' && w[1] == '~') {
                let text: String = chars[i + 2..i + 2 + close_idx].iter().collect();
                flush_text(&mut text_buf, out);
                out.push(Token::Strike(text));
                i += 2 + close_idx + 2;
                continue;
            }
        }

        // Underline: __text__
        if c == '_' && i + 1 < chars.len() && chars[i + 1] == '_' {
            if let Some(close_idx) = chars[i + 2..].windows(2).position(|w| w[0] == '_' && w[1] == '_') {
                let text: String = chars[i + 2..i + 2 + close_idx].iter().collect();
                flush_text(&mut text_buf, out);
                out.push(Token::Underline(text));
                i += 2 + close_idx + 2;
                continue;
            }
        }

        // Accumulator.
        text_buf.push(c);
        i += 1;
    }
    flush_text(&mut text_buf, out);
}

/// Render a list of parsed tokens into a vertical `egui::Ui` block.
/// `font_size` is the body text size; `spoiler_base` disambiguates spoiler
/// indexes between messages (use the message id as the base).
pub fn render_tokens(
    ui: &mut Ui,
    tokens: &[Token],
    revealed: &mut std::collections::HashSet<usize>,
    font_size: f32,
    spoiler_base: usize,
) {
    let mut spoiler_idx: usize = 0;
    let mut current_line: Vec<&Token> = Vec::new();
    let mut flush_line = |line: &mut Vec<&Token>, ui: &mut Ui, spoiler_idx: &mut usize, font_size: f32, spoiler_base: usize| {
        if line.is_empty() {
            return;
        }
        ui.horizontal_wrapped(|ui| {
            for tok in line.drain(..) {
                render_one_token(ui, tok, spoiler_idx, revealed, font_size, spoiler_base);
            }
        });
    };

    for tok in tokens {
        match tok {
            Token::LineBreak => {
                flush_line(&mut current_line, ui, &mut spoiler_idx, font_size, spoiler_base);
                ui.end_row();
            }
            Token::ParagraphBreak => {
                flush_line(&mut current_line, ui, &mut spoiler_idx, font_size, spoiler_base);
                ui.end_row();
                ui.add_space(2.0);
            }
            Token::CodeBlock { lang, code } => {
                flush_line(&mut current_line, ui, &mut spoiler_idx, font_size, spoiler_base);
                ui.end_row();
                let _ = lang;
                let block = egui::Frame::new()
                    .fill(colors::CODE_BG)
                    .inner_margin(egui::Margin::symmetric(8, 4))
                    .corner_radius(4.0);
                block.show(ui, |ui| {
                    ui.label(
                        RichText::new(code)
                            .color(colors::TEXT_PRIMARY)
                            .family(egui::FontFamily::Monospace)
                            .size(13.0),
                    );
                });
                ui.end_row();
            }
            Token::Quote(text) => {
                flush_line(&mut current_line, ui, &mut spoiler_idx, font_size, spoiler_base);
                ui.end_row();
                let quote = egui::Frame::new()
                    .fill(Color32::TRANSPARENT)
                    .stroke(egui::Stroke::new(4.0, colors::TEXT_TERTIARY))
                    .inner_margin(egui::Margin { left: 12, right: 8, top: 2, bottom: 2 })
                    .corner_radius(2.0);
                quote.show(ui, |ui| {
                    ui.label(
                        RichText::new(text)
                            .color(colors::TEXT_SECONDARY)
                            .size(font_size),
                    );
                });
                ui.end_row();
            }
            other => {
                current_line.push(other);
            }
        }
    }
    flush_line(&mut current_line, ui, &mut spoiler_idx, font_size, spoiler_base);
}

fn render_one_token(
    ui: &mut Ui,
    token: &Token,
    spoiler_idx: &mut usize,
    revealed: &mut std::collections::HashSet<usize>,
    font_size: f32,
    spoiler_base: usize,
) {
    let body = colors::TEXT_PRIMARY;
    let seg = crate::ui::emoji::render_with_emojis;
    match token {
        Token::Text(s) => {
            seg(ui, s, &|rt| rt, font_size, body);
        }
        Token::Bold(s) => {
            seg(ui, s, &|rt| rt.strong(), font_size, body);
        }
        Token::Italic(s) => {
            seg(ui, s, &|rt| rt.italics(), font_size, body);
        }
        Token::BoldItalic(s) => {
            seg(ui, s, &|rt| rt.strong().italics(), font_size, body);
        }
        Token::Strike(s) => {
            seg(ui, s, &|rt| rt.strikethrough(), font_size, body);
        }
        Token::Underline(s) => {
            seg(ui, s, &|rt| rt.underline(), font_size, body);
        }
        Token::Code(s) => {
            let frame = egui::Frame::new()
                .fill(colors::CODE_BG)
                .inner_margin(egui::Margin::symmetric(4, 1))
                .corner_radius(3.0);
            frame.show(ui, |ui| {
                ui.label(
                    RichText::new(s)
                        .color(colors::TEXT_PRIMARY)
                        .family(egui::FontFamily::Monospace)
                        .size(13.0),
                );
            });
        }
        Token::Spoiler(s) => {
            let idx = spoiler_base + *spoiler_idx;
            *spoiler_idx += 1;
            let is_revealed = revealed.contains(&idx);
            let frame = egui::Frame::new()
                .fill(if is_revealed { colors::SPOILER_REVEALED } else { colors::SPOILER_HIDDEN })
                .inner_margin(egui::Margin::symmetric(4, 1))
                .corner_radius(3.0);
            let resp = frame.show(ui, |ui| {
                if is_revealed {
                    seg(ui, s, &|rt| rt, font_size, colors::TEXT_PRIMARY);
                } else {
                    seg(ui, s, &|rt| rt, font_size, colors::SPOILER_HIDDEN);
                }
            });
            if resp.response.interact(egui::Sense::click()).clicked() {
                if is_revealed {
                    revealed.remove(&idx);
                } else {
                    revealed.insert(idx);
                }
            }
        }
        Token::Url { url, label } => {
            let display = label.clone().unwrap_or_else(|| url.clone());
            let resp = ui.add(
                Label::new(RichText::new(display).color(colors::TEXT_LINK).underline().size(font_size))
                    .sense(egui::Sense::click()),
            );
            if resp.clicked() {
                let _ = open::that(url);
            }
        }
        Token::UserMention { id: _, label } => {
            let frame = egui::Frame::new()
                .fill(colors::MENTION_BG)
                .inner_margin(egui::Margin::symmetric(4, 1))
                .corner_radius(3.0);
            frame.show(ui, |ui| {
                seg(ui, label, &|rt| rt, font_size, colors::MENTION_FG);
            });
        }
        Token::RoleMention { id: _, label, color } => {
            let bg = color.gamma_multiply(0.20);
            let frame = egui::Frame::new()
                .fill(bg)
                .inner_margin(egui::Margin::symmetric(4, 1))
                .corner_radius(3.0);
            frame.show(ui, |ui| {
                seg(ui, label, &|rt| rt, font_size, *color);
            });
        }
        Token::ChannelMention { id: _, label } => {
            let frame = egui::Frame::new()
                .fill(colors::MENTION_BG)
                .inner_margin(egui::Margin::symmetric(4, 1))
                .corner_radius(3.0);
            frame.show(ui, |ui| {
                seg(ui, label, &|rt| rt, font_size, colors::MENTION_FG);
            });
        }
        Token::CustomEmoji { url, name, .. } => {
            crate::image_loader::render_emoji(ui, url, (font_size * 1.5).round().max(18.0), name);
        }
        Token::UnicodeEmoji { unicode, .. } => {
            crate::ui::emoji::render_emoji_image(ui, unicode, (font_size * 1.4).round().max(17.0));
        }
        // Block tokens are rendered by the caller.
        Token::CodeBlock { .. } | Token::Quote(_) | Token::LineBreak | Token::ParagraphBreak => {}
    }
    let _ = FontId::default();
}

/// Convenience: parse + render in one shot.
pub fn render_message_content(
    ui: &mut Ui,
    content: &str,
    lookups: &dyn MentionLookup,
    font_size: f32,
    spoiler_base: usize,
    revealed: &mut std::collections::HashSet<usize>,
) {
    let tokens = parse(content, lookups);
    render_tokens(ui, &tokens, revealed, font_size, spoiler_base);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn first(tokens: &[Token]) -> Option<&Token> {
        tokens.first()
    }

    #[test]
    fn empty_input_yields_no_tokens() {
        let t = parse("", &NoLookup);
        assert!(t.is_empty(), "empty input should produce no tokens, got {t:?}");
    }

    #[test]
    fn plain_text_emits_single_text() {
        let t = parse("hello world", &NoLookup);
        assert_eq!(t.len(), 1);
        assert!(matches!(&t[0], Token::Text(s) if s == "hello world"));
    }

    #[test]
    fn bold_emits_bold_token() {
        let t = parse("**bold**", &NoLookup);
        assert_eq!(t.len(), 1);
        match &t[0] {
            Token::Bold(s) => assert_eq!(s, "bold"),
            other => panic!("expected Bold, got {other:?}"),
        }
    }

    #[test]
    fn italic_emits_italic_token() {
        let t = parse(" *italic* ", &NoLookup);
        let italic = t.iter().find(|tok| matches!(tok, Token::Italic(_)));
        match italic {
            Some(Token::Italic(s)) => assert_eq!(s, "italic"),
            other => panic!("expected Italic, got {other:?}"),
        }
    }

    #[test]
    fn inline_code_emits_code_token() {
        let t = parse("see `let x = 5;` here", &NoLookup);
        let code = t.iter().find(|tok| matches!(tok, Token::Code(_)));
        match code {
            Some(Token::Code(s)) => assert_eq!(s, "let x = 5;"),
            other => panic!("expected Code, got {other:?}"),
        }
    }

    #[test]
    fn multiline_code_block_with_lang() {
        let input = "```rust\nfn main() {}\n```";
        let t = parse(input, &NoLookup);
        assert_eq!(t.len(), 1, "expected single CodeBlock token, got {t:?}");
        match &t[0] {
            Token::CodeBlock { lang, code } => {
                assert_eq!(lang.as_deref(), Some("rust"));
                assert_eq!(code, "fn main() {}");
            }
            other => panic!("expected CodeBlock, got {other:?}"),
        }
    }

    #[test]
    fn multiline_code_block_without_lang() {
        let input = "```\nplain\ncode\n```";
        let t = parse(input, &NoLookup);
        assert_eq!(t.len(), 1, "got {t:?}");
        match &t[0] {
            Token::CodeBlock { lang, code } => {
                assert!(lang.is_none());
                assert_eq!(code, "plain\ncode");
            }
            other => panic!("expected CodeBlock, got {other:?}"),
        }
    }

    #[test]
    fn unterminated_code_block_flushes_raw() {
        let input = "```rust\nfn main() {";
        let t = parse(input, &NoLookup);
        assert_eq!(t.len(), 1, "got {t:?}");
        match &t[0] {
            Token::CodeBlock { lang, code } => {
                assert_eq!(lang.as_deref(), Some("rust"));
                assert_eq!(code, "fn main() {");
            }
            other => panic!("expected CodeBlock, got {other:?}"),
        }
    }

    #[test]
    fn user_mention_emits_user_mention() {
        let t = parse("hello <@12345>", &NoLookup);
        let mention = t.iter().find(|tok| matches!(tok, Token::UserMention { .. }));
        match mention {
            Some(Token::UserMention { id, label }) => {
                assert_eq!(u64::from(*id), 12345);
                assert!(label.starts_with('@'));
            }
            other => panic!("expected UserMention, got {other:?}"),
        }
    }

    #[test]
    fn channel_mention_emits_channel_mention() {
        let t = parse("see <#67890>", &NoLookup);
        let m = t.iter().find(|tok| matches!(tok, Token::ChannelMention { .. }));
        match m {
            Some(Token::ChannelMention { id, .. }) => assert_eq!(u64::from(*id), 67890),
            other => panic!("expected ChannelMention, got {other:?}"),
        }
    }

    #[test]
    fn url_emits_url_token() {
        let t = parse("https://example.com", &NoLookup);
        match first(&t) {
            Some(Token::Url { url, .. }) => assert_eq!(url, "https://example.com"),
            other => panic!("expected Url, got {other:?}"),
        }
    }

    #[test]
    fn spoiler_token() {
        let t = parse("||secret||", &NoLookup);
        match first(&t) {
            Some(Token::Spoiler(s)) => assert_eq!(s, "secret"),
            other => panic!("expected Spoiler, got {other:?}"),
        }
    }

    #[test]
    fn strike_token() {
        let t = parse("~~deleted~~", &NoLookup);
        match first(&t) {
            Some(Token::Strike(s)) => assert_eq!(s, "deleted"),
            other => panic!("expected Strike, got {other:?}"),
        }
    }

    #[test]
    fn quote_emits_quote_token() {
        let t = parse("> quoted text", &NoLookup);
        match first(&t) {
            Some(Token::Quote(s)) => assert_eq!(s, "quoted text"),
            other => panic!("expected Quote, got {other:?}"),
        }
    }

    #[test]
    fn multiline_text_emits_linebreaks() {
        let t = parse("line1\nline2\nline3", &NoLookup);
        // Three text tokens + two line breaks between them.
        let texts: Vec<&Token> = t.iter().filter(|tok| matches!(tok, Token::Text(_))).collect();
        let breaks: Vec<&Token> = t.iter().filter(|tok| matches!(tok, Token::LineBreak)).collect();
        assert_eq!(texts.len(), 3, "got {t:?}");
        assert_eq!(breaks.len(), 2, "got {t:?}");
    }

    #[test]
    fn custom_emoji_animated() {
        let t = parse("<a:dance:7777>", &NoLookup);
        match first(&t) {
            Some(Token::CustomEmoji { animated, name, id, .. }) => {
                assert!(*animated);
                assert_eq!(name, "dance");
                assert_eq!(u64::from(*id), 7777);
            }
            other => panic!("expected CustomEmoji, got {other:?}"),
        }
    }

    #[test]
    fn custom_emoji_static() {
        let t = parse("<:smile:1234>", &NoLookup);
        match first(&t) {
            Some(Token::CustomEmoji { animated, name, id, .. }) => {
                assert!(!*animated);
                assert_eq!(name, "smile");
                assert_eq!(u64::from(*id), 1234);
            }
            other => panic!("expected CustomEmoji, got {other:?}"),
        }
    }

    #[test]
    fn mixed_inline_text_and_bold() {
        let t = parse("hello **world** end", &NoLookup);
        let mut it = t.iter();
        match it.next() {
            Some(Token::Text(s)) => assert_eq!(s, "hello "),
            other => panic!("expected Text, got {other:?}"),
        }
        match it.next() {
            Some(Token::Bold(s)) => assert_eq!(s, "world"),
            other => panic!("expected Bold, got {other:?}"),
        }
        match it.next() {
            Some(Token::Text(s)) => assert_eq!(s, " end"),
            other => panic!("expected Text, got {other:?}"),
        }
    }
}
