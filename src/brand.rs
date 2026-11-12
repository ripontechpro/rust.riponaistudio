//! Ripon AI Photo Studio branding: name, colours, logo.
use eframe::egui::{self, Color32};

pub const NAME: &str = "RIPON AI PHOTO STUDIO";
pub const TITLE: &str = "Ripon AI Photo Studio";
pub const TAGLINE: &str = "EDIT  \u{2022}  ENHANCE  \u{2022}  CREATE";
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

pub const BLUE: Color32 = Color32::from_rgb(0x33, 0xa1, 0xff);
pub const BLUE_DK: Color32 = Color32::from_rgb(0x1a, 0x6f, 0xc4);
pub const ORANGE: Color32 = Color32::from_rgb(0xff, 0x9f, 0x2e);
pub const GREEN: Color32 = Color32::from_rgb(0x3d, 0xd6, 0x8c);
pub const RED: Color32 = Color32::from_rgb(0xff, 0x5c, 0x6c);
pub const BG: Color32 = Color32::from_rgb(0x08, 0x0b, 0x12);
pub const PANEL: Color32 = Color32::from_rgb(0x0d, 0x12, 0x1c);
pub const CARD: Color32 = Color32::from_rgb(0x13, 0x1a, 0x28);
pub const FIELD: Color32 = Color32::from_rgb(0x1c, 0x25, 0x38);
pub const FIELD_HI: Color32 = Color32::from_rgb(0x26, 0x32, 0x4a);
pub const BORDER: Color32 = Color32::from_rgb(0x24, 0x2e, 0x44);
pub const TEXT: Color32 = Color32::from_rgb(0xe9, 0xed, 0xf5);
pub const MUTED: Color32 = Color32::from_rgb(0x8f, 0x9a, 0xb0);

pub static LOGO_PNG: &[u8] = include_bytes!("../assets/logo.png");
pub static ICON_PNG: &[u8] = include_bytes!("../assets/icon256.png");

pub fn lerp_color(a: Color32, b: Color32, t: f32) -> Color32 {
    let f = |x: u8, y: u8| (x as f32 + (y as f32 - x as f32) * t) as u8;
    Color32::from_rgb(f(a.r(), b.r()), f(a.g(), b.g()), f(a.b(), b.b()))
}

/// Blue -> orange gradient text, like the logo ring.
pub fn gradient_text(text: &str, size: f32) -> egui::text::LayoutJob {
    let mut job = egui::text::LayoutJob::default();
    let n = text.chars().count().max(2) as f32 - 1.0;
    for (i, c) in text.chars().enumerate() {
        job.append(
            &c.to_string(),
            0.0,
            egui::TextFormat {
                font_id: semibold(size),
                color: lerp_color(BLUE, ORANGE, i as f32 / n),
                ..Default::default()
            },
        );
    }
    job
}

pub fn icon_data() -> egui::IconData {
    let im = image::load_from_memory(ICON_PNG).expect("icon").to_rgba8();
    egui::IconData { width: im.width(), height: im.height(), rgba: im.into_raw() }
}

pub fn logo_image() -> egui::ColorImage {
    let im = image::load_from_memory(LOGO_PNG).expect("logo").to_rgba8();
    egui::ColorImage::from_rgba_unmultiplied([im.width() as usize, im.height() as usize], im.as_raw())
}

pub fn semibold(size: f32) -> egui::FontId {
    egui::FontId::new(size, egui::FontFamily::Name("semibold".into()))
}
pub fn medium(size: f32) -> egui::FontId {
    egui::FontId::new(size, egui::FontFamily::Name("medium".into()))
}

fn setup_fonts(ctx: &egui::Context) {
    use std::sync::Arc;
    let mut fonts = egui::FontDefinitions::default();
    let add = |fonts: &mut egui::FontDefinitions, key: &str, bytes: &'static [u8]| {
        fonts.font_data.insert(key.to_string(), Arc::new(egui::FontData::from_static(bytes)));
    };
    add(&mut fonts, "poppins", include_bytes!("../assets/fonts/Poppins-Regular.ttf"));
    add(&mut fonts, "poppins_medium", include_bytes!("../assets/fonts/Poppins-Medium.ttf"));
    add(&mut fonts, "poppins_semibold", include_bytes!("../assets/fonts/Poppins-SemiBold.ttf"));
    fonts.families.entry(egui::FontFamily::Proportional).or_default().insert(0, "poppins".into());
    // the default fonts stay behind Poppins as fallback for symbols
    for (name, key) in [("medium", "poppins_medium"), ("semibold", "poppins_semibold")] {
        let mut fam = vec![key.to_string(), "poppins".to_string()];
        fam.extend(fonts.families.get(&egui::FontFamily::Proportional).cloned().unwrap_or_default().into_iter().filter(|f| f != "poppins"));
        fonts.families.insert(egui::FontFamily::Name(name.into()), fam);
    }
    ctx.set_fonts(fonts);
}

pub fn apply_theme(ctx: &egui::Context) {
    setup_fonts(ctx);
    let mut v = egui::Visuals::dark();
    v.panel_fill = PANEL;
    v.window_fill = CARD;
    v.window_stroke = egui::Stroke::new(1.0_f32, BORDER);
    v.window_corner_radius = egui::CornerRadius::same(14);
    v.extreme_bg_color = Color32::from_rgb(0x0a, 0x0e, 0x16);
    v.faint_bg_color = CARD;
    v.override_text_color = Some(TEXT);
    v.selection.bg_fill = BLUE_DK;
    v.selection.stroke = egui::Stroke::new(1.0_f32, BLUE);
    v.hyperlink_color = ORANGE;
    v.slider_trailing_fill = true;
    let r = egui::CornerRadius::same(8);
    for (w, fill, stroke) in [
        (&mut v.widgets.noninteractive, CARD, BORDER),
        (&mut v.widgets.inactive, FIELD, Color32::from_rgb(0x2a, 0x35, 0x4c)),
        (&mut v.widgets.hovered, FIELD_HI, Color32::from_rgb(0x3b, 0x4b, 0x6b)),
        (&mut v.widgets.active, BLUE_DK, BLUE),
    ] {
        w.bg_fill = fill;
        w.weak_bg_fill = fill;
        w.corner_radius = r;
        w.bg_stroke = egui::Stroke::new(1.0_f32, stroke);
        w.fg_stroke.color = TEXT;
    }
    v.widgets.open.corner_radius = r;
    ctx.set_visuals(v);
    ctx.style_mut(|s| {
        s.spacing.item_spacing = egui::vec2(8.0, 7.0);
        s.spacing.button_padding = egui::vec2(12.0, 6.0);
        s.spacing.interact_size.y = 26.0;
        s.spacing.slider_rail_height = 5.0;
        s.spacing.combo_width = 120.0;
        s.visuals.menu_corner_radius = egui::CornerRadius::same(10);
        use egui::{FontFamily::Proportional, FontId, TextStyle};
        s.text_styles.insert(TextStyle::Body, FontId::new(13.5, Proportional));
        s.text_styles.insert(TextStyle::Button, FontId::new(13.5, egui::FontFamily::Name("medium".into())));
        s.text_styles.insert(TextStyle::Small, FontId::new(11.5, Proportional));
        s.text_styles.insert(TextStyle::Heading, FontId::new(20.0, egui::FontFamily::Name("semibold".into())));
    });
}
