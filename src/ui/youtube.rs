//! YouTube link rendering (point 7).
//!
//! Bare YouTube URLs in messages render as a full video embed: source +
//! author line, clickable title, big thumbnail with a play button. Video
//! metadata comes from YouTube's public oEmbed endpoint (no API key),
//! cached per URL in the app state.

use egui::{Rect, Sense, Ui};

use crate::colors;
use crate::image_loader;
use crate::state::AppState;

/// Extract the first YouTube video link from a message. Returns
/// (video_id, normalized watch URL).
pub fn find_link(content: &str) -> Option<(String, String)> {
    for word in content.split_whitespace() {
        let cleaned = word.trim_matches(|c: char| {
            !c.is_ascii_alphanumeric()
                && c != ':'
                && c != '/'
                && c != '.'
                && c != '='
                && c != '?'
                && c != '-'
                && c != '_'
        });
        if let Some(id) = extract_video_id(cleaned) {
            let url = format!("https://www.youtube.com/watch?v={id}");
            return Some((id, url));
        }
    }
    None
}

fn extract_video_id(url: &str) -> Option<String> {
    let low = url.to_ascii_lowercase();
    let is_yt = low.contains("youtube.com/")
        || low.contains("youtu.be/")
        || low.contains("youtube-nocookie.com/");
    if !is_yt {
        return None;
    }
    // youtu.be/<id>
    if let Some(rest) = url.split_once("youtu.be/").map(|(_, r)| r) {
        let id: String = rest.chars().take(11).collect();
        if is_video_id(&id) {
            return Some(id);
        }
    }
    // youtube.com/watch?v=<id>
    if let Some(q) = url.split_once('?').map(|(_, q)| q) {
        for kv in q.split('&') {
            if let Some(v) = kv.strip_prefix("v=") {
                let id: String = v.chars().take(11).collect();
                if is_video_id(&id) {
                    return Some(id);
                }
            }
        }
    }
    // youtube.com/shorts/<id>, /embed/<id>, /live/<id>
    for seg in ["shorts/", "embed/", "live/"] {
        if let Some(idx) = low.find(seg) {
            let rest = &url[idx + seg.len()..];
            let id: String = rest.chars().take(11).collect();
            if is_video_id(&id) {
                return Some(id);
            }
        }
    }
    None
}

fn is_video_id(id: &str) -> bool {
    id.len() == 11
        && id.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

/// Percent-encode just the URL-unsafe chars a YouTube URL can contain.
fn simple_url_encode(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 8);
    for b in s.bytes() {
        match b {
            b':' | b'/' | b'.' | b'-' | b'_' | b'~' => out.push(b as char),
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

/// Kick an oEmbed fetch if the URL is not cached yet.
pub fn ensure_oembed(app_state: &AppState, url: &str, video_id: &str) {
    if app_state.oembed(url).is_some() {
        return;
    }
    let key = url.to_string();
    // Placeholder marker so we don't re-fetch every frame.
    if app_state.oembed(url).is_none() {
        app_state.set_oembed(
            url,
            crate::model::OEmbedInfo {
                title: None,
                author_name: None,
                thumbnail_url: None,
            },
        );
    }
    let key2 = key.clone();
    let video_id_owned = video_id.to_string();
    tokio::spawn(async move {
        let client = crate::rest::plain_client();
        let endpoint = format!(
            "https://www.youtube.com/oembed?format=json&url={}",
            simple_url_encode(&key2)
        );
        let fetched = async {
            let resp = client
                .get(&endpoint)
                .timeout(std::time::Duration::from_secs(10))
                .send()
                .await?;
            let resp = resp.error_for_status()?;
            resp.json::<crate::model::OEmbedInfo>().await
        };
        match fetched.await {
            Ok(info) => {
                if let Some(s) = crate::state::global() {
                    let mut full = info;
                    if full.thumbnail_url.is_none() {
                        full.thumbnail_url = Some(format!(
                            "https://i.ytimg.com/vi/{video_id_owned}/hqdefault.jpg"
                        ));
                    }
                    s.set_oembed(&key2, full);
                    let _ = s
                        .event_sender()
                        .send(crate::gateway::events::Event::RepaintRequested);
                }
            }
            Err(e) => {
                tracing::debug!(error = %e, "youtube oembed fetch failed");
                if let Some(s) = crate::state::global() {
                    // Fill with a bare thumbnail so the card still renders.
                    s.set_oembed(
                        &key2,
                        crate::model::OEmbedInfo {
                            title: None,
                            author_name: None,
                            thumbnail_url: Some(format!(
                                "https://i.ytimg.com/vi/{video_id_owned}/hqdefault.jpg"
                            )),
                        },
                    );
                }
            }
        }
    });
}

/// Draw the full video embed card (source + author line, clickable title,
/// big thumbnail with a play button). Skips itself while metadata +
/// thumbnail are still loading, so the row grows once when both are ready.
///
/// Layout contract: the card reserves its whole rect with ONE
/// [`Ui::allocate_exact_size`] and paints everything through
/// `ui.painter_at(rect)` - the exact pattern the GIF attachments use.
/// A nested `Frame::show` must NOT be used here: egui gives ScrollArea
/// content a viewport-bounded `max_rect`, and rows near the window edge
/// hand `Frame::show` a zero-height region, which collapses the whole
/// card (painted at height 0, no space reserved) even though every early
/// gate passed. `allocate_exact_size` always reserves the exact size, so
/// the row grows and the next messages are pushed down correctly.
pub fn render(ui: &mut Ui, app_state: &AppState, url: &str, video_id: &str) {
    ensure_oembed(app_state, url, video_id);
    let Some(info) = app_state.oembed(url) else {
        return;
    };
    let Some(thumb) = info.thumbnail_url.clone() else {
        return;
    };
    if image_loader::global_cache()
        .get_or_fetch(ui.ctx(), &thumb, 800, 450, image_loader::Shape::Rounded(6))
        .is_none()
    {
        return; // grow the row only when the thumbnail is actually ready
    }

    // ── Card geometry (computed BEFORE allocating, so the reserved rect
    //    is final and nothing can shrink it later in the frame) ──
    let card_w = 440.0_f32.min(ui.available_width().max(280.0) - 16.0).max(240.0);
    let pad_l = 16.0;
    let pad_r = 12.0;
    let pad_t = 8.0;
    let pad_b = 8.0;
    let inner_w = (card_w - pad_l - pad_r).max(180.0);
    let thumb_h = (inner_w * 9.0 / 16.0).min(248.0);
    let card_h = pad_t + 17.0 /*source line*/ + 21.0 /*title*/ + 4.0 + thumb_h + pad_b;

    let (rect, resp) = ui.allocate_exact_size(egui::vec2(card_w, card_h), Sense::click());
    let p = ui.painter_at(rect);

    // Card background + red left stripe (YouTube identity).
    p.rect_filled(rect, 4.0, colors::EMBED_BG);
    let stripe = Rect::from_min_max(
        egui::pos2(rect.min.x, rect.min.y),
        egui::pos2(rect.min.x + 4.0, rect.max.y),
    );
    p.rect_filled(stripe, 4.0, colors::RED);

    // Source line: "YouTube" + author.
    let left = rect.min.x + pad_l;
    let source_w = p
        .layout("YouTube".to_string(), egui::FontId::proportional(12.0), colors::RED, 200.0)
        .size()
        .x;
    let galley = p.layout("YouTube".to_string(), egui::FontId::proportional(12.0), colors::RED, 200.0);
    p.galley(egui::pos2(left, rect.min.y + pad_t), galley, colors::RED);
    let x = left + source_w + 6.0;
    if let Some(author) = info.author_name.clone().filter(|a| !a.is_empty()) {
        let g = p.layout(author, egui::FontId::proportional(12.0), colors::TEXT_TERTIARY, 200.0);
        p.galley(egui::pos2(x, rect.min.y + pad_t), g, colors::TEXT_TERTIARY);
    }

    // Title: link-styled, one line, wrapped/ellipsized to the card width.
    let title = info.title.clone().unwrap_or_else(|| "Watch on YouTube".to_string());
    let galley = p.layout(title, egui::FontId::proportional(14.5), colors::TEXT_LINK, inner_w);
    p.galley(egui::pos2(left, rect.min.y + pad_t + 18.0), galley, colors::TEXT_LINK);

    // Thumbnail, painted into the reserved sub-rect.
    let thumb_rect = Rect::from_min_size(
        egui::pos2(left, rect.min.y + pad_t + 42.0),
        egui::vec2(inner_w, thumb_h),
    );
    if let Some(handle) = image_loader::global_cache().get_or_fetch(
        ui.ctx(),
        &thumb,
        800,
        450,
        image_loader::Shape::Rounded(6),
    ) {
        let uv = Rect::from_min_max(egui::Pos2::new(0.0, 0.0), egui::Pos2::new(1.0, 1.0));
        p.image(handle.id(), thumb_rect, uv, egui::Color32::WHITE);
    } else {
        p.rect_filled(thumb_rect, 6.0, egui::Color32::from_rgb(0x38, 0x3A, 0x40));
    }

    // Play button: dark halo disc + white disc + blurple triangle.
    let center = thumb_rect.center();
    let disc_r = (thumb_h * 0.16).clamp(20.0, 34.0);
    p.circle_filled(center, disc_r, egui::Color32::from_black_alpha(140));
    p.circle_filled(center, disc_r - 2.0, colors::TEXT_PRIMARY);
    let tri = disc_r * 0.55;
    let p1 = center + egui::vec2(-tri * 0.55, -tri);
    let p2 = center + egui::vec2(-tri * 0.55, tri);
    let p3 = center + egui::vec2(tri * 0.85, 0.0);
    p.add(egui::Shape::convex_polygon(
        vec![p1, p2, p3],
        colors::BLURPLE,
        egui::Stroke::NONE,
    ));

    // The whole card is clickable (title + thumbnail both open the video).
    if resp.clicked() {
        let _ = open::that_detached(url);
    }
    ui.add_space(4.0);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn watch_url_id() {
        let (id, url) = find_link("check this https://www.youtube.com/watch?v=dQw4w9WgXcQ now").unwrap();
        assert_eq!(id, "dQw4w9WgXcQ");
        assert_eq!(url, "https://www.youtube.com/watch?v=dQw4w9WgXcQ");
    }

    #[test]
    fn short_url_id() {
        let (id, _) = find_link("https://youtu.be/abcdefghijk?si=xyz").unwrap();
        assert_eq!(id, "abcdefghijk");
    }

    #[test]
    fn shorts_and_embed() {
        assert_eq!(
            extract_video_id("https://www.youtube.com/shorts/1234567890_"),
            Some("1234567890_".to_string())
        );
        assert_eq!(
            extract_video_id("https://www.youtube.com/embed/dQw4w9WgXcQ"),
            Some("dQw4w9WgXcQ".to_string())
        );
    }

    #[test]
    fn non_youtube_ignored() {
        assert_eq!(find_link("see https://vimeo.com/12345"), None);
        assert_eq!(find_link("plain text"), None);
    }
}
