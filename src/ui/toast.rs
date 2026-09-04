//! In-app toasts: brief bottom-center notices (copies, update progress,
//! errors) with a slide-in + fade animation.

use std::time::{Duration, Instant};

use once_cell::sync::Lazy;
use parking_lot::Mutex;

use crate::colors;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Info,
    Success,
    Error,
}

#[derive(Clone)]
struct Toast {
    text: String,
    kind: Kind,
    born: Instant,
    life: Duration,
}

static TOASTS: Lazy<Mutex<Vec<Toast>>> = Lazy::new(|| Mutex::new(Vec::new()));

/// Show a toast for `secs` seconds (default 3).
pub fn show(kind: Kind, text: impl Into<String>) {
    let life = match kind {
        Kind::Error => Duration::from_secs(5),
        _ => Duration::from_secs(3),
    };
    TOASTS.lock().push(Toast {
        text: text.into(),
        kind,
        born: Instant::now(),
        life,
    });
}

pub fn info(text: impl Into<String>) {
    show(Kind::Info, text);
}
pub fn success(text: impl Into<String>) {
    show(Kind::Success, text);
}
pub fn error(text: impl Into<String>) {
    show(Kind::Error, text);
}

/// Render all live toasts. Call once per frame from the app layer.
pub fn render(ctx: &egui::Context) {
    let now = Instant::now();
    let toasts: Vec<Toast> = TOASTS
        .lock()
        .iter()
        .filter(|t| now.duration_since(t.born) < t.life)
        .cloned()
        .collect();
    if toasts.is_empty() {
        return;
    }
    ctx.request_repaint_after(Duration::from_millis(80));

    let screen = ctx.viewport_rect();
    let layer = egui::LayerId::new(egui::Order::Foreground, egui::Id::new("toasts"));
    let painter = ctx.layer_painter(layer);
    let mut y = screen.bottom() - 24.0;
    for t in toasts.iter().rev() {
        let age = now.duration_since(t.born);
        // Slide up + fade in over 180ms, fade out over the last 350ms.
        let mut alpha = 1.0;
        let mut slide = 0.0f32;
        if age < Duration::from_millis(180) {
            let k = age.as_secs_f32() / 0.18;
            alpha = k;
            slide = (1.0 - k) * 18.0;
        }
        let remaining = (t.life - age).as_secs_f32();
        if remaining < 0.35 {
            alpha = alpha.min((remaining / 0.35).max(0.0));
        }
        let galley = painter.layout(
            t.text.clone(),
            egui::FontId::proportional(13.5),
            colors::TEXT_PRIMARY,
            f32::INFINITY,
        );
        let w = galley.size().x + 40.0;
        let rect = egui::Rect::from_min_size(
            egui::pos2(screen.center().x - w / 2.0, y - 36.0 + slide),
            egui::vec2(w, 30.0),
        );
        let accent = match t.kind {
            Kind::Info => colors::BLURPLE,
            Kind::Success => colors::STATUS_ONLINE,
            Kind::Error => colors::RED,
        };
        painter.rect_filled(rect, 8.0, egui::Color32::from_rgba_premultiplied(
            (colors::BG_FLOATING.r() as f32 * alpha) as u8,
            (colors::BG_FLOATING.g() as f32 * alpha) as u8,
            (colors::BG_FLOATING.b() as f32 * alpha) as u8,
            (240.0 * alpha) as u8,
        ));
        painter.rect_filled(
            egui::Rect::from_min_size(rect.min, egui::vec2(4.0, 30.0)),
            8.0,
            egui::Color32::from_rgba_premultiplied(accent.r(), accent.g(), accent.b(), (255.0 * alpha) as u8),
        );
        painter.galley(
            egui::pos2(rect.min.x + 20.0, rect.center().y - galley.size().y / 2.0),
            galley,
            egui::Color32::from_white_alpha((255.0 * alpha) as u8),
        );
        y = rect.min.y - 8.0;
    }
    // Drop expired toasts.
    TOASTS.lock().retain(|t| now.duration_since(t.born) < t.life);
}
