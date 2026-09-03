//! Login screen. A centered card on a soft stone-dark gradient. Shows the
//! Basalt logo (three hexagonal basalt columns), a token input, and a
//! sign-in button.

use egui::{Align, Color32, Key, Layout, Rect, Stroke, Ui, Vec2};
use egui::epaint::StrokeKind;

use crate::colors;
use crate::config::Config;

pub struct LoginState {
    pub token: String,
    pub error: Option<String>,
    /// 0..=1 entrance animation progress.
    pub enter: f32,
}

impl Default for LoginState {
    fn default() -> Self {
        Self {
            token: String::new(),
            error: None,
            enter: 0.0,
        }
    }
}

pub fn render(ui: &mut Ui, login: &mut LoginState, cfg: &mut Config) -> bool {
    let mut done = false;

    let dt = crate::ui::scroll::dt_from_ctx(ui.ctx()).min(0.040);
    login.enter = (login.enter + dt / 0.45).min(1.0);
    let t = crate::ui::scroll::ease_out_cubic(login.enter);

    let full = ui.max_rect();
    let painter = ui.painter().clone();
    painter.rect_filled(full, 0.0, colors::BG_GUILDS_BAR);
    // Stone gradient: lighter at the top, dark at the bottom.
    painter.rect_filled(
        Rect::from_min_max(
            full.min,
            full.min + egui::vec2(full.width(), full.height() * 0.62),
        ),
        0.0,
        Color32::from_rgba_premultiplied(0x41, 0x46, 0x51, 42),
    );
    // Soft halo behind the card.
    let halo_cx = full.center().x;
    let halo_cy = full.top() + full.height() * 0.34;
    let halo_r = 130.0 + 30.0 * t;
    let halo_alpha = (34.0 * t) as u8;
    painter.circle_filled(
        egui::pos2(halo_cx, halo_cy),
        halo_r,
        Color32::from_rgba_premultiplied(0x58, 0x65, 0xF2, halo_alpha),
    );

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

    let shadow_rect = card_rect.translate(egui::vec2(0.0, 8.0));
    painter.rect_filled(
        shadow_rect,
        12.0,
        Color32::from_rgba_premultiplied(0, 0, 0, (90.0 * t) as u8),
    );

    painter.rect_filled(card_rect, 12.0, colors::BG_FLOATING);
    painter.rect_stroke(card_rect, 12.0, Stroke::new(1.0, colors::BG_ACCENT), StrokeKind::Middle);

    crate::ui::allocate_ui_at_rect(ui, card_rect, |ui| {
        ui.vertical_centered(|ui| {
            ui.add_space(30.0);

            // Logo: three basalt columns.
            let logo_size = 78.0;
            crate::ui::draw_basalt_logo(ui.painter(), card_rect.center().x, ui.next_widget_position().y + logo_size * 0.5, logo_size);
            ui.add_space(logo_size);

            ui.add_space(14.0);
            ui.label(
                egui::RichText::new("Welcome to Basalt")
                    .color(colors::TEXT_HEADER)
                    .size(24.0)
                    .strong(),
            );
            ui.add_space(2.0);
            ui.label(
                egui::RichText::new("Sign in with a Discord token to continue.")
                    .color(colors::TEXT_TERTIARY)
                    .size(13.0),
            );
            ui.add_space(18.0);

            // TOKEN label
            ui.with_layout(Layout::left_to_right(Align::TOP), |ui| {
                ui.add_space(32.0);
                ui.label(
                    egui::RichText::new("TOKEN")
                        .color(colors::TEXT_TERTIARY)
                        .size(11.0)
                        .strong(),
                );
            });
            ui.add_space(4.0);
            let input_w = card_w - 64.0;
            let resp = ui.add(
                egui::TextEdit::singleline(&mut login.token)
                    .password(true)
                    .desired_width(input_w)
                    .hint_text("Paste your Discord token")
                    .text_color(colors::TEXT_PRIMARY),
            );
            ui.add_space(8.0);
            if let Some(e) = &login.error {
                ui.with_layout(Layout::left_to_right(Align::TOP), |ui| {
                    ui.add_space(32.0);
                    ui.label(
                        egui::RichText::new(e)
                            .color(colors::STATUS_DND)
                            .size(12.0),
                    );
                });
                ui.add_space(6.0);
            }

            ui.add_space(4.0);
            let btn = egui::Button::new(
                egui::RichText::new("Sign in")
                    .color(colors::TEXT_PRIMARY)
                    .size(14.0)
                    .strong(),
            )
            .fill(colors::BLURPLE)
            .corner_radius(egui::CornerRadius::same(8))
            .min_size(Vec2::new(input_w, 38.0));
            let btn_resp = ui.add(btn);

            let enter_pressed = resp.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter));
            if btn_resp.clicked() || enter_pressed {
                if login.token.trim().is_empty() {
                    login.error = Some("Token cannot be empty.".into());
                } else {
                    cfg.token = Some(login.token.trim().to_string());
                    let _ = cfg.save();
                    done = true;
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

    let footer = format!("basalt {}", env!("CARGO_PKG_VERSION"));
    painter.text(
        egui::pos2(full.center().x, full.bottom() - 16.0),
        egui::Align2::CENTER_CENTER,
        footer,
        egui::FontId::proportional(11.0),
        colors::TEXT_MUTED,
    );

    done
}
