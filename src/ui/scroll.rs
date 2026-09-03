//! Smooth scroll helper. Provides eased scroll-to-position behavior on top
//! of egui's `ScrollArea` so that navigation between channels, jumping to
//! the latest message, or clicking a search result feels polished.
//!
//! Usage:
//!   let mut smoother = SmoothScroll::default();
//!   smoother.target = Some(target_y);
//!   let scroll = egui::ScrollArea::vertical().show(ui, |ui| { ... });
//!   let offset = smoother.update(ctx, &scroll, dt);

use egui::Context;

/// Configuration for an animated scroll.
#[derive(Debug, Clone)]
pub struct SmoothScroll {
    /// The target offset we are easing toward. `None` means "follow the
    /// user's last manual position".
    pub target: Option<f32>,
    /// The current animated offset.
    pub current: f32,
    /// Duration of the ease in seconds.
    pub duration: f32,
    /// Time elapsed since the animation started.
    pub elapsed: f32,
    /// Where the animation started.
    pub from: f32,
}

impl Default for SmoothScroll {
    fn default() -> Self {
        Self {
            target: None,
            current: 0.0,
            duration: 0.32,
            elapsed: 0.0,
            from: 0.0,
        }
    }
}

impl SmoothScroll {
    /// Sets a new target and kicks off a fresh ease from the current position.
    /// No-op if the target is within 0.5px of the existing target.
    pub fn animate_to(&mut self, target: f32) {
        let already_there = self
            .target
            .map(|t| (t - target).abs() < 0.5)
            .unwrap_or(false);
        if already_there {
            return;
        }
        self.from = self.current;
        self.target = Some(target);
        self.elapsed = 0.0;
    }

    /// Snap to a target with no animation.
    pub fn snap_to(&mut self, target: f32) {
        self.target = Some(target);
        self.current = target;
        self.elapsed = self.duration;
    }

    /// Stops the animation, leaving the offset where it is.
    pub fn stop(&mut self) {
        self.target = None;
    }

    /// Returns the (clamped) animated offset after `dt` seconds have passed.
    pub fn update(&mut self, _ctx: &Context, dt: f32) -> Option<f32> {
        if let Some(target) = self.target {
            self.elapsed = (self.elapsed + dt).min(self.duration);
            if self.elapsed >= self.duration {
                self.current = target;
                self.target = None;
            } else {
                let t = self.elapsed / self.duration;
                let eased = ease_out_cubic(t);
                self.current = self.from + (target - self.from) * eased;
            }
            Some(self.current)
        } else {
            None
        }
    }
}

/// Cubic ease-out. Produces a decelerating curve - fast at the start,
/// slow at the end. Good for scroll snap.
pub fn ease_out_cubic(t: f32) -> f32 {
    let t = t.clamp(0.0, 1.0);
    let inv = 1.0 - t;
    1.0 - inv * inv * inv
}

/// Quintic ease-in-out. Used for cross-panel transitions (e.g. login -> app).
pub fn ease_in_out_quint(t: f32) -> f32 {
    let t = t.clamp(0.0, 1.0);
    if t < 0.5 {
        16.0 * t * t * t * t * t
    } else {
        let inv = -2.0 * t + 2.0;
        1.0 - (inv * inv * inv * inv * inv) / 2.0
    }
}

/// Returns the current frame's delta time in seconds, clamped to 100 ms
/// so a long stall (e.g. window resize, OS sleep) doesn't jump an
/// animation past its target.
pub fn dt_from_ctx(ctx: &Context) -> f32 {
    ctx.input(|i| i.unstable_dt).min(0.1)
}
