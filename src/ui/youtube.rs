//! YouTube link rendering (point 7).
//!
//! Bare YouTube URLs in messages render as a full video embed: source +
//! author line, clickable title, big thumbnail with a play button. Video
//! metadata comes from YouTube's public oEmbed endpoint (no API key),
//! cached per URL in the app state.

use egui::{Rect, Sense, Ui, Vec2};

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

/// Draw the full video embed card (title clickable, thumbnail with play
/// button). Skips itself while metadata + thumbnail are still loading
/// (the row grows once, when both are ready).
pub fn render(ui: &mut Ui, app_state: &AppState, url: &str, video_id: &str) {
    ensure_oembed(app_state, url, video_id);
    let Some(info) = app_state.oembed(url) else {
        return;
    };
    let Some(thumb) = info.thumbnail_url.clone() else {
        return;
    };
    let cached = image_loader::global_cache()
        .get_or_fetch(ui.ctx(), &thumb, 800, 450, image_loader::Shape::Rounded(6))
        .is_some();
    if !cached {
        return; // grow the row only when the thumbnail is actually ready
    }

    let card_w = 440.0_f32.min(ui.available_width() - 16.0).max(240.0);
    let frame = egui::Frame::new()
        .fill(colors::EMBED_BG)
        .corner_radius(4.0)
        .inner_margin(egui::Margin { left: 16, right: 12, top: 8, bottom: 8 });
    let resp = frame.show(ui, |ui| {
        ui.set_width(card_w);
        ui.vertical(|ui| {
            // Author line: YouTube + channel.
            ui.horizontal(|ui| {
                ui.label(
                    egui::RichText::new("YouTube")
                        .size(12.0)
                        .color(colors::RED)
                        .strong(),
                );
                if let Some(author) = info.author_name.clone().filter(|a| !a.is_empty()) {
                    ui.label(egui::RichText::new(author).size(12.0).color(colors::TEXT_TERTIARY));
                }
            });
            // Title: clickable, opens the video in the browser.
            let title = info.title.clone().unwrap_or_else(|| "Watch on YouTube".to_string());
            if ui
                .link(
                    egui::RichText::new(title)
                        .size(14.5)
                        .color(colors::TEXT_LINK)
                        .strong(),
                )
                .clicked()
            {
                let _ = open::that_detached(url);
            }
            ui.add_space(4.0);
            // Thumbnail 440x248 (16:9) with the play button, painted INTO
            // the one allocated rect (a second allocate would push the
            // image below the button).
            let thumb_size = Vec2::new(card_w - 28.0, ((card_w - 28.0) * 9.0 / 16.0).min(248.0));
            let (rect, thumb_resp) = ui.allocate_exact_size(thumb_size, Sense::click());
            let painter = ui.painter_at(rect);
            if let Some(handle) = image_loader::global_cache().get_or_fetch(
                ui.ctx(),
                &thumb,
                800,
                450,
                image_loader::Shape::Rounded(6),
            ) {
                let uv = egui::Rect::from_min_max(egui::Pos2::new(0.0, 0.0), egui::Pos2::new(1.0, 1.0));
                painter.image(handle.id(), rect, uv, egui::Color32::WHITE);
            } else {
                painter.rect_filled(rect, 6.0, egui::Color32::from_rgb(0x38, 0x3A, 0x40));
            }
            // Play button: white disc + blurple triangle.
            let center = rect.center();
            let disc_r = (thumb_size.y * 0.16).clamp(20.0, 34.0);
            painter.circle_filled(center, disc_r, egui::Color32::from_black_alpha(140));
            painter.circle_filled(center, disc_r - 2.0, colors::TEXT_PRIMARY);
            let tri = disc_r * 0.55;
            let p1 = center + egui::vec2(-tri * 0.55, -tri);
            let p2 = center + egui::vec2(-tri * 0.55, tri);
            let p3 = center + egui::vec2(tri * 0.85, 0.0);
            painter.add(egui::Shape::convex_polygon(
                vec![p1, p2, p3],
                colors::BLURPLE,
                egui::Stroke::NONE,
            ));
            if thumb_resp.clicked() {
                let _ = open::that_detached(url);
            }
        });
    });
    // Left stripe, YouTube red.
    let cr = resp.response.rect;
    ui.painter_at(cr).rect_filled(
        Rect::from_min_max(egui::pos2(cr.min.x, cr.min.y), egui::pos2(cr.min.x + 4.0, cr.max.y)),
        4.0,
        colors::RED,
    );
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
