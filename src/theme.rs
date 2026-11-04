//! Look: flat and square, dark or light. Every corner radius is zero and nothing casts a shadow.

use eframe::egui::{self, Color32, ColorImage, CornerRadius, Shadow, Stroke, TextureHandle, TextureOptions};
use std::sync::atomic::{AtomicBool, Ordering};

static DARK: AtomicBool = AtomicBool::new(true);
static HIGH_CONTRAST: AtomicBool = AtomicBool::new(false);

pub fn is_dark() -> bool {
    DARK.load(Ordering::Relaxed)
}

pub fn is_high_contrast() -> bool {
    HIGH_CONTRAST.load(Ordering::Relaxed)
}

/// Picks the dark or the light value.
fn pick(dark: Color32, light: Color32) -> Color32 {
    if is_dark() { dark } else { light }
}

/// Same, with stronger values for high contrast (pure black and white, an explicit opt-in).
fn pick_hc(dark: Color32, light: Color32, dark_hc: Color32, light_hc: Color32) -> Color32 {
    if is_high_contrast() { pick(dark_hc, light_hc) } else { pick(dark, light) }
}

const fn g(v: u8) -> Color32 {
    Color32::from_gray(v)
}

// Dark: a soft near-black (never pure #000) with white text and icons. Light: the reverse.
pub fn bg() -> Color32 {
    pick_hc(g(16), g(242), g(0), g(255))
}
pub fn bg_fullscreen() -> Color32 {
    pick_hc(g(12), g(236), g(0), g(255))
}
pub fn bar() -> Color32 {
    pick_hc(g(21), g(255), g(0), g(255))
}
pub fn bar_overlay() -> Color32 {
    if is_high_contrast() {
        return bar();
    }
    pick(Color32::from_rgba_unmultiplied(21, 21, 21, 235), Color32::from_rgba_unmultiplied(255, 255, 255, 238))
}
pub fn line() -> Color32 {
    pick_hc(g(40), g(218), g(170), g(70))
}
pub fn hover() -> Color32 {
    pick_hc(g(42), g(232), g(60), g(205))
}
pub fn selected() -> Color32 {
    pick_hc(g(58), g(214), g(90), g(170))
}
pub fn text() -> Color32 {
    pick_hc(g(245), g(18), g(255), g(0))
}
pub fn text_dim() -> Color32 {
    pick_hc(g(140), g(105), g(215), g(45))
}
pub fn icon() -> Color32 {
    pick_hc(g(240), g(24), g(255), g(0))
}
pub fn icon_disabled() -> Color32 {
    pick_hc(g(78), g(190), g(120), g(140))
}
/// Placeholder for strip pages that are still decoding.
pub fn placeholder() -> Color32 {
    pick_hc(g(22), g(230), g(25), g(225))
}
pub fn progress() -> Color32 {
    pick_hc(g(120), g(140), g(200), g(60))
}
pub fn error() -> Color32 {
    pick(Color32::from_rgb(235, 120, 110), Color32::from_rgb(190, 40, 30))
}

#[derive(Clone, Copy, PartialEq, Eq, Debug, Default, serde::Serialize, serde::Deserialize)]
pub enum ThemePref {
    #[default]
    System,
    Dark,
    Light,
}

pub fn apply(ctx: &egui::Context, dark: bool, high_contrast: bool) {
    DARK.store(dark, Ordering::Relaxed);
    HIGH_CONTRAST.store(high_contrast, Ordering::Relaxed);
    let stroke = if high_contrast { 2.0 } else { 1.0 };
    // Pinned, so egui doesn't swap in its own style when Windows changes theme; the app
    // follows Windows itself (see `App::dark_mode`).
    ctx.set_theme(if dark { egui::Theme::Dark } else { egui::Theme::Light });
    ctx.set_visuals(if dark { egui::Visuals::dark() } else { egui::Visuals::light() });
    ctx.all_styles_mut(|style| {
        let v = &mut style.visuals;
        v.window_corner_radius = CornerRadius::ZERO;
        v.menu_corner_radius = CornerRadius::ZERO;
        v.window_shadow = Shadow::NONE;
        v.popup_shadow = Shadow::NONE;
        v.window_fill = bar();
        v.panel_fill = bar();
        v.extreme_bg_color = bg();
        v.window_stroke = Stroke::new(stroke, line());
        v.override_text_color = Some(text());
        v.selection.bg_fill = selected();
        v.selection.stroke = Stroke::new(stroke, text());
        for w in [
            &mut v.widgets.noninteractive,
            &mut v.widgets.inactive,
            &mut v.widgets.hovered,
            &mut v.widgets.active,
            &mut v.widgets.open,
        ] {
            w.corner_radius = CornerRadius::ZERO;
            w.expansion = 0.0;
            w.fg_stroke.color = text();
        }
        v.widgets.noninteractive.bg_stroke = Stroke::new(stroke, line());
        // Keyboard focus and hover get a visible outline in high contrast.
        if high_contrast {
            v.widgets.hovered.bg_stroke = Stroke::new(stroke, text());
        }
        v.widgets.inactive.weak_bg_fill = Color32::TRANSPARENT;
        v.widgets.inactive.bg_fill = hover();
        v.widgets.hovered.weak_bg_fill = hover();
        v.widgets.hovered.bg_fill = hover();
        if !high_contrast {
            v.widgets.hovered.bg_stroke = Stroke::NONE;
        }
        v.widgets.active.weak_bg_fill = selected();
        v.widgets.active.bg_fill = selected();
        v.widgets.active.bg_stroke = Stroke::NONE;
        v.widgets.open.weak_bg_fill = selected();
        style.spacing.button_padding = egui::vec2(10.0, 5.0);
        style.spacing.menu_margin = egui::Margin::same(4);
        style.spacing.item_spacing = egui::vec2(6.0, 4.0);
    });
}

/// Rasterizes an SVG to an egui image. Done once at startup at 3x the drawn size, so the
/// icons stay crisp on high-DPI screens; drawing scales them down with mipmaps.
pub fn rasterize_svg(svg: &[u8], size: u32) -> ColorImage {
    let tree = resvg::usvg::Tree::from_data(svg, &resvg::usvg::Options::default()).expect("built-in SVG is valid");
    let mut pixmap = resvg::tiny_skia::Pixmap::new(size, size).expect("non-zero size");
    let scale = size as f32 / tree.size().width().max(tree.size().height());
    resvg::render(&tree, resvg::tiny_skia::Transform::from_scale(scale, scale), &mut pixmap.as_mut());
    ColorImage::from_rgba_premultiplied([size as usize, size as usize], pixmap.data())
}

pub fn app_icon() -> egui::IconData {
    let img = rasterize_svg(include_bytes!("../assets/app_icon.svg"), 256);
    egui::IconData {
        rgba: img.pixels.iter().flat_map(|c| c.to_srgba_unmultiplied()).collect(),
        width: 256,
        height: 256,
    }
}

macro_rules! icons {
    ($($name:ident),* $(,)?) => {
        pub struct Icons { $(pub $name: TextureHandle,)* pub logo: TextureHandle }
        impl Icons {
            pub fn load(ctx: &egui::Context) -> Self {
                let opts = TextureOptions::LINEAR.with_mipmap_mode(Some(egui::TextureFilter::Linear));
                Self { $($name: ctx.load_texture(
                    concat!("icon-", stringify!($name)),
                    rasterize_svg(include_bytes!(concat!("../assets/icons/", stringify!($name), ".svg")), 72),
                    opts,
                ),)*
                logo: ctx.load_texture("logo", rasterize_svg(include_bytes!("../assets/app_icon.svg"), 192), opts) }
            }
        }
    };
}

icons!(
    open,
    prev,
    next,
    fit_page,
    fit_width,
    strip,
    book,
    bookmark,
    bookmark_on,
    enlarge,
    rotate_cw,
    rotate_ccw,
    play,
    pause,
    info,
    fullscreen,
    exit_fullscreen,
    more,
    lock,
    unlock,
    settings,
    thumbs,
    volume,
    mute,
    pin,
    pin_on,
);

#[cfg(test)]
mod tests {
    #[test]
    fn icons_rasterize() {
        for svg in [&include_bytes!("../assets/icons/next.svg")[..], include_bytes!("../assets/app_icon.svg")] {
            let img = super::rasterize_svg(svg, 48);
            assert!(img.pixels.iter().any(|p| p.a() > 0), "icon drew nothing");
        }
    }
}

/// Regenerates `assets/app.ico` (embedded in the exe by build.rs) after editing the SVG:
/// `cargo test write_app_ico -- --ignored`.
#[cfg(test)]
#[test]
#[ignore]
fn write_app_ico() {
    use image::codecs::ico::{IcoEncoder, IcoFrame};
    let frames: Vec<IcoFrame> = [16u32, 20, 24, 32, 40, 48, 64, 128, 256]
        .iter()
        .map(|&s| {
            let img = rasterize_svg(include_bytes!("../assets/app_icon.svg"), s);
            let rgba: Vec<u8> = img.pixels.iter().flat_map(|c| c.to_srgba_unmultiplied()).collect();
            IcoFrame::as_png(&rgba, s, s, image::ExtendedColorType::Rgba8).unwrap()
        })
        .collect();
    let file = std::fs::File::create(concat!(env!("CARGO_MANIFEST_DIR"), "/assets/app.ico")).unwrap();
    IcoEncoder::new(file).encode_images(&frames).unwrap();
}

/// Regenerates the PNG icons the macOS and Linux packages use (`packaging/icons`):
/// `cargo test write_icon_pngs -- --ignored`.
#[cfg(test)]
#[test]
#[ignore]
fn write_icon_pngs() {
    let dir = concat!(env!("CARGO_MANIFEST_DIR"), "/packaging/icons");
    std::fs::create_dir_all(dir).unwrap();
    for s in [16u32, 32, 64, 128, 256, 512, 1024] {
        let img = rasterize_svg(include_bytes!("../assets/app_icon.svg"), s);
        let rgba: Vec<u8> = img.pixels.iter().flat_map(|c| c.to_srgba_unmultiplied()).collect();
        image::RgbaImage::from_raw(s, s, rgba).unwrap().save(format!("{dir}/{s}.png")).unwrap();
    }
}
