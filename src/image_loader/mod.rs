//! Async image loader. Fetches avatars, guild icons, attachments, and
//! Twemoji PNGs off-thread and installs decoded `egui::ColorImage`
//! textures via `ctx.load_texture()` (which triggers a repaint).
//!
//! Extras:
//!   - CPU-side corner masks (rounded avatars) applied once at decode.
//!   - A failed-fetch cooldown so a dead CDN is not hammered every frame.
//!   - An inline emoji renderer with a fallback URL variant.
//!
//! Two hard rules learned the hard way:
//!   1. Every CDN request MUST carry a User-Agent. cdn.discordapp.com sits
//!      behind Cloudflare, which 403s UA-less requests (reqwest sends none
//!      by default) - that is why avatars/emoji/embed thumbs used to show
//!      as gray placeholders.
//!   2. Decode + resize + mask are CPU work and run in `spawn_blocking`.
//!      They used to run inline on the single tokio worker, where a burst
//!      of image loads starved the gateway heartbeats and Discord dropped
//!      the session ("Connection lost - reconnecting").

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use egui::{ColorImage, Context, Painter, Pos2, Rect, Sense, TextureHandle, TextureOptions, Ui, Vec2};
use once_cell::sync::Lazy;
use parking_lot::Mutex;

static GLOBAL_CACHE: Lazy<ImageCache> = Lazy::new(ImageCache::new);

/// Shared HTTP client for all image fetches: connection pooling, a real
/// User-Agent (Cloudflare rejects UA-less requests), and hard timeouts so a
/// dead CDN can never leave a fetch (and its UI placeholder) stuck forever.
static IMAGE_HTTP: Lazy<reqwest::Client> = Lazy::new(|| {
    reqwest::Client::builder()
        .user_agent(crate::identity::image_user_agent())
        .timeout(Duration::from_secs(15))
        .connect_timeout(Duration::from_secs(8))
        .pool_idle_timeout(Duration::from_secs(90))
        .pool_max_idle_per_host(8)
        .build()
        .expect("image http client init")
});

pub fn global_cache() -> &'static ImageCache {
    &GLOBAL_CACHE
}

/// A decoded animated GIF: frames as textures + per-frame delays.
/// Rendered by `render_animated_image` (server icons animate on hover).
pub struct AnimatedImage {
    frames: Vec<TextureHandle>,
    delays: Vec<Duration>,
    total: Duration,
}

impl AnimatedImage {
    pub fn frame_count(&self) -> usize {
        self.frames.len()
    }
    /// Frame for time `t` since animation start, plus remaining time of it.
    fn frame_at(&self, t: Duration) -> (usize, Duration) {
        if self.frames.is_empty() {
            return (0, Duration::from_millis(100));
        }
        let cycle = self.total.max(Duration::from_millis(1));
        let mut t = (t.as_nanos() % cycle.as_nanos()) as u64;
        for (i, d) in self.delays.iter().enumerate() {
            let d = (*d).max(Duration::from_millis(20));
            if t < d.as_millis() as u64 {
                return (i, d - Duration::from_millis(t));
            }
            t -= d.as_millis() as u64;
        }
        (self.frames.len() - 1, Duration::from_millis(100))
    }
}

static ANIMATED_CACHE: Lazy<Mutex<HashMap<String, Arc<AnimatedImage>>>> =
    Lazy::new(|| Mutex::new(HashMap::new()));
static ANIMATED_INFLIGHT: Lazy<Mutex<std::collections::HashSet<String>>> =
    Lazy::new(|| Mutex::new(std::collections::HashSet::new()));

/// Fetch + decode an animated GIF. `None` while loading or when the URL
/// is not an animated GIF.
pub fn get_or_fetch_animated(ctx: &Context, url: &str, max_px: u32) -> Option<Arc<AnimatedImage>> {
    if let Some(a) = ANIMATED_CACHE.lock().get(url) {
        return Some(a.clone());
    }
    let should_spawn = ANIMATED_INFLIGHT.lock().insert(url.to_string());
    if should_spawn {
        let ctx = ctx.clone();
        let url = url.to_string();
        let max_px = max_px.max(16);
        tokio::spawn(async move {
            if let Some(anim) = fetch_animated(&ctx, &url, max_px).await {
                ANIMATED_CACHE.lock().insert(url.clone(), anim);
            }
            ANIMATED_INFLIGHT.lock().remove(&url);
        });
    }
    None
}

async fn fetch_animated(ctx: &Context, url: &str, max_px: u32) -> Option<Arc<AnimatedImage>> {
    let bytes = IMAGE_HTTP
        .get(url)
        .send()
        .await
        .ok()?
        .error_for_status()
        .ok()?
        .bytes()
        .await
        .ok()?
        .to_vec();
    if !bytes.starts_with(b"GIF8") {
        return None; // only GIFs are animated here
    }
    let ctx2 = ctx.clone();
    let frames = tokio::task::spawn_blocking(move || decode_gif_frames(&ctx2, &bytes, max_px))
        .await
        .ok()??;
    let delays: Vec<Duration> = frames.iter().map(|(_, d)| *d).collect();
    let total: Duration = delays.iter().sum();
    let textures: Vec<TextureHandle> = frames
        .into_iter()
        .map(|(img, _)| ctx.load_texture(format!("{url}#anim{}", rand_hint()), ColorImage::from_rgba_unmultiplied([img.width() as usize, img.height() as usize], &img), TextureOptions::LINEAR))
        .collect();
    Some(Arc::new(AnimatedImage { frames: textures, delays, total }))
}

fn rand_hint() -> u64 {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    Instant::now().hash(&mut h);
    h.finish()
}

fn decode_gif_frames(_ctx: &Context, bytes: &[u8], max_px: u32) -> Option<Vec<(image::RgbaImage, Duration)>> {
    use image::AnimationDecoder as _;
    let decoder = image::codecs::gif::GifDecoder::new(std::io::Cursor::new(bytes)).ok()?;
    let mut out = Vec::new();
    for frame in decoder.into_frames() {
        let frame = frame.ok()?;
        let (numer, denom) = frame.delay().numer_denom_ms();
        let delay = Duration::from_millis((numer as f64 / denom.max(1) as f64).ceil() as u64);
        let mut img = frame.into_buffer();
        let (w, h) = img.dimensions();
        if w > max_px || h > max_px {
            let scale = max_px as f32 / w.max(h) as f32;
            let (tw, th) = ((w as f32 * scale).round() as u32, (h as f32 * scale).round() as u32);
            img = image::imageops::resize(&img, tw.max(1), th.max(1), image::imageops::FilterType::Triangle);
        }
        out.push((img, delay));
    }
    if out.len() < 2 {
        return None; // single-frame or failed: not animated
    }
    Some(out)
}

/// Render an animated GIF in a `size` rect. `playing` = animate (hover);
/// otherwise the first frame shows. Corner radius follows `shape`.
pub fn render_animated_image(ui: &mut Ui, url: &str, size: Vec2, shape: Shape, playing: bool) {
    let ctx = ui.ctx().clone();
    let (rect, _) = ui.allocate_exact_size(size, Sense::hover());
    if let Some(anim) = get_or_fetch_animated(&ctx, url, (size.x.max(size.y) * 2.0) as u32) {
        let start_id = egui::Id::new("anim_start").with(url);
        let start = match ui.ctx().data(|d| d.get_temp::<Instant>(start_id)) {
            Some(t) => t,
            None => {
                let now = Instant::now();
                ui.ctx().data_mut(|d| d.insert_temp(start_id, now));
                now
            }
        };
        let elapsed = start.elapsed();
        let (idx, remaining) = anim.frame_at(elapsed);
        let idx = if playing { idx } else { 0 };
        if playing {
            ui.ctx().request_repaint_after(remaining.max(Duration::from_millis(30)));
        }
        if let Some(handle) = anim.frames.get(idx).or(anim.frames.first()) {
            let uv = Rect::from_min_max(Pos2::new(0.0, 0.0), Pos2::new(1.0, 1.0));
            let painter = ui.painter_at(rect);
            painter.image(handle.id(), rect, uv, egui::Color32::WHITE);
        }
    } else {
        ui.painter_at(rect).rect_filled(rect, corner_radius_for(shape, size.min_elem()), egui::Color32::from_rgb(0x38, 0x3A, 0x40));
    }
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
                    Err(_) => {
                        // Mark the URL as failed so we stop retrying it this
                        // session; the placeholder texture stays in place.
                        global_cache().failed.lock().insert(url, Instant::now());
                    }
                }
                global_cache().inflight.lock().remove(&key);
            });
        }
        None
    }

    /// Drop every cached texture (settings: clear image cache).
    pub fn clear_all(&self) {
        self.inner.lock().clear();
        self.failed.lock().clear();
        ANIMATED_CACHE.lock().clear();
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
    let bytes = IMAGE_HTTP
        .get(url)
        .send()
        .await?
        .error_for_status()?
        .bytes()
        .await?
        .to_vec();
    // Decode + resize + corner-mask are CPU-heavy (Lanczos3 on a 600px
    // banner is ~100ms+). Run them on the blocking pool so the async
    // worker stays free for gateway heartbeats.
    let rgba = tokio::task::spawn_blocking(move || -> anyhow::Result<image::RgbaImage> {
        let img = image::load_from_memory(&bytes)?;
        let img = if img.width() > max_w || img.height() > max_h {
            img.resize(max_w, max_h, image::imageops::FilterType::Lanczos3)
        } else {
            img
        };
        let mut rgba = img.to_rgba8();
        apply_shape(&mut rgba, shape);
        Ok(rgba)
    })
    .await??;
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
    // Clamp the radius so the core rect stays valid on narrow images.
    let r = radius.min(w as f32 / 2.0).min(h as f32 / 2.0);
    // Signed distance to the rounded-rect boundary: clamp the point into
    // the "core" rect (the rect minus the corner radii), then measure the
    // distance to the clamped point. The old expression used
    // `x.min(r).max(w-r)`, which collapses to `w-r` for nearly every x and
    // wiped the whole image except a small blob at the bottom-right
    // corner - that was the "avatars render as tiny dots" bug.
    let corner = |x: f32, y: f32| -> f32 {
        let cx = x.clamp(r, (w as f32) - r);
        let cy = y.clamp(r, (h as f32) - r);
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

/// Convenience widget: render an avatar at `size` px, plus an optional
/// status dot. Discord renders avatars as full circles everywhere (message
/// rows, member list, user box); the fallback shows the user's initial
/// while the texture loads.
pub fn render_avatar(
    ui: &mut Ui,
    url: &str,
    size: f32,
    fallback_initials: &str,
    status: Option<&str>,
) {
    render_avatar_ex(ui, url, size, fallback_initials, status, Shape::Circle, crate::colors::BG_FLOATING);
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

/// Compute the display size for an image with a known source aspect
/// ratio, constrained to fit inside `max_w x max_h` (like CSS
/// `max-width/max-height` with `auto` on the other axis). Returns a
/// square when the source dimensions are unknown.
pub fn fit_size(src_w: Option<u32>, src_h: Option<u32>, max_w: f32, max_h: f32) -> Vec2 {
    match (src_w, src_h) {
        (Some(w), Some(h)) if w > 0 && h > 0 => {
            let ar = w as f32 / h as f32;
            let mut out = egui::vec2(max_w, max_w / ar);
            if out.y > max_h {
                out = egui::vec2(max_h * ar, max_h);
            }
            out
        }
        _ => Vec2::splat(max_w.min(max_h)),
    }
}

/// Convenience widget: render a square/rounded image at `size` px.
pub fn render_image(ui: &mut Ui, url: &str, size: f32, shape: Shape) {
    render_image_size(ui, url, Vec2::splat(size), shape);
}

/// Render an image inside a `size` (w x h) rect. The rect is reserved even
/// while the texture loads, so layout never jumps; embed thumbnails and
/// attachments use this with their real aspect ratio instead of a forced
/// square (which stretched every unfurled image).
pub fn render_image_size(ui: &mut Ui, url: &str, size: Vec2, shape: Shape) {
    let cache = global_cache();
    let ctx = ui.ctx().clone();
    let max_w = (size.x.max(1.0) * 2.0) as u32;
    let max_h = (size.y.max(1.0) * 2.0) as u32;
    let (rect, _) = ui.allocate_exact_size(size, Sense::hover());
    if let Some(handle) = cache.get_or_fetch(&ctx, url, max_w, max_h, shape) {
        let uv = Rect::from_min_max(Pos2::new(0.0, 0.0), Pos2::new(1.0, 1.0));
        ui.painter_at(rect).image(handle.id(), rect, uv, egui::Color32::WHITE);
    } else {
        let painter = ui.painter_at(rect);
        let r = corner_radius_for(shape, size.min_elem());
        painter.rect_filled(rect, r, egui::Color32::from_rgb(0x38, 0x3A, 0x40));
    }
}

/// A clickable custom-emoji cell for picker grids: returns true when
/// clicked. Allocates its own rect; falls back to text while loading.
pub fn render_emoji_cell(ui: &mut Ui, url: &str, size: f32, name: &str) -> bool {
    let cache = global_cache();
    let ctx = ui.ctx().clone();
    let (rect, resp) = ui.allocate_exact_size(Vec2::splat(size), Sense::click());
    let resp = resp
        .on_hover_cursor(egui::CursorIcon::PointingHand)
        .on_hover_text(format!(":{name}:"));
    if let Some(handle) = cache.get_or_fetch(&ctx, url, (size * 2.0) as u32, (size * 2.0) as u32, Shape::Square) {
        let uv = Rect::from_min_max(Pos2::new(0.0, 0.0), Pos2::new(1.0, 1.0));
        ui.painter_at(rect).image(handle.id(), rect, uv, egui::Color32::WHITE);
    } else {
        ui.painter_at(rect).text(
            rect.center(),
            egui::Align2::CENTER_CENTER,
            format!(":{name}:"),
            egui::FontId::proportional(11.0),
            egui::Color32::from_rgb(0xB5, 0xBA, 0xC1),
        );
    }
    resp.clicked()
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
/// Draw an image so it COVERS `rect` (aspect-fill, center crop) like a
/// CSS `background-size: cover`. Used for guild banners. Falls back to a
/// flat fill while loading.
pub fn draw_cover_image(ui: &mut Ui, rect: Rect, url: &str, max_w: u32, max_h: u32) {
    let cache = global_cache();
    let ctx = ui.ctx().clone();
    if let Some(handle) = cache.get_or_fetch(&ctx, url, max_w, max_h, Shape::Square) {
        let (tw, th) = (handle.size_vec2().x.max(1.0), handle.size_vec2().y.max(1.0));
        let target_ar = rect.width() / rect.height().max(1.0);
        let src_ar = tw / th;
        let uv = if src_ar > target_ar {
            // Source wider: crop the sides.
            let keep = target_ar / src_ar;
            let cut = (1.0 - keep) / 2.0;
            Rect::from_min_max(Pos2::new(cut, 0.0), Pos2::new(1.0 - cut, 1.0))
        } else {
            // Source taller: crop top/bottom.
            let keep = src_ar / target_ar;
            let cut = (1.0 - keep) / 2.0;
            Rect::from_min_max(Pos2::new(0.0, cut), Pos2::new(1.0, 1.0 - cut))
        };
        ui.painter_at(rect).image(handle.id(), rect, uv, egui::Color32::WHITE);
    } else {
        // Loading: flat sidebar-tinted placeholder so the layout is stable.
        ui.painter_at(rect).rect_filled(rect, 0.0, egui::Color32::from_rgb(0x1E, 0x1F, 0x22));
    }
}

/// Draw an emoji cluster at an explicit rect (picker grids and other manual
/// layouts). Same cache/fallback chain as `render_emoji_inline`, but the
/// caller controls placement.
pub fn draw_emoji_at(ui: &mut Ui, rect: egui::Rect, url: &str, fallback_url: &str, cluster: &str) {
    let cache = global_cache();
    let ctx = ui.ctx().clone();
    let px = rect.width().max(8.0) as u32;
    let uv = Rect::from_min_max(Pos2::new(0.0, 0.0), Pos2::new(1.0, 1.0));
    if let Some(handle) = cache.get_or_fetch(&ctx, url, px, px, Shape::Square) {
        ui.painter_at(rect).image(handle.id(), rect, uv, egui::Color32::WHITE);
        return;
    }
    if let Some(handle) = cache.get_or_fetch(&ctx, fallback_url, px, px, Shape::Square) {
        ui.painter_at(rect).image(handle.id(), rect, uv, egui::Color32::WHITE);
        return;
    }
    let galley = ui.painter().layout_no_wrap(
        cluster.to_string(),
        egui::FontId::proportional(rect.width() * 0.8),
        egui::Color32::from_rgb(0xB5, 0xBA, 0xC1),
    );
    ui.painter_at(rect).galley(rect.min, galley, egui::Color32::WHITE);
}

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
    fn fit_size_preserves_aspect() {
        // 16:9 source in a 384x384 box -> 384x216.
        let s = fit_size(Some(1600), Some(900), 384.0, 384.0);
        assert!((s.x - 384.0).abs() < 0.5, "got {s:?}");
        assert!((s.y - 216.0).abs() < 0.5, "got {s:?}");
        // Portrait 9:16 in a 384x384 box -> height-bound: 216x384.
        let s = fit_size(Some(900), Some(1600), 384.0, 384.0);
        assert!((s.x - 216.0).abs() < 0.5, "got {s:?}");
        assert!((s.y - 384.0).abs() < 0.5, "got {s:?}");
        // Unknown source -> square of the smaller bound.
        let s = fit_size(None, None, 80.0, 384.0);
        assert_eq!(s, Vec2::splat(80.0));
    }

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

    /// Regression test for the "avatars as dots" bug: `x.min(r).max(w-r)`
    /// collapsed to `w-r` for nearly every x, erasing everything but a
    /// bottom-right blob. A rounded mask must keep the image body visible.
    #[test]
    fn rounded_mask_keeps_body_opaque_on_wide_images() {
        let (w, h) = (160u32, 90u32);
        let mut img = image::RgbaImage::from_pixel(w, h, image::Rgba([10, 20, 30, 255]));
        apply_shape(&mut img, Shape::Rounded(4));
        // Pixels with any visibility (the 1px edge feather counts).
        let visible = (0..h)
            .flat_map(|y| (0..w).map(move |x| (x, y)))
            .filter(|&(x, y)| img.get_pixel(x, y).0[3] > 0)
            .count();
        // Over 97% of the image must remain visible (only corner arcs and
        // the feather band at the perimeter are cut).
        assert!(
            visible as f32 / (w * h) as f32 > 0.97,
            "rounded mask erased the image body: only {visible}/{} visible",
            w * h
        );
        // And the extreme corners ARE cut.
        assert_eq!(img.get_pixel(0, 0).0[3], 0);
        assert_eq!(img.get_pixel(w - 1, h - 1).0[3], 0);
    }

    #[test]
    fn square_shape_is_noop() {
        let mut img = image::RgbaImage::from_pixel(8, 8, image::Rgba([1, 2, 3, 200]));
        apply_shape(&mut img, Shape::Square);
        assert_eq!(img.get_pixel(0, 0).0[3], 200);
        assert_eq!(img.get_pixel(7, 7).0[3], 200);
    }
}
