//! Async image loader. Fetches avatars, guild icons, attachments, and
//! Twemoji PNGs off-thread and installs decoded `egui::ColorImage`
//! textures via `ctx.load_texture()` (which triggers a repaint).
//!
//! Extras:
//!   - CPU-side corner masks (rounded avatars) applied once at decode.
//!   - A failed-fetch cooldown so a dead CDN is not hammered every frame.
//!   - An inline emoji renderer with a fallback URL variant.

use std::collections::HashMap;
use std::time::{Duration, Instant};

use egui::{ColorImage, Context, Painter, Pos2, Rect, Sense, TextureHandle, TextureOptions, Ui, Vec2};
use once_cell::sync::Lazy;
use parking_lot::Mutex;

static GLOBAL_CACHE: Lazy<ImageCache> = Lazy::new(ImageCache::new);

pub fn global_cache() -> &'static ImageCache {
    &GLOBAL_CACHE
}

/// How long a failed fetch is remembered before allowing a retry.
const FAIL_COOLDOWN: Duration = Duration::from_secs(600);

/// Corner shape applied at decode time (baked into the texture alpha).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Shape {
    Square,
    /// Rounded rect; the radius is a fraction of the shorter side.
    Rounded(u8),
    Circle,
}

pub struct ImageCache {
    inner: Mutex<HashMap<String, TextureHandle>>,
    inflight: Mutex<HashMap<String, ()>>,
    failed: Mutex<HashMap<String, Instant>>,
}

impl Default for ImageCache {
    fn default() -> Self {
        Self {
            inner: Mutex::new(HashMap::new()),
            inflight: Mutex::new(HashMap::new()),
            failed: Mutex::new(HashMap::new()),
        }
    }
}

impl ImageCache {
    pub fn new() -> Self {
        Self::default()
    }

    /// Returns a `TextureHandle` if loaded, `None` if not yet ready.
    /// On a miss, kicks off a background fetch (no-op if one is running).
    /// `shape` is part of the cache key, so the same URL can exist as both
    /// square and rounded variants.
    pub fn get_or_fetch(
        &self,
        ctx: &Context,
        url: &str,
        max_w: u32,
        max_h: u32,
        shape: Shape,
    ) -> Option<TextureHandle> {
        let key = cache_key(url, shape);
        if let Some(h) = self.inner.lock().get(&key).cloned() {
            return Some(h);
        }
        // Don't retry a recently-failed URL.
        if let Some(t) = self.failed.lock().get(url) {
            if t.elapsed() < FAIL_COOLDOWN {
                return None;
            }
        }
        let should_spawn = {
            let mut inflight = self.inflight.lock();
            if inflight.contains_key(&key) {
                false
            } else {
                inflight.insert(key.clone(), ());
                true
            }
        };
        if should_spawn {
            let ctx = ctx.clone();
            let url = url.to_string();
            let key = key.clone();
            let max_w = max_w.max(16);
            let max_h = max_h.max(16);
            tokio::spawn(async move {
                match fetch_and_install(&ctx, &url, &key, max_w, max_h, shape).await {
                    Ok(handle) => {
                        global_cache().inner.lock().insert(key.clone(), handle);
                        global_cache().failed.lock().remove(&url);
                    }
                    Err(e) => {
                        tracing::debug!(url = %url, error = %e, "image fetch failed");
                        global_cache().failed.lock().insert(url, Instant::now());
                    }
                }
                global_cache().inflight.lock().remove(&key);
            });
        }
        None
    }

    pub fn stats(&self) -> (usize, usize) {
        let loaded = self.inner.lock().len();
        let inflight = self.inflight.lock().len();
        (loaded, inflight)
    }
}

fn cache_key(url: &str, shape: Shape) -> String {
    match shape {
        Shape::Square => url.to_string(),
        Shape::Rounded(r) => format!("{url}#r{r}"),
        Shape::Circle => format!("{url}#c"),
    }
}

async fn fetch_and_install(
    ctx: &Context,
    url: &str,
    key: &str,
    max_w: u32,
    max_h: u32,
    shape: Shape,
) -> anyhow::Result<TextureHandle> {
    let bytes = reqwest::get(url).await?.error_for_status()?.bytes().await?;
    let img = image::load_from_memory(&bytes)?;
    let img = if img.width() > max_w || img.height() > max_h {
        img.resize(max_w, max_h, image::imageops::FilterType::Lanczos3)
    } else {
        img
    };
    let mut rgba = img.to_rgba8();
    apply_shape(&mut rgba, shape);
    let size = [rgba.width() as usize, rgba.height() as usize];
    let color = ColorImage::from_rgba_unmultiplied(size, &rgba);
    let handle = ctx.load_texture(key, color, TextureOptions::LINEAR);
    Ok(handle)
}

/// Bake a corner shape into the alpha channel of the image.
fn apply_shape(img: &mut image::RgbaImage, shape: Shape) {
    let (w, h) = img.dimensions();
    if w == 0 || h == 0 {
        return;
    }
    let radius = match shape {
        Shape::Square => return,
        Shape::Rounded(r) => {
            let frac = (r as f32) / 100.0;
            (w.min(h) as f32 * frac).round().max(1.0)
        }
        Shape::Circle => w.min(h) as f32 / 2.0,
    };
    let r = radius;
    // Distance from the outside of the nearest corner arc center.
    let corner = |x: f32, y: f32| -> f32 {
        // Arc centers in each corner.
        let cx = x
            .min(r)
            .max((w as f32) - r)
            .max(0.0);
        let cy = y
            .min(r)
            .max((h as f32) - r)
            .max(0.0);
        ((x - cx).hypot(y - cy) - r).clamp(-1.0, 1.0)
    };
    for y in 0..h {
        for x in 0..w {
            let d = corner(x as f32 + 0.5, y as f32 + 0.5);
            if d > 0.0 {
                let px = img.get_pixel_mut(x, y);
                px.0[3] = 0;
            } else if d > -1.0 {
                // One pixel of feathering on the edge for smooth corners.
                let px = img.get_pixel_mut(x, y);
                let cov = (-d * 255.0) as u8;
                px.0[3] = px.0[3].min(cov);
            }
        }
    }
}

/// Paint a status dot in the lower-right corner of an avatar rect.
/// `status` is "online" / "idle" / "dnd" / "offline" / "invisible".
/// The ring is painted in `ring_color` so the dot reads on any background.
pub fn paint_status_dot(painter: &Painter, avatar_rect: Rect, status: &str, ring_color: egui::Color32) {
    let dot_r = avatar_rect.size().min_elem() * 0.16;
    let center = Pos2::new(avatar_rect.right() - dot_r, avatar_rect.bottom() - dot_r);
    let color = crate::colors::status_color(status);
    // Ring.
    painter.circle_filled(center, dot_r + 2.5, ring_color);
    if status == "invisible" {
        // Hollow: ring of the status color around the background color.
        painter.circle_filled(center, dot_r, ring_color);
        painter.circle_stroke(center, dot_r - 1.0, egui::Stroke::new(2.0, color));
    } else {
        painter.circle_filled(center, dot_r, color);
    }
}

/// Convenience widget: render an avatar at `size` px with rounded corners,
/// plus an optional status dot. Shows an initial placeholder while loading.
pub fn render_avatar(
    ui: &mut Ui,
    url: &str,
    size: f32,
    fallback_initials: &str,
    status: Option<&str>,
) {
    render_avatar_ex(ui, url, size, fallback_initials, status, Shape::Rounded(22), crate::colors::BG_FLOATING);
}

/// Avatar with explicit shape + ring color.
#[allow(clippy::too_many_arguments)]
pub fn render_avatar_ex(
    ui: &mut Ui,
    url: &str,
    size: f32,
    fallback_initials: &str,
    status: Option<&str>,
    shape: Shape,
    ring: egui::Color32,
) {
    let cache = global_cache();
    let ctx = ui.ctx().clone();
    let (rect, _) = ui.allocate_exact_size(Vec2::splat(size), Sense::hover());
    let painter = ui.painter_at(rect);
    painter.rect_filled(rect, corner_radius_for(shape, size), egui::Color32::from_rgb(0x38, 0x3A, 0x40));
    if let Some(handle) = cache.get_or_fetch(&ctx, url, (size * 2.0) as u32, (size * 2.0) as u32, shape) {
        let uv = Rect::from_min_max(Pos2::new(0.0, 0.0), Pos2::new(1.0, 1.0));
        painter.image(handle.id(), rect, uv, egui::Color32::WHITE);
    } else {
        painter.text(
            rect.center(),
            egui::Align2::CENTER_CENTER,
            fallback_initials.to_uppercase().chars().next().unwrap_or('?').to_string(),
            egui::FontId::proportional(size * 0.45),
            egui::Color32::WHITE,
        );
    }
    if let Some(st) = status {
        paint_status_dot(&painter, rect, st, ring);
    }
}

fn corner_radius_for(shape: Shape, size: f32) -> f32 {
    match shape {
        Shape::Square => 0.0,
        Shape::Rounded(r) => size * (r as f32) / 100.0,
        Shape::Circle => size / 2.0,
    }
}

/// Convenience widget: render a square/rounded image at `size` px.
pub fn render_image(ui: &mut Ui, url: &str, size: f32, shape: Shape) {
    let cache = global_cache();
    let ctx = ui.ctx().clone();
    if let Some(handle) = cache.get_or_fetch(&ctx, url, (size * 2.0) as u32, (size * 2.0) as u32, shape) {
        let (rect, _) = ui.allocate_exact_size(Vec2::splat(size), Sense::hover());
        let uv = Rect::from_min_max(Pos2::new(0.0, 0.0), Pos2::new(1.0, 1.0));
        ui.painter_at(rect).image(handle.id(), rect, uv, egui::Color32::WHITE);
    } else {
        let (rect, _) = ui.allocate_exact_size(Vec2::splat(size), Sense::hover());
        let painter = ui.painter_at(rect);
        let r = corner_radius_for(shape, size);
        painter.rect_filled(rect, r, egui::Color32::from_rgb(0x38, 0x3A, 0x40));
    }
}

/// Render a custom Discord emoji (`<:name:id>`) inline at `size` px,
/// falling back to `:name:` text while it loads.
pub fn render_emoji(ui: &mut Ui, url: &str, size: f32, name: &str) {
    let cache = global_cache();
    let ctx = ui.ctx().clone();
    if let Some(handle) = cache.get_or_fetch(&ctx, url, (size * 2.0) as u32, (size * 2.0) as u32, Shape::Square) {
        let (rect, _) = ui.allocate_exact_size(Vec2::splat(size), Sense::hover());
        let uv = Rect::from_min_max(Pos2::new(0.0, 0.0), Pos2::new(1.0, 1.0));
        ui.painter_at(rect).image(handle.id(), rect, uv, egui::Color32::WHITE);
    } else {
        ui.label(egui::RichText::new(format!(":{name}:")).size(13.0).color(egui::Color32::from_rgb(0xB5, 0xBA, 0xC1)));
    }
}

/// Render an inline Twemoji at `size` px. Tries `url`, then `fallback_url`,
/// then gives up and renders the raw emoji cluster as text (monochrome
/// fallback - better than a hole in the message).
pub fn render_emoji_inline(ui: &mut Ui, url: &str, fallback_url: &str, size: f32, cluster: &str) {
    let cache = global_cache();
    let ctx = ui.ctx().clone();
    if let Some(handle) = cache.get_or_fetch(&ctx, url, (size * 2.0) as u32, (size * 2.0) as u32, Shape::Square) {
        let (rect, _) = ui.allocate_exact_size(Vec2::splat(size), Sense::hover());
        let uv = Rect::from_min_max(Pos2::new(0.0, 0.0), Pos2::new(1.0, 1.0));
        ui.painter_at(rect).image(handle.id(), rect, uv, egui::Color32::WHITE);
        return;
    }
    if let Some(handle) = cache.get_or_fetch(&ctx, fallback_url, (size * 2.0) as u32, (size * 2.0) as u32, Shape::Square) {
        let (rect, _) = ui.allocate_exact_size(Vec2::splat(size), Sense::hover());
        let uv = Rect::from_min_max(Pos2::new(0.0, 0.0), Pos2::new(1.0, 1.0));
        ui.painter_at(rect).image(handle.id(), rect, uv, egui::Color32::WHITE);
        return;
    }
    // Not loaded (or failed): reserve the space and draw the raw cluster
    // small so layout does not jump when the texture arrives.
    let galley = ui.painter().layout_no_wrap(
        cluster.to_string(),
        egui::FontId::proportional(size * 0.85),
        egui::Color32::from_rgb(0xB5, 0xBA, 0xC1),
    );
    let (rect, _) = ui.allocate_exact_size(Vec2::new(size, size), Sense::hover());
    ui.painter_at(rect).galley(rect.min, galley, egui::Color32::WHITE);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shape_cache_keys_differ() {
        assert_eq!(cache_key("http://x/a.png", Shape::Square), "http://x/a.png");
        assert_eq!(cache_key("http://x/a.png", Shape::Rounded(22)), "http://x/a.png#r22");
        assert_eq!(cache_key("http://x/a.png", Shape::Circle), "http://x/a.png#c");
        assert_ne!(cache_key("http://x/a.png", Shape::Square), cache_key("http://x/a.png", Shape::Rounded(22)));
    }

    #[test]
    fn rounded_mask_cuts_corners() {
        let mut img = image::RgbaImage::from_pixel(20, 20, image::Rgba([255, 0, 0, 255]));
        apply_shape(&mut img, Shape::Circle);
        // Center opaque.
        assert_eq!(img.get_pixel(10, 10).0[3], 255);
        // Corner transparent.
        assert_eq!(img.get_pixel(1, 1).0[3], 0);
    }

    #[test]
    fn square_shape_is_noop() {
        let mut img = image::RgbaImage::from_pixel(8, 8, image::Rgba([1, 2, 3, 200]));
        apply_shape(&mut img, Shape::Square);
        assert_eq!(img.get_pixel(0, 0).0[3], 200);
        assert_eq!(img.get_pixel(7, 7).0[3], 200);
    }
}
