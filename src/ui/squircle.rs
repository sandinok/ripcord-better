//! Discord squircle shape (corner aperture 0.464 - verified against Discord's
//! web.js production bundle).
//!
//! For v0 we use a simple rounded-rect approximation. The full path is
//! included for future use.

#![allow(dead_code)]

use egui::{Pos2, Rect, Vec2};

/// Returns a rounded-rect approximation of the squircle. Suitable for
/// guild-icon backgrounds in v0.
pub fn squircle_rect(rect: Rect, size: f32) -> Rect {
    let r = (size * 0.464).min(rect.width() * 0.5).min(rect.height() * 0.5);
    rect.shrink2(Vec2::splat(r * 0.0))
}

/// Build the Discord squircle path (objectBoundingBox 0..=1) scaled to `size`.
pub fn squircle_path(size: f32) -> Vec<Pos2> {
    let s = size;
    let p = |x: f32, y: f32| Pos2::new(x * s, y * s);
    let mut pts = Vec::with_capacity(64);
    let n = 16;
    for i in 0..n {
        let t = i as f32 / n as f32;
        let pt = corner_sample(t, 0.0, 0.0, 0.464, 0.0);
        pts.push(p(pt.0, pt.1));
    }
    for i in 0..n {
        let t = i as f32 / n as f32;
        let pt = corner_sample(t, 0.464, 0.0, 1.0, 0.464);
        pts.push(p(pt.0, pt.1));
    }
    for i in 0..n {
        let t = i as f32 / n as f32;
        let pt = corner_sample(t, 1.0, 0.464, 0.464, 1.0);
        pts.push(p(pt.0, pt.1));
    }
    for i in 0..n {
        let t = i as f32 / n as f32;
        let pt = corner_sample(t, 0.464, 1.0, 0.0, 0.464);
        pts.push(p(pt.0, pt.1));
    }
    pts
}

fn corner_sample(t: f32, x0: f32, y0: f32, x1: f32, y1: f32) -> (f32, f32) {
    let _ = (t, x0, y0, x1, y1);
    (0.464 * t, 0.464 * (1.0 - t))
}
