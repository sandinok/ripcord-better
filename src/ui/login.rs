//! Login screen, v0.2 rework (point 33): animated background, centered
//! card with a pulsing logo, token field with show/hide + paste, loading
//! state on the sign-in button, animated error feedback, and a polished
//! login -> splash -> app transition.

use std::time::{Duration, Instant};

use egui::epaint::StrokeKind;
use egui::{Align, Color32, Key, Layout, Rect, Sense, Stroke, Ui, Vec2};

use crate::colors;
use crate::config::Config;

pub struct LoginState {
    pub token: String,
    pub error: Option<String>,
    /// 0..=1 entrance animation progress.
    pub enter: f32,
    /// Show the token in clear text.
    pub show_token: bool,
    /// Login flow phase.
    pub phase: Phase,
    /// When the current phase started (animations).
    pub phase_at: Option<Instant>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Phase {
    /// Card with the token field.
    Input,
    /// Validating the token against REST (spinner).
    Validating,
    /// Signed in; waiting for the app shell (splash).
    Splash,
}

impl Default for LoginState {
    fn default() -> Self {
        Self {
            token: String::new(),
            error: None,
            enter: 0.0,
            show_token: false,
            phase: Phase::Input,
            phase_at: Some(Instant::now()),
        }
    }
}

impl LoginState {
    fn set_phase(&mut self, phase: Phase) {
        self.phase = phase;
        self.phase_at = Some(Instant::now());
    }
}

/// Returns true exactly once, when the session should start.
pub fn render(ui: &mut Ui, login: &mut LoginState, cfg: &mut Config) -> bool {
    let mut done = false;
    let full = ui.max_rect();

    // ── Background: dark stone + slow drifting blurple/teal blobs ──
    let painter = ui.painter().clone();
    painter.rect_filled(full, 0.0, colors::BG_GUILDS_BAR);
    let time = ui.input(|i| i.time) as f32;
    ui.ctx().request_repaint_after(Duration::from_millis(50));
    let blobs: [(Color32, f32, f32, f32); 3] = [
        (
            Color32::from_rgb(0x58, 0x65, 0xF2),
            full.center().x + (time * 26.0).sin() * 120.0,
            full.center().y + (time * 17.0).cos() * 80.0,
            240.0,
        ),
        (
            Color32::from_rgb(0x2E, 0x9C, 0xB8),
            full.min.x + 140.0 + (time * 20.0 + 2.0).cos() * 90.0,
            full.max.y - 120.0 + (time * 23.0).sin() * 60.0,
            200.0,
        ),
        (
            Color32::from_rgb(0x41, 0x46, 0x51),
            full.max.x - 160.0 + (time * 15.0).sin() * 70.0,
            full.min.y + 130.0 + (time * 19.0).cos() * 50.0,
            170.0,
        ),
    ];
    for (color, x, y, r) in blobs {
        // Layered translucent discs read as a soft blur without shaders.
        for i in (0..4).rev() {
            let alpha = 7.0 - i as f32 * 1.4;
            let rad = r * (1.0 - i as f32 * 0.16);
            painter.circle_filled(egui::pos2(x, y), rad, color.gamma_multiply(alpha / 30.0));
        }
    }

    // ── Card entrance / splash fade-out ──
    let dt = crate::ui::scroll::dt_from_ctx(ui.ctx()).min(0.040);
    login.enter = (login.enter + dt / 0.45).min(1.0);
    let t = crate::ui::scroll::ease_out_cubic(login.enter);

    match login.phase {
        Phase::Input | Phase::Validating => {
            // Card (fades + slides in, fades out during validating).
            let fade = match login.phase {
                Phase::Input | Phase::Splash => t,
                Phase::Validating => 1.0,
            };
            let card_w = 430.0;
            let card_h = 480.0;
            let slide = (1.0 - t) * 16.0;
            let card_rect = Rect::from_min_size(
                egui::pos2(
                    full.center().x - card_w * 0.5,
                    full.center().y - card_h * 0.5 - slide,
                ),
                egui::vec2(card_w, card_h),
            );
            // Error shake: decaying sine after the phase timestamp.
            let shake = error_shake(login);
            let card_rect = card_rect.translate(egui::vec2(shake, 0.0));
            let painter = ui.painter().clone();
            painter.rect_filled(
                card_rect.translate(egui::vec2(0.0, 8.0)),
                12.0,
                Color32::from_black_alpha((90.0 * fade) as u8),
            );
            painter.rect_filled(card_rect, 12.0, colors::BG_FLOATING);
            painter.rect_stroke(
                card_rect,
                12.0,
                Stroke::new(1.0, colors::BG_ACCENT),
                StrokeKind::Middle,
            );

            crate::ui::allocate_ui_at_rect(ui, card_rect, |ui| {
                ui.vertical_centered(|ui| {
                    ui.add_space(30.0);
                    // Logo: gentle breathing scale.
                    let logo_size = 78.0 + 3.0 * (time * 1.4).sin();
                    crate::ui::draw_basalt_logo(
                        ui.painter(),
                        card_rect.center().x,
                        ui.next_widget_position().y + logo_size * 0.5,
                        logo_size,
                    );
                    ui.add_space(logo_size + 8.0);
                    ui.label(
                        egui::RichText::new("Welcome back")
                            .color(colors::TEXT_HEADER)
                            .size(24.0)
                            .strong(),
                    );
                    ui.label(
                        egui::RichText::new("Glad to see you again. Enter your token.")
                            .color(colors::TEXT_TERTIARY)
                            .size(13.0),
                    );
                    ui.add_space(18.0);

                    // TOKEN label + show/hide toggle hint
                    ui.with_layout(Layout::left_to_right(Align::TOP), |ui| {
                        ui.add_space(32.0);
                        ui.label(
                            egui::RichText::new("TOKEN")
                                .color(colors::TEXT_TERTIARY)
                                .size(11.0)
                                .strong(),
                        );
                        ui.with_layout(Layout::right_to_left(Align::TOP), |ui| {
                            ui.add_space(32.0);
                            let (r, resp) = ui.allocate_exact_size(Vec2::splat(18.0), Sense::click());
                            let resp = resp
                                .on_hover_cursor(egui::CursorIcon::PointingHand)
                                .on_hover_text(if login.show_token { "Hide token" } else { "Show token" });
                            let ip = ui.painter();
                            crate::icons::draw(
                                ip,
                                if login.show_token { "visibility" } else { "visibility_off" },
                                r.center(),
                                16.0,
                                colors::TEXT_TERTIARY,
                            );
                            if resp.clicked() {
                                login.show_token = !login.show_token;
                            }
                        });
                    });
                    ui.add_space(4.0);
                    let input_w = card_w - 64.0;
                    let mut token_edit = egui::TextEdit::singleline(&mut login.token)
                        .desired_width(input_w - 34.0)
                        .hint_text("Paste your Discord token")
                        .text_color(colors::TEXT_PRIMARY);
                    if !login.show_token {
                        token_edit = token_edit.password(true);
                    }
                    let resp = ui.add(token_edit);
                    // Paste button on the right of the input.
                    {
                        let btn_rect = Rect::from_min_size(
                            egui::pos2(card_rect.min.x + 32.0 + input_w - 30.0, resp.rect.min.y),
                            Vec2::new(30.0, resp.rect.height()),
                        );
                        let bresp = ui
                            .interact(btn_rect, ui.id().with("paste_token"), Sense::click())
                            .on_hover_cursor(egui::CursorIcon::PointingHand)
                            .on_hover_text("Paste from clipboard");
                        let bp = ui.painter_at(btn_rect);
                        crate::icons::draw(
                            &bp,
                            "content_copy",
                            btn_rect.center(),
                            17.0,
                            if bresp.hovered() { colors::TEXT_PRIMARY } else { colors::TEXT_TERTIARY },
                        );
                        if bresp.clicked() {
                            // Ask the platform layer to paste into the
                            // (about-to-be-focused) token field: the OS
                            // clipboard read happens natively, the pasted
                            // text lands in the TextEdit like Ctrl+V.
                            resp.request_focus();
                            ui.ctx().send_viewport_cmd(egui::ViewportCommand::RequestPaste);
                        }
                    }

                    ui.add_space(8.0);
                    // Error with slide-in + fade.
                    if let Some(e) = error_text(login) {
                        let err_t = login
                            .phase_at
                            .map(|at| at.elapsed().as_secs_f32().min(0.25) / 0.25)
                            .unwrap_or(1.0)
                            .clamp(0.0, 1.0);
                        ui.with_layout(Layout::left_to_right(Align::TOP), |ui| {
                            ui.add_space(32.0 + (1.0 - err_t) * 8.0);
                            let alpha = err_t;
                            ui.label(
                                egui::RichText::new(e)
                                    .color(egui::Color32::from_rgba_premultiplied(
                                        colors::STATUS_DND.r(),
                                        colors::STATUS_DND.g(),
                                        colors::STATUS_DND.b(),
                                        (255.0 * alpha) as u8,
                                    ))
                                    .size(12.0),
                            );
                        });
                        ui.add_space(6.0);
                    }

                    ui.add_space(6.0);
                    // Sign-in button: idle / loading states.
                    let busy = login.phase == Phase::Validating;
                    let input_ok = !login.token.trim().is_empty();
                    let fill = if busy {
                        colors::BLURPLE_ACTIVE
                    } else if input_ok {
                        colors::BLURPLE
                    } else {
                        colors::BG_ACCENT
                    };
                    let (btn_rect, btn_resp) =
                        ui.allocate_exact_size(Vec2::new(input_w, 38.0), Sense::click());
                    let btn_resp = btn_resp
                        .on_hover_cursor(egui::CursorIcon::PointingHand)
                        .on_disabled_hover_text("Enter a token first");
                    let bp = ui.painter_at(btn_rect);
                    bp.rect_filled(btn_rect, 8.0, fill);
                    if busy {
                        // Spinner + "Validating...".
                        let spin = (time * 6.0) as i32;
                        let c = btn_rect.center();
                        let a = spin as f32 * 0.7;
                        bp.line_segment(
                            [c, c + egui::vec2(a.cos() * 10.0, a.sin() * 10.0)],
                            Stroke::new(2.5, colors::TEXT_PRIMARY),
                        );
                        bp.circle_stroke(c, 11.0, Stroke::new(2.0, colors::TEXT_PRIMARY.gamma_multiply(0.35)));
                        bp.text(
                            egui::pos2(c.x + 30.0, c.y),
                            egui::Align2::LEFT_CENTER,
                            "Validating with Discord...",
                            egui::FontId::proportional(13.0),
                            colors::TEXT_PRIMARY,
                        );
                        ui.ctx().request_repaint_after(Duration::from_millis(60));
                    } else {
                        bp.text(
                            btn_rect.center(),
                            egui::Align2::CENTER_CENTER,
                            "Sign in",
                            egui::FontId::proportional(14.0),
                            colors::TEXT_PRIMARY,
                        );
                    }

                    let enter_pressed = resp.has_focus() && ui.input(|i| i.key_pressed(Key::Enter));
                    if (btn_resp.clicked() || enter_pressed) && !busy {
                        if login.token.trim().is_empty() {
                            login.error = Some("Token cannot be empty.".into());
                            login.set_phase(Phase::Input);
                        } else {
                            login.error = None;
                            login.set_phase(Phase::Validating);
                            // Validate against the real API, then sign in.
                            let _token = login.token.trim().to_string();
                            let ctx = ui.ctx().clone();
                            let cfg_token = login.token.trim().to_string();
                            tokio::spawn(async move {
                                let rest = crate::rest::Http::new(Some(cfg_token.clone()))
                                    .expect("rest client for login validation");
                                crate::scrub::set_live_token(&cfg_token);
                                let mut result = rest.get_current_user().await;
                                if let Err(crate::rest::HttpError::Discord(code, _)) = &result {
                                    if code.as_u16() == 401 {
                                        rest.set_bot_prefix(true);
                                        result = rest.get_current_user().await;
                                    }
                                }
                                let ok = result.is_ok();
                                let err = result.err().map(|e| format!("Discord rejected this token: {e}"));
                                ctx.data_mut(|d| {
                                    d.insert_temp(
                                        egui::Id::new("login_result"),
                                        (ok, err),
                                    )
                                });
                                ctx.request_repaint();
                            });
                        }
                    }

                    // Poll the validation result.
                    if login.phase == Phase::Validating {
                        if let Some((ok, err)) = ui.ctx().data(|d| {
                            d.get_temp::<(bool, Option<String>)>(egui::Id::new("login_result"))
                        }) {
                            ui.ctx().data_mut(|d| {
                                d.remove_temp::<(bool, Option<String>)>(egui::Id::new("login_result"))
                            });
                            if ok {
                                login.set_phase(Phase::Splash);
                            } else {
                                login.error = Some(err.unwrap_or_else(|| "Invalid token.".into()));
                                login.set_phase(Phase::Input);
                            }
                        }
                    }

                    ui.add_space(14.0);
                    ui.with_layout(Layout::left_to_right(Align::TOP), |ui| {
                        ui.add_space(32.0);
                        ui.vertical(|ui| {
                            ui.label(
                                egui::RichText::new("How to get a token")
                                    .color(colors::TEXT_TERTIARY)
                                    .size(11.0)
                                    .strong(),
                            );
                            ui.add_space(2.0);
                            ui.label(
                                egui::RichText::new(
                                    "Open discord.com in a browser, open DevTools (F12),\n\
                                     Application > Local Storage > discord.com > token.",
                                )
                                .color(colors::TEXT_MUTED)
                                .size(11.0),
                            );
                            ui.add_space(6.0);
                            ui.label(
                                egui::RichText::new(
                                    "Bot tokens from the Developer Portal work too, and\n\
                                     keep you clear of the user-token ToS gray zone.",
                                )
                                .color(colors::STATUS_IDLE)
                                .size(11.0),
                            );
                        });
                    });
                    ui.add_space(26.0);
                });
            });
        }
        Phase::Splash => {
            // Transition: logo scales up + "Connecting..." then hands off
            // to the app shell (done=true after ~600ms).
            let age = login
                .phase_at
                .map(|at| at.elapsed().as_secs_f32())
                .unwrap_or(0.0);
            let k = (age / 0.6).min(1.0);
            let logo_size = 78.0 + 60.0 * crate::ui::scroll::ease_out_cubic(k);
            let alpha = (1.0 - k * 0.6).max(0.0);
            let painter = ui.painter().clone();
            painter.rect_filled(
                full,
                0.0,
                Color32::from_rgba_premultiplied(0x1E, 0x1F, 0x22, (255.0 * alpha) as u8),
            );
            crate::ui::draw_basalt_logo(
                &painter,
                full.center().x,
                full.center().y,
                logo_size,
            );
            painter.text(
                egui::pos2(full.center().x, full.center().y + logo_size * 0.62 + 18.0),
                egui::Align2::CENTER_CENTER,
                "Connecting to Discord...",
                egui::FontId::proportional(14.0),
                colors::TEXT_TERTIARY,
            );
            ui.ctx().request_repaint_after(Duration::from_millis(40));
            if age > 0.6 {
                cfg.set_plain_token(login.token.trim());
                let _ = cfg.save();
                done = true;
            }
        }
    }

    let footer = format!("basalt {}", env!("CARGO_PKG_VERSION"));
    ui.painter().text(
        egui::pos2(full.center().x, full.bottom() - 16.0),
        egui::Align2::CENTER_CENTER,
        footer,
        egui::FontId::proportional(11.0),
        colors::TEXT_MUTED,
    );

    done
}

/// Decaying horizontal shake for error feedback.
fn error_shake(login: &LoginState) -> f32 {
    if login.error.is_none() {
        return 0.0;
    }
    let age = login
        .phase_at
        .map(|at| at.elapsed().as_secs_f32())
        .unwrap_or(1.0);
    if age > 0.4 {
        return 0.0;
    }
    (age * 40.0).sin() * (1.0 - age / 0.4) * 7.0
}

/// Error text (owned) with the generic-invalid shaping.
fn error_text(login: &LoginState) -> Option<String> {
    login.error.clone()
}
