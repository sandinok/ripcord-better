//! Real vector icons from Google's Material Symbols library (Apache-2.0).
//!
//! The icon geometry lives in `icons_data.rs` as authentic SVG path strings.
//! At draw time each icon is:
//!   1. parsed (full SVG path grammar: M/L/H/V/C/S/Q/T/A + relatives + Z),
//!   2. flattened to polyline subpaths in 0..1 normalized space,
//!   3. rasterized once per (icon, pixel size) into an antialiased alpha
//!      mask with a nonzero-winding scanline fill + 4x supersampling,
//!   4. cached as a tintable texture and drawn as a single quad.
//!
//! This renders crisply at any DPI and any size, with true holes (the gear,
//! the letter shapes) that hand-drawn primitives cannot express.

#![allow(dead_code)]

use std::collections::HashMap;

use egui::{Color32, ColorImage, Painter, Pos2, Rect, TextureHandle, TextureOptions, Vec2};
use once_cell::sync::OnceCell;
use parking_lot::Mutex;

use crate::icons_data::MATERIAL_ICONS;

/// Supersampling factor for antialiasing (4x = 16 samples per pixel).
const SS: u32 = 4;

// ────────────────────────────── parse ──────────────────────────────

/// One parsed subpath, polyline points in normalized 0..1 space.
type SubPath = Vec<[f32; 2]>;

struct PathParser<'a> {
    s: &'a [u8],
    i: usize,
}

/// Parse an SVG path string into flattened subpaths, normalized to 0..1.
/// `scale` maps SVG user units to 1.0 (e.g. 960 for Material Symbols)
/// and `y_offset` shifts the coordinate system so y lands in 0..scale.
pub fn parse_path(d: &str, scale: f32, y_offset: f32) -> Vec<SubPath> {
    let mut p = PathParser { s: d.as_bytes(), i: 0 };
    let mut subpaths: Vec<SubPath> = Vec::new();
    let mut cur: SubPath = Vec::new();

    // State in SVG user units.
    let mut x = 0.0f32;
    let mut y = 0.0f32;
    let mut start_x = 0.0f32;
    let mut start_y = 0.0f32;
    let mut last_q_control: Option<(f32, f32)> = None;
    let mut last_c_control: Option<(f32, f32)> = None;
    let mut cmd = 0u8;

    let norm = |px: f32, py: f32| -> [f32; 2] {
        [(px) / scale, (py + y_offset) / scale]
    };

    while let Some(c) = p.next_cmd(&mut cmd) {
        match c {
            b'M' | b'm' => {
                if !cur.is_empty() {
                    subpaths.push(std::mem::take(&mut cur));
                }
                let Some((nx, ny)) = p.try_next_pair(c == b'm', x, y) else { continue };
                x = nx;
                y = ny;
                start_x = x;
                start_y = y;
                cur.push(norm(x, y));
                last_q_control = None;
                last_c_control = None;
                // Subsequent implicit pairs are LINETO.
                while let Some((nx, ny)) = p.try_next_pair(c == b'm', x, y) {
                    x = nx;
                    y = ny;
                    cur.push(norm(x, y));
                }
            }
            b'L' | b'l' => {
                while let Some((nx, ny)) = p.try_next_pair(c == b'l', x, y) {
                    x = nx;
                    y = ny;
                    cur.push(norm(x, y));
                }
                last_q_control = None;
                last_c_control = None;
            }
            b'H' | b'h' => {
                while let Some(v) = p.try_next_number() {
                    x = if c == b'h' { x + v } else { v };
                    cur.push(norm(x, y));
                }
                last_q_control = None;
                last_c_control = None;
            }
            b'V' | b'v' => {
                while let Some(v) = p.try_next_number() {
                    y = if c == b'v' { y + v } else { v };
                    cur.push(norm(x, y));
                }
                last_q_control = None;
                last_c_control = None;
            }
            b'C' | b'c' => {
                while let Some((x1, y1, x2, y2, nx, ny)) = p.try_next_cubic(c == b'c', x, y) {
                    flatten_cubic(&mut cur, x, y, x1, y1, x2, y2, nx, ny, &norm);
                    last_c_control = Some((x2, y2));
                    x = nx;
                    y = ny;
                }
                last_q_control = None;
            }
            b'S' | b's' => {
                while let Some((x2, y2, nx, ny)) = p.try_next_shorthand(c == b's', x, y) {
                    let (x1, y1) = match last_c_control {
                        Some((cx, cy)) => (2.0 * x - cx, 2.0 * y - cy),
                        None => (x, y),
                    };
                    flatten_cubic(&mut cur, x, y, x1, y1, x2, y2, nx, ny, &norm);
                    last_c_control = Some((x2, y2));
                    x = nx;
                    y = ny;
                }
                last_q_control = None;
            }
            b'Q' | b'q' => {
                while let Some((x1, y1, nx, ny)) = p.try_next_quad(c == b'q', x, y) {
                    flatten_quad(&mut cur, x, y, x1, y1, nx, ny, &norm);
                    last_q_control = Some((x1, y1));
                    x = nx;
                    y = ny;
                }
                last_c_control = None;
            }
            b'T' | b't' => {
                while let Some((nx, ny)) = p.try_next_pair(c == b't', x, y) {
                    let (x1, y1) = match last_q_control {
                        Some((cx, cy)) => (2.0 * x - cx, 2.0 * y - cy),
                        None => (x, y),
                    };
                    flatten_quad(&mut cur, x, y, x1, y1, nx, ny, &norm);
                    last_q_control = Some((x1, y1));
                    x = nx;
                    y = ny;
                }
                last_c_control = None;
            }
            b'A' | b'a' => {
                while let Some(args) = p.try_next_arc(c == b'a', x, y) {
                    let (rx, ry, rot, large, sweep, nx, ny) = args;
                    flatten_arc(&mut cur, x, y, rx, ry, rot, large, sweep, nx, ny, &norm);
                    x = nx;
                    y = ny;
                }
                last_q_control = None;
                last_c_control = None;
            }
            b'Z' | b'z' => {
                if !cur.is_empty() {
                    // Close back to the subpath start.
                    cur.push(norm(start_x, start_y));
                    x = start_x;
                    y = start_y;
                    subpaths.push(std::mem::take(&mut cur));
                }
                last_q_control = None;
                last_c_control = None;
            }
            _ => {
                // Unknown command: skip it (defensive).
            }
        }
    }
    if !cur.is_empty() {
        subpaths.push(cur);
    }
    subpaths
}

impl<'a> PathParser<'a> {
    fn skip_ws(&mut self) {
        while self.i < self.s.len() {
            let b = self.s[self.i];
            if b == b' ' || b == b'\t' || b == b'\n' || b == b'\r' || b == b',' {
                self.i += 1;
            } else {
                break;
            }
        }
    }

    /// Peek the next command byte, consuming separators. Returns the command
    /// (and stores it in `cmd` for implicit-repeat detection when a number
    /// appears where a command would be expected).
    fn next_cmd(&mut self, cmd: &mut u8) -> Option<u8> {
        self.skip_ws();
        if self.i >= self.s.len() {
            return None;
        }
        let b = self.s[self.i];
        if b.is_ascii_alphabetic() {
            self.i += 1;
            *cmd = b;
            Some(b)
        } else if *cmd != 0 && (b.is_ascii_digit() || b == b'-' || b == b'+' || b == b'.') {
            // SVG allows implicit repeats: "L 10 10 20 20" is two linetos.
            // Do not consume - the number belongs to the repeated command.
            Some(*cmd)
        } else {
            // Junk where a command/number was expected (defensive: never
            // happens in real path data, but guarantees forward progress).
            self.i += 1;
            self.next_cmd(cmd)
        }
    }

    /// Parse a number if one starts at the cursor. Handles SVG's compact
    /// syntax where numbers are separated by nothing but a sign change
    /// ("240-160"), commas, or whitespace.
    fn try_next_number(&mut self) -> Option<f32> {
        self.skip_ws();
        let start = self.i;
        let mut end = start;
        let mut seen_digit = false;
        let mut seen_dot = false;
        let mut seen_exp = false;
        while end < self.s.len() {
            let b = self.s[end];
            if b == b'-' || b == b'+' {
                if end == start {
                    // Leading sign of this number.
                    end += 1;
                    continue;
                }
                let prev = self.s[end - 1];
                if seen_exp && (prev == b'e' || prev == b'E') {
                    // Sign of the exponent.
                    end += 1;
                    continue;
                }
                // Sign of the NEXT number - stop here.
                break;
            }
            if b.is_ascii_digit() {
                seen_digit = true;
                end += 1;
                continue;
            }
            if b == b'.' && !seen_dot && !seen_exp {
                seen_dot = true;
                end += 1;
                continue;
            }
            if (b == b'e' || b == b'E') && seen_digit && !seen_exp {
                seen_exp = true;
                end += 1;
                continue;
            }
            break;
        }
        if !seen_digit || end == start {
            self.i = start;
            return None;
        }
        let text = std::str::from_utf8(&self.s[start..end]).ok()?.trim();
        match text.parse::<f32>() {
            Ok(v) => {
                self.i = end;
                Some(v)
            }
            Err(_) => {
                self.i = start;
                None
            }
        }
    }

    fn try_next_pair(&mut self, relative: bool, cx: f32, cy: f32) -> Option<(f32, f32)> {
        let nx = self.try_next_number()?;
        let ny = self.try_next_number()?;
        if relative {
            Some((cx + nx, cy + ny))
        } else {
            Some((nx, ny))
        }
    }

    fn try_next_quad(&mut self, relative: bool, cx: f32, cy: f32) -> Option<(f32, f32, f32, f32)> {
        let x1 = self.try_next_number()?;
        let y1 = self.try_next_number()?;
        let nx = self.try_next_number()?;
        let ny = self.try_next_number()?;
        if relative {
            Some((cx + x1, cy + y1, cx + nx, cy + ny))
        } else {
            Some((x1, y1, nx, ny))
        }
    }

    fn try_next_cubic(
        &mut self,
        relative: bool,
        cx: f32,
        cy: f32,
    ) -> Option<(f32, f32, f32, f32, f32, f32)> {
        let x1 = self.try_next_number()?;
        let y1 = self.try_next_number()?;
        let x2 = self.try_next_number()?;
        let y2 = self.try_next_number()?;
        let nx = self.try_next_number()?;
        let ny = self.try_next_number()?;
        if relative {
            Some((cx + x1, cy + y1, cx + x2, cy + y2, cx + nx, cy + ny))
        } else {
            Some((x1, y1, x2, y2, nx, ny))
        }
    }

    fn try_next_shorthand(
        &mut self,
        relative: bool,
        cx: f32,
        cy: f32,
    ) -> Option<(f32, f32, f32, f32)> {
        let x2 = self.try_next_number()?;
        let y2 = self.try_next_number()?;
        let nx = self.try_next_number()?;
        let ny = self.try_next_number()?;
        if relative {
            Some((cx + x2, cy + y2, cx + nx, cy + ny))
        } else {
            Some((x2, y2, nx, ny))
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn try_next_arc(
        &mut self,
        relative: bool,
        cx: f32,
        cy: f32,
    ) -> Option<(f32, f32, f32, bool, bool, f32, f32)> {
        let rx = self.try_next_number()?;
        let ry = self.try_next_number()?;
        let rot = self.try_next_number()?;
        let large = self.try_next_arc_flag()?;
        let sweep = self.try_next_arc_flag()?;
        let nx = self.try_next_number()?;
        let ny = self.try_next_number()?;
        if relative {
            Some((rx, ry, rot, large, sweep, cx + nx, cy + ny))
        } else {
            Some((rx, ry, rot, large, sweep, nx, ny))
        }
    }

    fn try_next_arc_flag(&mut self) -> Option<bool> {
        self.skip_ws();
        if self.i < self.s.len() {
            let b = self.s[self.i];
            if b == b'0' {
                self.i += 1;
                return Some(false);
            }
            if b == b'1' {
                self.i += 1;
                return Some(true);
            }
        }
        None
    }
}

// ───────────────────────────── flatten ─────────────────────────────

#[allow(clippy::too_many_arguments)]
fn flatten_quad(cur: &mut SubPath, x0: f32, y0: f32, x1: f32, y1: f32, x2: f32, y2: f32, norm: &impl Fn(f32, f32) -> [f32; 2]) {
    let steps = curve_steps(x0, y0, x1, y1, x2, y2, x1, y1);
    for i in 1..=steps {
        let t = i as f32 / steps as f32;
        let mt = 1.0 - t;
        let px = mt * mt * x0 + 2.0 * mt * t * x1 + t * t * x2;
        let py = mt * mt * y0 + 2.0 * mt * t * y1 + t * t * y2;
        cur.push(norm(px, py));
    }
}

#[allow(clippy::too_many_arguments)]
fn flatten_cubic(
    cur: &mut SubPath,
    x0: f32, y0: f32,
    x1: f32, y1: f32,
    x2: f32, y2: f32,
    x3: f32, y3: f32,
    norm: &impl Fn(f32, f32) -> [f32; 2],
) {
    let steps = curve_steps(x0, y0, x1, y1, x2, y2, x3, y3);
    for i in 1..=steps {
        let t = i as f32 / steps as f32;
        let mt = 1.0 - t;
        let px = mt * mt * mt * x0 + 3.0 * mt * mt * t * x1 + 3.0 * mt * t * t * x2 + t * t * t * x3;
        let py = mt * mt * mt * y0 + 3.0 * mt * mt * t * y1 + 3.0 * mt * t * t * y2 + t * t * t * y3;
        cur.push(norm(px, py));
    }
}

#[allow(clippy::too_many_arguments)]
fn curve_steps(x0: f32, y0: f32, x1: f32, y1: f32, x2: f32, y2: f32, x3: f32, y3: f32) -> u32 {
    // Rough control-polygon length → step count, clamped.
    let d = (x1 - x0).hypot(y1 - y0) + (x2 - x1).hypot(y2 - y1) + (x3 - x2).hypot(y3 - y2);
    let steps = (d / 6.0).ceil() as u32;
    steps.clamp(4, 32)
}

/// Endpoint → center arc conversion, then sample. Follows the SVG spec
/// (F.6.5) — we include it for completeness even though Material Symbols
/// paths in this set don't use A commands.
#[allow(clippy::too_many_arguments)]
fn flatten_arc(
    cur: &mut SubPath,
    x0: f32, y0: f32,
    rx: f32, ry: f32,
    x_rot_deg: f32,
    large_arc: bool,
    sweep: bool,
    x1: f32, y1: f32,
    norm: &impl Fn(f32, f32) -> [f32; 2],
) {
    if rx <= 0.0 || ry <= 0.0 || (x0 == x1 && y0 == y1) {
        cur.push(norm(x1, y1));
        return;
    }
    let (rx, ry) = (rx.abs(), ry.abs());
    let phi = x_rot_deg.to_radians();
    let cos_p = phi.cos();
    let sin_p = phi.sin();
    // Step 1: translate + rotate to circle space.
    let dx = (x0 - x1) / 2.0;
    let dy = (y0 - y1) / 2.0;
    let x1p = cos_p * dx + sin_p * dy;
    let y1p = -sin_p * dx + cos_p * dy;
    // Step 2: correct radii.
    let lambda = (x1p * x1p) / (rx * rx) + (y1p * y1p) / (ry * ry);
    let (rx, ry) = if lambda > 1.0 {
        let s = lambda.sqrt();
        (rx * s, ry * s)
    } else {
        (rx, ry)
    };
    // Step 3: center.
    let num = (rx * rx * ry * ry) - (rx * rx * y1p * y1p) - (ry * ry * x1p * x1p);
    let num = num.max(0.0);
    let den = (rx * rx * y1p * y1p) + (ry * ry * x1p * x1p);
    let co = if den > 0.0 { (num / den).sqrt() } else { 0.0 };
    let co = if large_arc != sweep { -co } else { co };
    let cxp = co * rx * y1p / ry;
    let cyp = -co * ry * x1p / rx;
    let cx = cos_p * cxp - sin_p * cyp + (x0 + x1) / 2.0;
    let cy = sin_p * cxp + cos_p * cyp + (y0 + y1) / 2.0;
    // Step 4: angles.
    let ux = (x1p - cxp) / rx;
    let uy = (y1p - cyp) / ry;
    let vx = (-x1p - cxp) / rx;
    let vy = (-y1p - cyp) / ry;
    let theta = angle_delta(1.0, 0.0, ux, uy);
    let mut delta_theta = angle_delta(ux, uy, vx, vy);
    if !sweep && delta_theta > 0.0 {
        delta_theta -= std::f32::consts::TAU;
    } else if sweep && delta_theta < 0.0 {
        delta_theta += std::f32::consts::TAU;
    }
    let steps = ((delta_theta.abs() / std::f32::consts::FRAC_PI_2) * 12.0).ceil() as u32;
    let steps = steps.clamp(4, 64);
    for i in 1..=steps {
        let t = theta + delta_theta * (i as f32 / steps as f32);
        let px = cx + rx * t.cos() * cos_p - ry * t.sin() * sin_p;
        let py = cy + rx * t.cos() * sin_p + ry * t.sin() * cos_p;
        cur.push(norm(px, py));
    }
}

fn angle_delta(ux: f32, uy: f32, vx: f32, vy: f32) -> f32 {
    let dot = ux * vx + uy * vy;
    let len = (ux * ux + uy * uy).sqrt() * (vx * vx + vy * vy).sqrt();
    let mut angle = (dot / len.max(1e-9)).clamp(-1.0, 1.0).acos();
    if ux * vy - uy * vx < 0.0 {
        angle = -angle;
    }
    angle
}

// ───────────────────────────── rasterize ─────────────────────────────

/// Rasterize subpaths (0..1 space) into a `px * px` alpha mask with a
/// nonzero-winding scanline fill and 4x supersampling.
pub fn rasterize(subpaths: &[SubPath], px: u32) -> Vec<u8> {
    let ss_px = (px * SS) as usize;
    let mut coverage = vec![0u8; ss_px * ss_px];
    let scale = ss_px as f32;
    let to_ss = |p: [f32; 2]| -> [f32; 2] { [p[0] * scale, p[1] * scale] };

    // Pre-project all subpaths into SS space.
    let projected: Vec<Vec<[f32; 2]>> = subpaths
        .iter()
        .map(|sp| sp.iter().map(|&p| to_ss(p)).collect())
        .collect();

    for row in 0..ss_px {
        let y = row as f32 + 0.5;
        // Gather crossings with winding direction.
        let mut crossings: Vec<(f32, i32)> = Vec::new();
        for sp in &projected {
            let n = sp.len();
            if n < 2 {
                continue;
            }
            for i in 0..n {
                let a = sp[i];
                let b = sp[(i + 1) % n];
                let (ya, yb) = (a[1], b[1]);
                if (ya <= y && yb > y) || (yb <= y && ya > y) {
                    let dir: i32 = if yb > ya { 1 } else { -1 };
                    let t = (y - ya) / (yb - ya);
                    let x = a[0] + t * (b[0] - a[0]);
                    crossings.push((x, dir));
                }
            }
        }
        if crossings.is_empty() {
            continue;
        }
        crossings.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal));
        let mut winding = 0i32;
        let mut span_start = 0usize;
        for i in 0..crossings.len() {
            if winding == 0 {
                span_start = i;
            }
            winding += crossings[i].1;
            if winding == 0 {
                // Span from crossings[span_start].0 .. crossings[i].0
                let x0 = crossings[span_start].0;
                let x1 = crossings[i].0;
                let col_start = (x0 - 0.5).ceil().max(0.0) as usize;
                let col_end = ((x1 - 0.5).ceil() as usize).min(ss_px);
                for col in col_start..col_end {
                    coverage[row * ss_px + col] = 255;
                }
            }
        }
    }

    // Downsample SS → alpha.
    let px_us = px as usize;
    let mut out = vec![0u8; px_us * px_us];
    for py in 0..px_us {
        for pxx in 0..px_us {
            let mut acc = 0u32;
            for sy in 0..SS as usize {
                for sx in 0..SS as usize {
                    let r = py * SS as usize + sy;
                    let c = pxx * SS as usize + sx;
                    if coverage[r * ss_px + c] != 0 {
                        acc += 1;
                    }
                }
            }
            out[py * px_us + pxx] = ((acc * 255) / (SS * SS)) as u8;
        }
    }
    out
}

// ───────────────────────────── cache + API ─────────────────────────────

struct IconCache {
    parsed: Vec<Vec<SubPath>>,
    textures: Mutex<HashMap<(usize, u32), TextureHandle>>,
}

static CACHE: OnceCell<IconCache> = OnceCell::new();

fn cache() -> &'static IconCache {
    CACHE.get_or_init(|| {
        let parsed = MATERIAL_ICONS
            .iter()
            .map(|(_, d)| parse_path(d, 960.0, 960.0))
            .collect();
        IconCache {
            parsed,
            textures: Mutex::new(HashMap::new()),
        }
    })
}

fn icon_index(name: &str) -> Option<usize> {
    MATERIAL_ICONS.binary_search_by(|(n, _)| (*n).cmp(name)).ok()
}

/// Fetch (or build) the tintable texture for an icon at a pixel size.
pub fn texture(ctx: &egui::Context, name: &str, px: u32) -> Option<TextureHandle> {
    let idx = icon_index(name)?;
    let px = px.clamp(8, 192);
    let c = cache();
    if let Some(h) = c.textures.lock().get(&(idx, px)).cloned() {
        return Some(h.clone());
    }
    let subpaths = &c.parsed[idx];
    let alpha = rasterize(subpaths, px);
    let mut rgba = Vec::with_capacity((px * px * 4) as usize);
    for &a in &alpha {
        rgba.push(255);
        rgba.push(255);
        rgba.push(255);
        rgba.push(a);
    }
    let img = ColorImage::from_rgba_unmultiplied([px as usize, px as usize], &rgba);
    let handle = ctx.load_texture(format!("icon/{name}/{px}"), img, TextureOptions::LINEAR);
    c.textures.lock().insert((idx, px), handle.clone());
    Some(handle)
}

/// Draw an icon fitted into `rect` (icon fills the rect), tinted `color`.
pub fn draw_rect(painter: &Painter, name: &str, rect: Rect, color: Color32) {
    let px = (rect.width().max(rect.height()) * painter.ctx().pixels_per_point()).ceil() as u32;
    if let Some(handle) = texture(painter.ctx(), name, px) {
        let uv = Rect::from_min_max(Pos2::new(0.0, 0.0), Pos2::new(1.0, 1.0));
        // Fit the 24-unit icon square (which has ~2u padding) into the rect.
        let size = rect.size().min_elem();
        let center = rect.center();
        let dest = Rect::from_center_size(center, Vec2::splat(size));
        painter.image(handle.id(), dest, uv, color);
    }
}

/// Draw an icon centered at `center` with logical side `size`.
pub fn draw(painter: &Painter, name: &str, center: Pos2, size: f32, color: Color32) {
    let rect = Rect::from_center_size(center, Vec2::splat(size));
    draw_rect(painter, name, rect, color);
}

/// Draw an icon centered in a rect (no scaling beyond the given size).
pub fn draw_centered(painter: &Painter, name: &str, rect: Rect, size: f32, color: Color32) {
    draw(painter, name, rect.center(), size, color);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn all_icons_parse_into_bounds() {
        for (name, d) in MATERIAL_ICONS {
            let sp = parse_path(d, 960.0, 960.0);
            assert!(!sp.is_empty(), "{name} produced no subpaths");
            let mut min = [f32::MAX, f32::MAX];
            let mut max = [f32::MIN, f32::MIN];
            for p in &sp {
                for pt in p {
                    min[0] = min[0].min(pt[0]);
                    min[1] = min[1].min(pt[1]);
                    max[0] = max[0].max(pt[0]);
                    max[1] = max[1].max(pt[1]);
                }
            }
            assert!(min[0] >= -0.01 && min[1] >= -0.01, "{name} out of bounds: min {min:?}");
            assert!(max[0] <= 1.01 && max[1] <= 1.01, "{name} out of bounds: max {max:?}");
        }
    }

    #[test]
    fn icons_rasterize_with_partial_coverage() {
        for (name, d) in MATERIAL_ICONS {
            let sp = parse_path(d, 960.0, 960.0);
            let mask = rasterize(&sp, 24);
            let filled = mask.iter().filter(|&&a| a > 128).count();
            assert!(filled > 0, "{name} rasterized to nothing");
            assert!(filled < 24 * 24, "{name} rasterized to a solid block");
        }
    }

    #[test]
    fn settings_icon_has_holes() {
        // The gear has an inner ring that must be transparent (nonzero rule).
        let sp = parse_path("settings", 960.0, 960.0);
        let mask = rasterize(&sp, 48);
        // Sample the exact center of the 48px mask - inside the gear hub hole.
        let center = mask[24 * 48 + 24];
        assert!(center < 100, "gear center should be a hole, got {center}");
    }

    #[test]
    fn relative_commands_parse() {
        // Absolute square.
        let sp = parse_path("M 10 20 L 30 20 Z", 40.0, 0.0);
        assert_eq!(sp.len(), 1);
        assert_eq!(sp[0].len(), 3);
        assert!((sp[0][0][0] - 0.25).abs() < 1e-5);
        assert!((sp[0][0][1] - 0.5).abs() < 1e-5);
        assert!((sp[0][1][0] - 0.75).abs() < 1e-5);

        // Same square with relative commands + compact signs.
        let sp = parse_path("m10 20l20 0z", 40.0, 0.0);
        assert_eq!(sp.len(), 1);
        assert!((sp[0][0][0] - 0.25).abs() < 1e-5, "compact relative form");
        assert!((sp[0][1][0] - 0.75).abs() < 1e-5);

        // Negative numbers packed without separators: "l-20-10".
        let sp = parse_path("M40 40l-20-10", 40.0, 0.0);
        assert!((sp[0][1][0] - 0.5).abs() < 1e-5, "packed negatives x");
        assert!((sp[0][1][1] - 0.75).abs() < 1e-5, "packed negatives y");
    }

    #[test]
    fn icon_lookup_by_name() {
        assert!(icon_index("tag").is_some());
        assert!(icon_index("nonexistent_icon").is_none());
    }
}
