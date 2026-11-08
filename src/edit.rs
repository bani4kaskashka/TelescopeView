//! Editing a picture: the dials along the bottom, crop, rotate and flip, saved over the file or
//! as a copy. It's all worked out on the CPU, so it looks the same with either renderer, and the
//! one `render` path makes both the small preview and the saved picture.

use crate::app::{App, FLIP_H, FLIP_V, Pic, PicFrame, Start, compose};
use crate::canvas::turn;
use crate::files;
use crate::keys::Action;
use crate::theme;
use crate::ui::{icon_button, separator};
use eframe::egui::{self, Color32, ColorImage, Key, Pos2, Rect, Sense, Stroke, TextureOptions, Vec2};
use image::{ImageBuffer, Luma, RgbaImage, imageops};
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::mpsc::{Receiver, TryRecvError, channel};
use std::time::Duration;

/// The panel along the bottom: a row of tools, and the dials under it.
pub const PANEL_H: f32 = 124.0;
const DIAL_SIZE: Vec2 = Vec2::new(74.0, 84.0);
const DIAL_RADIUS: f32 = 24.0;
/// The preview's long side, in pixels: small enough to follow a dial as it's dragged.
const PREVIEW_SIDE: u32 = 2048;
const UNIT: Rect = Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0));
/// The smallest crop, as a share of the picture.
const MIN_CROP: f32 = 0.02;
/// Crop handles answer this many points from an edge.
const GRIP: f32 = 12.0;
/// As wide as the row of dials; a narrower window gets a bar from side to side that scrolls.
const BAR_W: f32 = DIALS.len() as f32 * DIAL_SIZE.x + 8.0;
/// How long a dial keeps showing its number after a change.
const NUMBER_FOR: f64 = 1.5;
/// Width / height in pixels. `None` is free; "Original" is worked out from the picture.
const RATIOS: [(&str, f32); 7] = [
    ("1:1", 1.0),
    ("4:3", 4.0 / 3.0),
    ("3:4", 3.0 / 4.0),
    ("3:2", 3.0 / 2.0),
    ("2:3", 2.0 / 3.0),
    ("16:9", 16.0 / 9.0),
    ("9:16", 9.0 / 16.0),
];

/// Every dial, in the order shown: its name and how far it goes each way.
pub const DIALS: [(&str, f32); 12] = [
    ("Light balance", 100.0),
    ("Brightness", 100.0),
    ("Exposure", 100.0),
    ("Contrast", 100.0),
    ("Highlights", 100.0),
    ("Shadows", 100.0),
    ("Saturation", 100.0),
    ("Tint", 100.0),
    ("Temperature", 100.0),
    ("Sharpness", 100.0),
    ("Definition", 100.0),
    ("Straighten", 45.0),
];
const LIGHT: usize = 0;
const BRIGHTNESS: usize = 1;
const EXPOSURE: usize = 2;
const CONTRAST: usize = 3;
const HIGHLIGHTS: usize = 4;
const SHADOWS: usize = 5;
const SATURATION: usize = 6;
const TINT: usize = 7;
const TEMPERATURE: usize = 8;
const SHARPNESS: usize = 9;
const DEFINITION: usize = 10;
const STRAIGHTEN: usize = 11;

/// Everything undo goes back through.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct State {
    pub dials: [f32; DIALS.len()],
    /// An orientation (see `app::compose`).
    pub turns: u8,
    /// What's kept, in 0..1 of the picture after turning and straightening.
    pub crop: Rect,
    /// The crop's width / height, held while its edges are dragged. `None` is free.
    ratio: Option<f32>,
}

impl State {
    fn new(turns: u8) -> Self {
        Self { dials: [0.0; DIALS.len()], turns, crop: UNIT, ratio: None }
    }

    /// What "before" looks like when comparing: same framing, no color changes.
    fn without_dials(mut self) -> Self {
        let angle = self.dials[STRAIGHTEN];
        self.dials = [0.0; DIALS.len()];
        self.dials[STRAIGHTEN] = angle;
        self
    }

    fn turned(mut self, by: u8) -> Self {
        self.turns = compose(self.turns, by);
        self.crop = Rect::from_two_pos(turn(self.crop.min, by), turn(self.crop.max, by));
        // Mirrored, a clockwise tilt comes out the other way.
        if by & 4 != 0 {
            self.dials[STRAIGHTEN] = -self.dials[STRAIGHTEN];
        }
        if by & 1 == 1 {
            self.ratio = self.ratio.map(|r| 1.0 / r);
        }
        self
    }
}

type Gray = ImageBuffer<Luma<u16>, Vec<u16>>;
/// What a preview shows: the dials and the orientation.
type Look = ([f32; DIALS.len()], u8);

/// The picture turned and straightened, and blurred copies of its brightness. Kept between
/// previews, since most changes are to a dial that needs none of this redone.
struct Base {
    turns: u8,
    angle: f32,
    img: RgbaImage,
    fine: Option<Gray>,
    coarse: Option<Gray>,
}

/// The whole edit at the picture's own size.
fn render(src: &RgbaImage, s: &State) -> RgbaImage {
    let mut out = adjust(&prepare(src, s, None), s);
    let (x, y, w, h) = crop_pixels(s.crop, out.width(), out.height());
    imageops::crop(&mut out, x, y, w, h).to_image()
}

fn prepare(src: &RgbaImage, s: &State, last: Option<Arc<Base>>) -> Arc<Base> {
    let angle = s.dials[STRAIGHTEN];
    let last = last.filter(|b| b.turns == s.turns && b.angle == angle);
    let want_fine = s.dials[SHARPNESS] != 0.0;
    let want_coarse = s.dials[DEFINITION] != 0.0 || s.dials[LIGHT] != 0.0;
    if let Some(b) = &last
        && (b.fine.is_some() || !want_fine)
        && (b.coarse.is_some() || !want_coarse)
    {
        return b.clone();
    }
    let (img, fine, coarse) = match last {
        Some(b) => (b.img.clone(), b.fine.clone(), b.coarse.clone()),
        None => (straightened(&oriented(src, s.turns), angle), None, None),
    };
    // Sized to the picture, so the preview and the saved file look alike.
    let side = img.width().max(img.height()) as f32;
    let blurred = |sigma: f32| imageops::fast_blur(&gray(&img), sigma);
    let fine = fine.or_else(|| want_fine.then(|| blurred((side / 1000.0).max(0.8))));
    let coarse = coarse.or_else(|| want_coarse.then(|| blurred(side / 25.0)));
    Arc::new(Base { turns: s.turns, angle, img, fine, coarse })
}

pub fn oriented(src: &RgbaImage, turns: u8) -> RgbaImage {
    let flipped;
    let img = if turns & 4 != 0 {
        flipped = imageops::flip_horizontal(src);
        &flipped
    } else {
        src
    };
    match turns & 3 {
        1 => imageops::rotate90(img),
        2 => imageops::rotate180(img),
        3 => imageops::rotate270(img),
        _ => img.clone(),
    }
}

/// Tilted clockwise by `degrees`, and zoomed in just enough to leave no empty corners.
fn straightened(img: &RgbaImage, degrees: f32) -> RgbaImage {
    let mut out = img.clone();
    if degrees == 0.0 {
        return out;
    }
    let (w, h) = (img.width() as f32, img.height() as f32);
    let (sin, cos) = degrees.to_radians().sin_cos();
    let zoom = ((w * cos.abs() + h * sin.abs()) / w).max((w * sin.abs() + h * cos.abs()) / h);
    for_rows(&mut out, |y, row| {
        let dy = y as f32 + 0.5 - h / 2.0;
        for (x, px) in row.as_chunks_mut::<4>().0.iter_mut().enumerate() {
            let dx = x as f32 + 0.5 - w / 2.0;
            let sx = (w / 2.0 + (dx * cos + dy * sin) / zoom - 0.5).clamp(0.0, w - 1.0);
            let sy = (h / 2.0 + (dy * cos - dx * sin) / zoom - 0.5).clamp(0.0, h - 1.0);
            if let Some(p) = imageops::interpolate_bilinear(img, sx, sy) {
                px.copy_from_slice(&p.0);
            }
        }
    });
    out
}

fn gray(img: &RgbaImage) -> Gray {
    let px = img.pixels().map(|p| (luma([p[0] as f32, p[1] as f32, p[2] as f32]) * 257.0) as u16).collect();
    ImageBuffer::from_raw(img.width(), img.height(), px).expect("one value per pixel")
}

fn luma([r, g, b]: [f32; 3]) -> f32 {
    0.2126 * r + 0.7152 * g + 0.0722 * b
}

fn smoothstep(from: f32, to: f32, x: f32) -> f32 {
    let t = ((x - from) / (to - from)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

fn to_linear(v: f32) -> f32 {
    if v <= 0.04045 { v / 12.92 } else { ((v + 0.055) / 1.055).powf(2.4) }
}

fn from_linear(v: f32) -> f32 {
    if v <= 0.0031308 { v * 12.92 } else { 1.055 * v.powf(1.0 / 2.4) - 0.055 }
}

/// Runs `f` on each row of `img`, on all the processor's cores.
fn for_rows(img: &mut RgbaImage, f: impl Fn(usize, &mut [u8]) + Sync) {
    let stride = img.width() as usize * 4;
    let threads = std::thread::available_parallelism().map_or(1, |n| n.get());
    let per_thread = (img.height() as usize).div_ceil(threads).max(1);
    let f = &f;
    std::thread::scope(|scope| {
        for (t, rows) in img.chunks_mut((stride * per_thread).max(1)).enumerate() {
            scope.spawn(move || {
                for (i, row) in rows.chunks_exact_mut(stride).enumerate() {
                    f(t * per_thread + i, row);
                }
            });
        }
    });
}

/// The dials, on each pixel. Exposure and white balance work on light itself, the rest on
/// how bright it looks.
fn adjust(base: &Base, s: &State) -> RgbaImage {
    let d = |i: usize| s.dials[i] / 100.0;
    let (light, contrast, highlights, shadows) = (d(LIGHT), d(CONTRAST), d(HIGHLIGHTS), d(SHADOWS));
    let (saturation, sharpness, definition) = (d(SATURATION), d(SHARPNESS), d(DEFINITION));
    let exposure = 2f32.powf(d(EXPOSURE) * 2.0);
    let (warm, tint) = (d(TEMPERATURE) * 0.3, d(TINT) * 0.2);
    let gains = [exposure * (1.0 + warm), exposure * (1.0 - tint), exposure * (1.0 - warm)];
    let in_light = gains != [1.0; 3];
    // Lifts or lowers the middle and leaves black and white where they are.
    let gamma = 2f32.powf(-d(BRIGHTNESS));
    let slope = if contrast > 0.0 { 1.0 + contrast } else { 1.0 + contrast * 0.7 };
    // Softening past a full blur of the fine detail makes no sense, sharpening can go further.
    let sharpen = sharpness * if sharpness > 0.0 { 2.0 } else { 1.0 };
    let linear: Vec<f32> = (0..256).map(|v| to_linear(v as f32 / 255.0)).collect();
    let width = base.img.width() as usize;

    let mut out = base.img.clone();
    for_rows(&mut out, |y, row| {
        let blur_at =
            |b: &Option<Gray>, x: usize| b.as_ref().map_or(0.0, |g| g.as_raw()[y * width + x] as f32 / 65535.0);
        for (x, px) in row.as_chunks_mut::<4>().0.iter_mut().enumerate() {
            let original = [px[0] as f32 / 255.0, px[1] as f32 / 255.0, px[2] as f32 / 255.0];
            let mut c = original;
            if in_light {
                for i in 0..3 {
                    c[i] = from_linear((linear[px[i] as usize] * gains[i]).min(1.0));
                }
            }
            if gamma != 1.0 {
                c = c.map(|v| v.max(0.0).powf(gamma));
            }
            if slope != 1.0 {
                c = c.map(|v| 0.5 + (v - 0.5) * slope);
            }
            let y_now = luma(c);
            let mut add =
                highlights * 0.4 * smoothstep(0.4, 1.0, y_now) + shadows * 0.4 * (1.0 - smoothstep(0.0, 0.6, y_now));
            let y_before = luma(original);
            // Light balance evens out the picture's bright and dark areas, not single pixels.
            if light != 0.0 {
                add += light * 0.5 * (0.5 - blur_at(&base.coarse, x));
            }
            if definition != 0.0 {
                add += definition * (y_before - blur_at(&base.coarse, x));
            }
            if sharpen != 0.0 {
                add += sharpen * (y_before - blur_at(&base.fine, x));
            }
            c = c.map(|v| v + add);
            if saturation != 0.0 {
                let y_then = luma(c);
                c = c.map(|v| y_then + (v - y_then) * (1.0 + saturation));
            }
            for i in 0..3 {
                px[i] = (c[i].clamp(0.0, 1.0) * 255.0).round() as u8;
            }
        }
    });
    out
}

/// A crop in 0..1 as pixels: at least one, and inside the picture.
fn crop_pixels(crop: Rect, w: u32, h: u32) -> (u32, u32, u32, u32) {
    let (x, y) = ((crop.min.x * w as f32).round() as u32, (crop.min.y * h as f32).round() as u32);
    let (x, y) = (x.min(w - 1), y.min(h - 1));
    let cw = ((crop.width() * w as f32).round() as u32).clamp(1, w - x);
    let ch = ((crop.height() * h as f32).round() as u32).clamp(1, h - y);
    (x, y, cw, ch)
}

/// A crop's edges being dragged by `d`: left, top, right, bottom. All four moves it whole.
/// `aspect` is width / height in 0..1 units, to hold a ratio.
fn drag_crop(crop: Rect, grip: [bool; 4], d: Vec2, aspect: Option<f32>) -> Rect {
    let [left, top, right, bottom] = grip;
    if grip == [true; 4] {
        let d = d.clamp(-crop.min.to_vec2(), Pos2::new(1.0, 1.0) - crop.max);
        return crop.translate(d);
    }
    let mut r = crop;
    if left {
        r.min.x = (r.min.x + d.x).clamp(0.0, r.max.x - MIN_CROP);
    }
    if right {
        r.max.x = (r.max.x + d.x).clamp(r.min.x + MIN_CROP, 1.0);
    }
    if top {
        r.min.y = (r.min.y + d.y).clamp(0.0, r.max.y - MIN_CROP);
    }
    if bottom {
        r.max.y = (r.max.y + d.y).clamp(r.min.y + MIN_CROP, 1.0);
    }
    let Some(a) = aspect else { return r };
    // The side being dragged sets the size, growing away from the opposite edge (from the
    // middle along a side), as far as there's room.
    let (w, h) = if left || right { (r.width(), r.width() / a) } else { (r.height() * a, r.height()) };
    let c = crop.center();
    let room_w = if left {
        crop.max.x
    } else if right {
        1.0 - crop.min.x
    } else {
        2.0 * c.x.min(1.0 - c.x)
    };
    let room_h = if top {
        crop.max.y
    } else if bottom {
        1.0 - crop.min.y
    } else {
        2.0 * c.y.min(1.0 - c.y)
    };
    let fit = (room_w / w).min(room_h / h).min(1.0);
    let (w, h) = (w * fit, h * fit);
    let x = if left {
        crop.max.x - w
    } else if right {
        crop.min.x
    } else {
        c.x - w / 2.0
    };
    let y = if top {
        crop.max.y - h
    } else if bottom {
        crop.min.y
    } else {
        c.y - h / 2.0
    };
    Rect::from_min_size(Pos2::new(x, y), Vec2::new(w, h))
}

/// The biggest crop of a ratio, in the middle.
fn centered_crop(aspect: f32) -> Rect {
    let size = if aspect >= 1.0 { Vec2::new(1.0, 1.0 / aspect) } else { Vec2::new(aspect, 1.0) };
    Rect::from_center_size(Pos2::new(0.5, 0.5), size)
}

/// What a press at `p` takes hold of: the same order as `drag_crop`, or None outside the crop.
fn grip_at(crop: Rect, p: Pos2) -> Option<[bool; 4]> {
    if !crop.expand(GRIP).contains(p) {
        return None;
    }
    let near = |a: f32, b: f32| (a - b).abs() <= GRIP;
    let grip = [near(p.x, crop.left()), near(p.y, crop.top()), near(p.x, crop.right()), near(p.y, crop.bottom())];
    Some(if grip == [false; 4] { [true; 4] } else { grip })
}

struct Loaded {
    full: Arc<RgbaImage>,
    small: Arc<RgbaImage>,
    has_alpha: bool,
    format: String,
    /// Too big to edit at its own size (see `PIXEL_BUDGET`), so never saved over the file.
    shrunk: bool,
}

/// What was asked for while the editor couldn't be left yet.
pub enum Then {
    Open(PathBuf, Start),
    Close,
}

pub struct Edit {
    pub path: PathBuf,
    pub state: State,
    first: State,
    /// The state undo goes back to: changes count once the mouse is let go.
    settled: State,
    undo: Vec<State>,
    redo: Vec<State>,
    pub cropping: bool,
    /// "Compare" is on: without the dials left of the line, with them to its right.
    pub comparing: bool,
    /// Where the line is across the picture, 0..1.
    split: f32,
    /// Asking whether to throw the changes away.
    pub confirm_leave: bool,
    /// Runs once the editor is left: after throwing away, or after the save in progress.
    then: Option<Then>,
    grip: Option<[bool; 4]>,
    loading: Option<Receiver<Result<Loaded, String>>>,
    loaded: Option<Loaded>,
    base: Option<Arc<Base>>,
    tex: Option<egui::TextureHandle>,
    on_texture: Option<Look>,
    job: Option<Receiver<(Look, Arc<Base>, ColorImage)>>,
    before_tex: Option<egui::TextureHandle>,
    before_on: Option<Look>,
    before_job: Option<Receiver<(Look, Arc<Base>, ColorImage)>>,
    saving: Option<Receiver<Result<PathBuf, String>>>,
}

impl Edit {
    fn new(ctx: &egui::Context, path: PathBuf, turns: u8) -> Self {
        let (tx, rx) = channel();
        let (file, ctx) = (path.clone(), ctx.clone());
        std::thread::spawn(move || {
            let _ = tx.send(load(&file));
            ctx.request_repaint();
        });
        let state = State::new(turns);
        Self {
            path,
            state,
            first: state,
            settled: state,
            undo: Vec::new(),
            redo: Vec::new(),
            cropping: false,
            comparing: false,
            split: 0.5,
            confirm_leave: false,
            then: None,
            grip: None,
            loading: Some(rx),
            loaded: None,
            base: None,
            tex: None,
            on_texture: None,
            job: None,
            before_tex: None,
            before_on: None,
            before_job: None,
            saving: None,
        }
    }

    /// The preview, as something `page_view` draws like any page: `None` until there is one.
    /// A crop shows by sliding the picture so the crop fills the page, clipped to it.
    pub fn pic(&self) -> Option<Arc<Pic>> {
        self.pic_of(self.tex.as_ref()?)
    }

    /// The same picture without the dials, while comparing.
    pub fn before_pic(&self) -> Option<Arc<Pic>> {
        if !self.comparing || self.before_on?.1 != self.on_texture?.1 {
            return None;
        }
        self.pic_of(self.before_tex.as_ref()?)
    }

    fn pic_of(&self, tex: &egui::TextureHandle) -> Option<Arc<Pic>> {
        let (loaded, (_, turns)) = (self.loaded.as_ref()?, self.on_texture?);
        let [w, h] = turned_size(&loaded.full, turns);
        // A turn still on its way to the texture: the crop is for the turned picture.
        let c = if self.cropping || turns != self.state.turns { UNIT } else { self.state.crop };
        let tile = Rect::from_min_max(
            Pos2::new(-c.min.x / c.width(), -c.min.y / c.height()),
            Pos2::new((1.0 - c.min.x) / c.width(), (1.0 - c.min.y) / c.height()),
        );
        Some(Arc::new(Pic {
            frames: vec![PicFrame { tiles: vec![(tex.clone(), tile)], delay: Duration::ZERO }],
            original_size: [(w as f32 * c.width()).max(1.0) as u32, (h as f32 * c.height()).max(1.0) as u32],
            format: loaded.format.clone(),
            has_alpha: loaded.has_alpha,
            shade: Vec::new(),
        }))
    }

    /// Where the line is on the picture drawn at `page`.
    pub fn line_x(&self, page: Rect) -> f32 {
        page.left() + page.width() * self.split
    }

    /// The line across the picture while comparing: drag anywhere to move it.
    pub fn compare_ui(&mut self, resp: &egui::Response, page: Rect, painter: &egui::Painter, ctx: &egui::Context) {
        if (resp.clicked() || resp.is_pointer_button_down_on() && ctx.input(|i| i.pointer.primary_down()))
            && let Some(p) = resp.interact_pointer_pos()
        {
            self.split = ((p.x - page.left()) / page.width().max(1.0)).clamp(0.0, 1.0);
        }
        if resp.hovered() {
            ctx.set_cursor_icon(egui::CursorIcon::ResizeHorizontal);
        }
        let seen = page.intersect(painter.clip_rect());
        let x = self.line_x(page);
        // Dark edges keep it visible on a white picture.
        let line = Rect::from_x_y_ranges(x - 2.0..=x + 2.0, seen.y_range());
        painter.rect_filled(line, 0.0, Color32::from_black_alpha(120));
        painter.vline(x, seen.y_range(), Stroke::new(2.0, Color32::WHITE));
        let center = Pos2::new(x, seen.center().y);
        painter.rect_filled(Rect::from_center_size(center, Vec2::new(22.0, 30.0)), 0.0, Color32::WHITE);
        for side in [-1.0, 1.0] {
            let tip = center + Vec2::new(side * 8.0, 0.0);
            let back = center + Vec2::new(side * 3.0, 0.0);
            let points = vec![tip, back + Vec2::new(0.0, -5.0), back + Vec2::new(0.0, 5.0)];
            painter.add(egui::Shape::convex_polygon(points, Color32::BLACK, Stroke::NONE));
        }
        let tag = |text: &str, x: f32, align: f32| {
            let font = egui::FontId::proportional(12.0);
            let galley = painter.layout_no_wrap(text.to_owned(), font, Color32::WHITE);
            let at = Pos2::new(x - galley.size().x * align, seen.top() + 8.0);
            let plate = Rect::from_min_size(at, galley.size()).expand2(Vec2::new(6.0, 3.0));
            painter.rect_filled(plate, 0.0, Color32::from_black_alpha(160));
            painter.galley(at, galley, Color32::WHITE);
        };
        tag("Before", seen.left() + 14.0, 0.0);
        tag("After", seen.right() - 14.0, 1.0);
    }

    /// Whether leaving now would lose the changes or cut a save short.
    pub fn holds_leaving(&self) -> bool {
        self.saving.is_some() || self.state != self.first
    }

    /// Asks about the changes, or waits for the save, and then does `then`.
    pub fn leave_then(&mut self, then: Then) {
        self.confirm_leave = self.saving.is_none();
        self.then = Some(then);
    }

    pub fn keep_editing(&mut self) {
        self.confirm_leave = false;
        self.then = None;
    }

    fn turn(&mut self, by: u8) {
        self.state = self.state.turned(by);
    }

    fn undo(&mut self) {
        if let Some(s) = self.undo.pop() {
            self.redo.push(self.state);
            self.state = s;
            self.settled = s;
        }
    }

    fn redo(&mut self) {
        if let Some(s) = self.redo.pop() {
            self.undo.push(self.state);
            self.state = s;
            self.settled = s;
        }
    }

    /// Width / height of the crop's ratio in 0..1 units.
    fn aspect(&self) -> Option<f32> {
        let ratio = self.state.ratio?;
        let [w, h] = turned_size(&self.loaded.as_ref()?.full, self.state.turns);
        Some(ratio * h as f32 / w as f32)
    }

    fn set_ratio(&mut self, ratio: Option<f32>) {
        self.state.ratio = ratio;
        if let Some(a) = self.aspect() {
            self.state.crop = centered_crop(a);
        }
    }

    /// The crop box on the page: `page` is the whole picture on screen.
    pub fn crop_ui(&mut self, resp: &egui::Response, page: Rect, painter: &egui::Painter, ctx: &egui::Context) {
        let on_screen = Rect::from_min_max(
            page.min + self.state.crop.min.to_vec2() * page.size(),
            page.min + self.state.crop.max.to_vec2() * page.size(),
        );
        if resp.drag_started_by(egui::PointerButton::Primary) {
            self.grip = ctx.input(|i| i.pointer.press_origin()).and_then(|p| grip_at(on_screen, p));
        }
        if !resp.dragged_by(egui::PointerButton::Primary) {
            self.grip = None;
        }
        if let Some(grip) = self.grip {
            let d = resp.drag_delta() / page.size();
            self.state.crop = drag_crop(self.state.crop, grip, d, self.aspect());
        }
        if let Some(grip) = self.grip.or_else(|| resp.hover_pos().and_then(|p| grip_at(on_screen, p))) {
            use egui::CursorIcon::*;
            ctx.set_cursor_icon(match grip {
                [true, true, true, true] => Move,
                [true, true, false, false] | [false, false, true, true] => ResizeNwSe,
                [true, false, false, true] | [false, true, true, false] => ResizeNeSw,
                [true, false, false, false] | [false, false, true, false] => ResizeHorizontal,
                _ => ResizeVertical,
            });
        }

        // Dark outside the crop, thirds and handles inside it.
        let r = Rect::from_min_max(
            page.min + self.state.crop.min.to_vec2() * page.size(),
            page.min + self.state.crop.max.to_vec2() * page.size(),
        );
        let shade = Color32::from_black_alpha(150);
        painter.rect_filled(Rect::from_min_max(page.min, Pos2::new(page.right(), r.top())), 0.0, shade);
        painter.rect_filled(Rect::from_min_max(Pos2::new(page.left(), r.bottom()), page.max), 0.0, shade);
        painter.rect_filled(Rect::from_min_max(Pos2::new(page.left(), r.top()), r.left_bottom()), 0.0, shade);
        painter.rect_filled(Rect::from_min_max(r.right_top(), Pos2::new(page.right(), r.bottom())), 0.0, shade);
        let thirds = Stroke::new(1.0, Color32::from_white_alpha(70));
        for k in [1.0 / 3.0, 2.0 / 3.0] {
            painter.vline(r.left() + r.width() * k, r.y_range(), thirds);
            painter.hline(r.x_range(), r.top() + r.height() * k, thirds);
        }
        painter.rect_stroke(r, 0.0, Stroke::new(1.0, Color32::WHITE), egui::StrokeKind::Outside);
        for x in [r.left(), r.center().x, r.right()] {
            for y in [r.top(), r.center().y, r.bottom()] {
                if Pos2::new(x, y) != r.center() {
                    painter.rect_filled(Rect::from_center_size(Pos2::new(x, y), Vec2::splat(8.0)), 0.0, Color32::WHITE);
                }
            }
        }
    }
}

fn turned_size(img: &RgbaImage, turns: u8) -> [u32; 2] {
    if turns % 2 == 1 { [img.height(), img.width()] } else { [img.width(), img.height()] }
}

/// A file's picture as plain pixels, with what the decoder says about it.
pub fn read_pixels(path: &std::path::Path) -> Result<(RgbaImage, crate::loader::Image), String> {
    let img = crate::loader::decode(path, u32::MAX)?;
    let tile = &img.frames.first().and_then(|f| f.tiles.first()).ok_or("no picture in it")?.pixels;
    let [w, h] = tile.size;
    let rgba = tile.pixels.iter().flat_map(|c| c.to_srgba_unmultiplied()).collect();
    let full = RgbaImage::from_raw(w as u32, h as u32, rgba).ok_or("odd picture size")?;
    Ok((full, img))
}

fn load(path: &std::path::Path) -> Result<Loaded, String> {
    let (full, img) = read_pixels(path)?;
    let [w, h] = [full.width() as usize, full.height() as usize];
    let side = full.width().max(full.height());
    let small = if side > PREVIEW_SIDE {
        let s = PREVIEW_SIDE as f32 / side as f32;
        let size = |v: u32| ((v as f32 * s) as u32).max(1);
        imageops::thumbnail(&full, size(full.width()), size(full.height()))
    } else {
        full.clone()
    };
    // `decode` leaves this for the loader's threads to work out, so it's looked up here.
    let has_alpha = full.pixels().any(|p| p[3] < 255);
    Ok(Loaded {
        shrunk: img.original_size != [w as u32, h as u32],
        full: Arc::new(full),
        small: Arc::new(small),
        has_alpha,
        format: img.format,
    })
}

impl App {
    /// Whether page `i` can be edited: a still picture that's been decoded.
    pub fn can_edit(&self, i: usize) -> bool {
        self.files.get(i).is_some_and(|p| !crate::video::is_video(p))
            && self.ready(i).is_some_and(|p| p.frames.len() == 1)
    }

    pub fn start_edit(&mut self, ctx: &egui::Context, i: usize) {
        if !self.can_edit(i) {
            return;
        }
        if i != self.index {
            self.go_to(ctx, i, i > self.index, false);
        }
        if self.slideshow.is_some() {
            self.toggle_slideshow();
        }
        // Selecting text would keep the left button from panning the preview.
        self.text = None;
        self.edit = Some(Edit::new(ctx, self.files[i].clone(), self.current_rotation()));
        self.zoom = None;
        self.pan = Vec2::ZERO;
    }

    /// Asks first if there's something to lose.
    fn leave_edit(&mut self, ctx: &egui::Context) {
        let Some(e) = &self.edit else { return };
        if e.saving.is_some() {
            return;
        }
        if e.state != e.first {
            if let Some(e) = &mut self.edit {
                e.confirm_leave = true;
            }
            return;
        }
        self.discard_edit(ctx);
    }

    pub fn discard_edit(&mut self, ctx: &egui::Context) {
        let then = self.edit.take().and_then(|e| e.then);
        self.zoom = None;
        self.pan = Vec2::ZERO;
        self.carry_on(ctx, then);
    }

    fn carry_on(&mut self, ctx: &egui::Context, then: Option<Then>) {
        match then {
            Some(Then::Open(dir, start)) => {
                self.open_folder(ctx, dir, start);
            }
            Some(Then::Close) => ctx.send_viewport_cmd(egui::ViewportCommand::Close),
            None => {}
        }
    }

    /// Whether `Save` can write over the file: a real one, in a format this app also writes.
    fn can_replace(&self) -> bool {
        let Some(e) = &self.edit else { return false };
        e.loaded.as_ref().is_some_and(|l| !l.shrunk)
            && self.current_file().is_some()
            && crate::actions::can_write(&e.path)
    }

    /// `replace` writes over the file, the original going to the Recycle Bin; otherwise it asks
    /// where to put a copy. Done on its own thread; `pump_edit` picks up the result.
    fn save_edit(&mut self, ctx: &egui::Context, replace: bool) {
        if replace && !self.can_replace() {
            return;
        }
        let start_in = self.dialog_dir();
        let Some(e) = &mut self.edit else { return };
        let Some(full) = e.loaded.as_ref().map(|l| l.full.clone()) else { return };
        if e.saving.is_some() {
            return;
        }
        let target = if replace {
            e.path.clone()
        } else {
            let stem = e.path.file_stem().map_or("picture".into(), |s| s.to_string_lossy());
            let ext = if crate::actions::can_write(&e.path) { files::ext_lower(&e.path) } else { "png".to_owned() };
            let mut dialog = rfd::FileDialog::new()
                .set_title("Save a copy")
                .add_filter("Pictures", &["png", "jpg", "jpeg", "webp", "bmp", "tif", "tiff"])
                .set_file_name(format!("{stem} edited.{ext}"));
            if let Some(dir) = start_in {
                dialog = dialog.set_directory(dir);
            }
            let Some(target) = dialog.save_file() else { return };
            target
        };
        let (source, state, (tx, rx), ctx) = (e.path.clone(), e.state, channel(), ctx.clone());
        e.saving = Some(rx);
        std::thread::spawn(move || {
            let out = render(&full, &state);
            let _ = tx.send(crate::actions::write_edited(out, &source, &target).map(|()| target));
            ctx.request_repaint();
        });
    }

    /// Once a frame: takes what the threads finished and starts the next preview.
    pub fn pump_edit(&mut self, ctx: &egui::Context) {
        let Some(e) = &mut self.edit else { return };
        if !ctx.input(|i| i.pointer.any_down()) && e.state != e.settled {
            e.undo.push(e.settled);
            e.redo.clear();
            e.settled = e.state;
        }
        let mut failed = None;
        if let Some(rx) = &e.loading {
            match rx.try_recv() {
                Ok(Ok(loaded)) => {
                    e.loaded = Some(loaded);
                    e.loading = None;
                }
                Ok(Err(msg)) => failed = Some(msg),
                Err(TryRecvError::Disconnected) => failed = Some("it crashed".to_owned()),
                Err(TryRecvError::Empty) => {}
            }
        }
        if let Some(rx) = &e.job {
            match rx.try_recv() {
                Ok((key, base, image)) => {
                    let opts = TextureOptions::LINEAR.with_mipmap_mode(Some(egui::TextureFilter::Linear));
                    match &mut e.tex {
                        Some(tex) => tex.set(image, opts),
                        None => e.tex = Some(ctx.load_texture("edit", image, opts)),
                    }
                    e.on_texture = Some(key);
                    e.base = Some(base);
                    e.job = None;
                }
                Err(TryRecvError::Disconnected) => e.job = None,
                Err(TryRecvError::Empty) => {}
            }
        }
        if let Some(rx) = &e.before_job {
            match rx.try_recv() {
                Ok((key, _, image)) => {
                    let opts = TextureOptions::LINEAR.with_mipmap_mode(Some(egui::TextureFilter::Linear));
                    match &mut e.before_tex {
                        Some(tex) => tex.set(image, opts),
                        None => e.before_tex = Some(ctx.load_texture("edit-before", image, opts)),
                    }
                    e.before_on = Some(key);
                    e.before_job = None;
                }
                Err(TryRecvError::Disconnected) => e.before_job = None,
                Err(TryRecvError::Empty) => {}
            }
        }
        if let Some(l) = &e.loaded {
            if e.job.is_none() && e.on_texture != Some((e.state.dials, e.state.turns)) {
                e.job = Some(start_preview(l.small.clone(), e.state, e.base.clone(), ctx));
            }
            let before = e.state.without_dials();
            if e.comparing && e.before_job.is_none() && e.before_on != Some((before.dials, before.turns)) {
                e.before_job = Some(start_preview(l.small.clone(), before, e.base.clone(), ctx));
            }
        }
        let saved = match e.saving.as_ref().map(Receiver::try_recv) {
            Some(Ok(r)) => Some(r),
            Some(Err(TryRecvError::Disconnected)) => Some(Err("it crashed".to_owned())),
            _ => None,
        };
        match saved {
            Some(Ok(path)) => {
                let then = self.edit.take().and_then(|e| e.then);
                self.thumbs.remove(&path);
                // The turn is in the pixels now; the old picture stays up, still turned on
                // screen, until the new one lands (see `save_rotation`).
                if self.rotation.contains_key(&path) {
                    self.unrotate.push(path.clone());
                }
                self.toast(format!("Saved {}", files::file_name(&path)));
                // Shows the result, unless it went somewhere else (or this is an archive's page).
                if then.is_some() {
                    self.carry_on(ctx, then);
                } else if path.parent() == self.dir.as_deref() {
                    self.open(ctx, path);
                }
            }
            Some(Err(msg)) => {
                e.saving = None;
                e.then = None;
                self.toast(format!("Couldn't save it: {msg}"));
            }
            None => {}
        }
        if let Some(msg) = failed {
            self.edit = None;
            self.toast(format!("Can't edit this picture: {msg}"));
        }
    }

    pub fn edit_keys(&mut self, ctx: &egui::Context) {
        let (esc, enter, undo, redo) = ctx.input(|i| {
            let ctrl = i.modifiers.command;
            (
                i.key_pressed(Key::Escape),
                i.key_pressed(Key::Enter),
                ctrl && !i.modifiers.shift && i.key_pressed(Key::Z),
                ctrl && (i.key_pressed(Key::Y) || i.modifiers.shift && i.key_pressed(Key::Z)),
            )
        });
        let save = ctx.input(|i| self.keymap.presses(i))[Action::SaveTurn as usize] > 0;
        if let Some(e) = &mut self.edit
            && e.confirm_leave
        {
            if esc {
                e.keep_editing();
            } else if enter {
                self.discard_edit(ctx);
            }
            return;
        }
        let cropping = self.edit.as_ref().is_some_and(|e| e.cropping);
        if let Some(e) = &mut self.edit {
            if undo {
                e.undo();
            }
            if redo {
                e.redo();
            }
        }
        if (esc || enter) && cropping {
            self.toggle_crop();
        } else if esc {
            self.leave_edit(ctx);
        }
        if save {
            self.save_edit(ctx, true);
        }
    }

    fn toggle_crop(&mut self) {
        if let Some(e) = &mut self.edit {
            e.cropping = !e.cropping;
            e.comparing = false;
            e.grip = None;
            self.zoom = None;
            self.pan = Vec2::ZERO;
        }
    }

    pub fn edit_panel(&mut self, ui: &mut egui::Ui) {
        let ctx = ui.ctx().clone();
        let can_replace = self.can_replace();
        let icons = [
            self.icons.crop.clone(),
            self.icons.rotate_ccw.clone(),
            self.icons.rotate_cw.clone(),
            self.icons.flip_h.clone(),
            self.icons.flip_v.clone(),
            self.icons.undo.clone(),
            self.icons.redo.clone(),
        ];
        let [crop_icon, left_icon, right_icon, flip_h_icon, flip_v_icon, undo_icon, redo_icon] = &icons;
        // In the order of `DIALS`.
        let dial_icons = [
            self.icons.dial_light.clone(),
            self.icons.dial_brightness.clone(),
            self.icons.dial_exposure.clone(),
            self.icons.dial_contrast.clone(),
            self.icons.dial_highlights.clone(),
            self.icons.dial_shadows.clone(),
            self.icons.dial_saturation.clone(),
            self.icons.dial_tint.clone(),
            self.icons.dial_temperature.clone(),
            self.icons.dial_sharpness.clone(),
            self.icons.dial_definition.clone(),
            self.icons.dial_straighten.clone(),
        ];
        let Some(e) = &mut self.edit else { return };
        let ready = e.loaded.is_some() && e.saving.is_none();
        let saving = e.saving.is_some();
        let (mut toggle_crop, mut leave, mut save, mut copy) = (false, false, false, false);

        // A bar just wide enough for the dials, in the middle; the sides stay background.
        let area = ui.max_rect();
        let bar = Rect::from_center_size(area.center(), Vec2::new(area.width().min(BAR_W), area.height()));
        ui.painter().rect_filled(bar, 0.0, theme::bar());
        if bar.width() < area.width() {
            ui.painter().rect_stroke(bar, 0.0, Stroke::new(1.0, theme::line()), egui::StrokeKind::Inside);
        }
        let inside = egui::UiBuilder::new().max_rect(bar.shrink2(Vec2::new(4.0, 1.0)));
        let ui = &mut ui.new_child(inside);
        ui.spacing_mut().item_spacing.x = 0.0;
        let row = Vec2::new(ui.available_width(), 34.0);
        ui.allocate_ui_with_layout(row, egui::Layout::left_to_right(egui::Align::Center), |ui| {
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.spacing_mut().item_spacing.x = 4.0;
                let tip = if can_replace {
                    "Save over the file (Ctrl+S). The original goes to the Recycle Bin."
                } else {
                    "This one can only be saved as a copy"
                };
                save = ui
                    .add_enabled(ready && can_replace, egui::Button::new("Save"))
                    .on_disabled_hover_text(tip)
                    .on_hover_text(tip)
                    .clicked();
                copy = ui.add_enabled(ready, egui::Button::new("Save a copy…")).clicked();
                leave = ui.add_enabled(!saving, egui::Button::new("Cancel")).on_hover_text("Esc").clicked();
                if saving {
                    ui.label(egui::RichText::new("Saving…").color(theme::text_dim()));
                }
                ui.with_layout(egui::Layout::left_to_right(egui::Align::Min), |ui| {
                    ui.spacing_mut().item_spacing.x = 0.0;
                    egui::ScrollArea::horizontal()
                        .id_salt("edit-tools")
                        .max_height(34.0)
                        .scroll_bar_visibility(egui::scroll_area::ScrollBarVisibility::AlwaysHidden)
                        .show(ui, |ui| {
                            ui.horizontal(|ui| {
                                let ok = ready;
                                if icon_button(ui, crop_icon, "Crop (Enter or Esc when done)", e.cropping, ok).clicked()
                                {
                                    toggle_crop = true;
                                }
                                if e.cropping {
                                    let ratio = e.state.ratio;
                                    let label = RATIOS
                                        .iter()
                                        .find(|(_, r)| Some(*r) == ratio)
                                        .map_or(if ratio.is_some() { "Original" } else { "Free" }, |(name, _)| name);
                                    let menu = ui.button(label);
                                    egui::Popup::menu(&menu).show(|ui| {
                                        if ui.add(egui::Button::selectable(ratio.is_none(), "Free")).clicked() {
                                            e.set_ratio(None);
                                        }
                                        if ui.button("Original").clicked()
                                            && let Some(l) = &e.loaded
                                        {
                                            let [w, h] = turned_size(&l.full, e.state.turns);
                                            e.set_ratio(Some(w as f32 / h as f32));
                                        }
                                        for (name, r) in RATIOS {
                                            if ui.add(egui::Button::selectable(ratio == Some(r), name)).clicked() {
                                                e.set_ratio(Some(r));
                                            }
                                        }
                                    });
                                }
                                if icon_button(ui, left_icon, "Rotate left", false, ok).clicked() {
                                    e.turn(3);
                                }
                                if icon_button(ui, right_icon, "Rotate right", false, ok).clicked() {
                                    e.turn(1);
                                }
                                if icon_button(ui, flip_h_icon, "Flip left to right", false, ok).clicked() {
                                    e.turn(FLIP_H);
                                }
                                if icon_button(ui, flip_v_icon, "Flip upside down", false, ok).clicked() {
                                    e.turn(FLIP_V);
                                }
                                separator(ui);
                                if icon_button(ui, undo_icon, "Undo (Ctrl+Z)", false, ok && !e.undo.is_empty())
                                    .clicked()
                                {
                                    e.undo();
                                }
                                if icon_button(ui, redo_icon, "Redo (Ctrl+Y)", false, ok && !e.redo.is_empty())
                                    .clicked()
                                {
                                    e.redo();
                                }
                                ui.spacing_mut().item_spacing.x = 4.0;
                                let compare = egui::Button::selectable(e.comparing, "Compare");
                                if ui
                                    .add_enabled(ok && !e.cropping, compare)
                                    .on_hover_text("A line over the picture: before on its left, after on its right")
                                    .clicked()
                                {
                                    e.comparing = !e.comparing;
                                    e.split = 0.5;
                                }
                                if ui.add_enabled(ok && e.state != e.first, egui::Button::new("Reset")).clicked() {
                                    e.state = e.first;
                                }
                            });
                        });
                });
            });
        });

        ui.style_mut().always_scroll_the_only_direction = true;
        egui::ScrollArea::horizontal().id_salt("edit-dials").show(ui, |ui| {
            ui.add_enabled_ui(ready, |ui| {
                ui.horizontal(|ui| {
                    for (k, (name, range)) in DIALS.iter().enumerate() {
                        dial(ui, name, &dial_icons[k], &mut e.state.dials[k], *range);
                    }
                });
            });
        });

        if toggle_crop {
            self.toggle_crop();
        }
        if leave {
            self.leave_edit(&ctx);
        }
        if save {
            self.save_edit(&ctx, true);
        }
        if copy {
            self.save_edit(&ctx, false);
        }
    }
}

/// Renders `want` on its own thread, for the texture `pump_edit` puts it on.
fn start_preview(
    src: Arc<RgbaImage>,
    want: State,
    last: Option<Arc<Base>>,
    ctx: &egui::Context,
) -> Receiver<(Look, Arc<Base>, ColorImage)> {
    let (tx, rx) = channel();
    let ctx = ctx.clone();
    std::thread::spawn(move || {
        let base = prepare(&src, &want, last);
        let out = adjust(&base, &want);
        let image = ColorImage::from_rgba_unmultiplied([out.width() as usize, out.height() as usize], &out);
        let _ = tx.send(((want.dials, want.turns), base, image));
        ctx.request_repaint();
    });
    rx
}

/// A round control: dragging it right or up raises the value. The ring around it shows how
/// much, clockwise from the top for plus and the other way for minus, with the name below.
/// Inside is an icon, or the number while hovered and for a moment after a change.
/// Double-click for zero; the arrow keys step it once it has focus.
fn dial(ui: &mut egui::Ui, name: &str, icon: &egui::TextureHandle, value: &mut f32, range: f32) -> egui::Response {
    let (rect, resp) = ui.allocate_exact_size(DIAL_SIZE, Sense::click_and_drag());
    let before = *value;
    if resp.clicked() || resp.drag_started() {
        resp.request_focus();
    }
    if resp.dragged() {
        let d = resp.drag_delta();
        *value += (d.x - d.y) * range / 200.0;
    }
    if resp.has_focus() {
        ui.memory_mut(|m| {
            let arrows = egui::EventFilter { horizontal_arrows: true, vertical_arrows: true, ..Default::default() };
            m.set_focus_lock_filter(resp.id, arrows);
        });
        let steps = ui.input(|i| {
            let n = |k: Key| i.num_presses(k) as f32;
            n(Key::ArrowRight) + n(Key::ArrowUp) - n(Key::ArrowLeft) - n(Key::ArrowDown)
        });
        *value += steps * range / 100.0;
    }
    if resp.double_clicked() {
        *value = 0.0;
    }
    *value = value.clamp(-range, range);

    let now = ui.input(|i| i.time);
    if *value != before {
        ui.data_mut(|d| d.insert_temp(resp.id, now));
    }
    let changed = ui.data(|d| d.get_temp::<f64>(resp.id));
    let left = changed.map_or(0.0, |t| t + NUMBER_FOR - now);
    if left > 0.0 {
        ui.ctx().request_repaint_after_secs(left as f32);
    }
    let show_number = resp.hovered() || resp.dragged() || left > 0.0;

    let enabled = ui.is_enabled();
    let center = rect.center_top() + Vec2::new(0.0, DIAL_RADIUS + 5.0);
    let active = resp.hovered() || resp.dragged() || resp.has_focus();
    let painter = ui.painter();
    let ring = if active { theme::text_dim() } else { theme::line() };
    painter.circle_stroke(center, DIAL_RADIUS, Stroke::new(2.0, ring));
    let share = *value / range;
    if share != 0.0 {
        let steps = ((share.abs() * 64.0).ceil() as usize).max(2);
        let points: Vec<Pos2> = (0..=steps)
            .map(|k| {
                let a = share * std::f32::consts::TAU * k as f32 / steps as f32;
                center + DIAL_RADIUS * Vec2::new(a.sin(), -a.cos())
            })
            .collect();
        let color = if enabled { theme::text() } else { theme::icon_disabled() };
        painter.add(egui::Shape::line(points, Stroke::new(3.0, color)));
    }
    let shown = if range < 100.0 { (*value * 10.0).round() / 10.0 } else { value.round() };
    let text = match (enabled, shown != 0.0) {
        (false, _) => theme::icon_disabled(),
        (true, true) => theme::text(),
        (true, false) => theme::text_dim(),
    };
    if show_number {
        // Not "-0" for a small negative value.
        let number = if shown == 0.0 { "0".to_owned() } else { shown.to_string() };
        painter.text(center, egui::Align2::CENTER_CENTER, number, egui::FontId::monospace(13.0), text);
    } else {
        painter.image(icon.id(), Rect::from_center_size(center, Vec2::splat(24.0)), UNIT, text);
    }
    let below = rect.center_bottom() - Vec2::new(0.0, 12.0);
    painter.text(below, egui::Align2::CENTER_CENTER, name, egui::FontId::proportional(12.0), theme::text_dim());

    resp.widget_info(|| egui::WidgetInfo::slider(enabled, *value as f64, name));
    resp.on_hover_cursor(egui::CursorIcon::ResizeHorizontal).on_hover_text("Drag to change, double-click for zero")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn leaving_waits_for_changes_and_saves() {
        let mut e = Edit::new(&egui::Context::default(), PathBuf::from("none.png"), 0);
        assert!(!e.holds_leaving(), "nothing to lose");

        e.state.dials[BRIGHTNESS] = 10.0;
        assert!(e.holds_leaving(), "unsaved changes");
        e.leave_then(Then::Close);
        assert!(e.confirm_leave && e.then.is_some(), "asks, and remembers what was asked for");
        e.keep_editing();
        assert!(!e.confirm_leave && e.then.is_none(), "kept editing: the request is dropped");

        // A save on its way: no question, it just waits.
        let (_tx, rx) = channel();
        e.saving = Some(rx);
        e.state = e.first;
        assert!(e.holds_leaving(), "saving");
        e.leave_then(Then::Close);
        assert!(!e.confirm_leave && e.then.is_some());
    }

    fn picture() -> RgbaImage {
        RgbaImage::from_fn(64, 40, |x, y| image::Rgba([(x * 4) as u8, (y * 6) as u8, 90, 255]))
    }

    fn with(dial: usize, value: f32) -> State {
        let mut s = State::new(0);
        s.dials[dial] = value;
        s
    }

    fn average(img: &RgbaImage, channel: usize) -> f32 {
        img.pixels().map(|p| p[channel] as f32).sum::<f32>() / (img.width() * img.height()) as f32
    }

    #[test]
    fn untouched_dials_leave_every_pixel_alone() {
        let src = picture();
        assert_eq!(render(&src, &State::new(0)), src);
    }

    #[test]
    fn every_dial_changes_the_picture_and_keeps_its_alpha() {
        let mut src = picture();
        src.get_pixel_mut(3, 3)[3] = 128;
        for (k, (name, range)) in DIALS.iter().enumerate() {
            let out = render(&src, &with(k, *range));
            assert_ne!(out, src, "{name} did nothing");
            assert_eq!(out.dimensions(), src.dimensions());
            if k != STRAIGHTEN {
                assert_eq!(out.get_pixel(3, 3)[3], 128, "{name} changed the alpha");
            }
        }
    }

    #[test]
    fn dials_go_the_right_way() {
        let src = picture();
        let mean = |k: usize, v: f32, c: usize| average(&render(&src, &with(k, v)), c);
        let plain = average(&src, 1);
        for k in [BRIGHTNESS, EXPOSURE, SHADOWS] {
            assert!(mean(k, 60.0, 1) > plain, "{} up", DIALS[k].0);
            assert!(mean(k, -60.0, 1) < plain, "{} down", DIALS[k].0);
        }
        assert!(mean(TEMPERATURE, 60.0, 0) > average(&src, 0), "warmer: more red");
        assert!(mean(TEMPERATURE, 60.0, 2) < average(&src, 2), "warmer: less blue");
        assert!(mean(TINT, 60.0, 1) < plain, "tint up: less green");
        let gray = render(&src, &with(SATURATION, -100.0));
        assert!(gray.pixels().all(|p| p[0].abs_diff(p[1]) <= 1 && p[1].abs_diff(p[2]) <= 1), "no color left");
        // Light balance pulls a dark picture up and a bright one down.
        let dark = RgbaImage::from_pixel(32, 32, image::Rgba([30, 30, 30, 255]));
        let bright = RgbaImage::from_pixel(32, 32, image::Rgba([220, 220, 220, 255]));
        assert!(average(&render(&dark, &with(LIGHT, 80.0)), 0) > 30.0);
        assert!(average(&render(&bright, &with(LIGHT, 80.0)), 0) < 220.0);
    }

    #[test]
    fn sharpness_raises_an_edge_and_softening_lowers_it() {
        let src = RgbaImage::from_fn(200, 20, |x, _| {
            let v = if x < 100 { 60 } else { 180 };
            image::Rgba([v, v, v, 255])
        });
        let step = |img: &RgbaImage| img.get_pixel(101, 10)[0] as i32 - img.get_pixel(98, 10)[0] as i32;
        assert!(step(&render(&src, &with(SHARPNESS, 100.0))) > step(&src));
        assert!(step(&render(&src, &with(SHARPNESS, -100.0))) < step(&src));
    }

    #[test]
    fn turning_and_cropping_make_the_right_picture() {
        let src = picture();
        let mut s = State::new(0).turned(1);
        assert_eq!(render(&src, &s).dimensions(), (40, 64));
        // Clockwise, the top-left corner lands top right.
        assert_eq!(render(&src, &s).get_pixel(39, 0), src.get_pixel(0, 0));
        s = State::new(0);
        s.crop = Rect::from_min_max(Pos2::new(0.5, 0.25), Pos2::new(1.0, 0.75));
        let out = render(&src, &s);
        assert_eq!(out.dimensions(), (32, 20));
        assert_eq!(out.get_pixel(0, 0), src.get_pixel(32, 10));
        // A crop turns along with the picture and stays around the same part of it.
        let t = s.turned(1);
        assert_eq!(render(&src, &t), imageops::rotate90(&out));
        let flipped = s.turned(FLIP_H);
        assert_eq!(render(&src, &flipped), imageops::flip_horizontal(&out));
    }

    #[test]
    fn every_turn_moves_the_crop_and_the_tilt_along() {
        let src = picture();
        let mut s = with(STRAIGHTEN, 6.0);
        // On whole pixels of the 64 x 40 picture either way round.
        s.crop = Rect::from_min_max(Pos2::new(0.125, 0.25), Pos2::new(0.75, 0.875));
        s.ratio = Some(1.5);
        let out = render(&src, &s);
        for by in 0..8 {
            let t = s.turned(by);
            let want = oriented(&out, by);
            let got = render(&src, &t);
            assert_eq!(got.dimensions(), want.dimensions(), "turn {by}");
            let off = got.pixels().zip(want.pixels()).flat_map(|(a, b)| (0..3).map(move |c| a[c].abs_diff(b[c])));
            assert!(off.max().unwrap() <= 2, "turn {by}");
            // A quarter turn swaps the ratio's sides, a mirror keeps it.
            assert_eq!(t.ratio, Some(if by & 1 == 1 { 1.0 / 1.5 } else { 1.5 }), "turn {by}");
            assert_eq!(t.turned(compose_inverse(by)), s, "turn {by} and back");
        }
    }

    /// The orientation that undoes `by`.
    fn compose_inverse(by: u8) -> u8 {
        (0..8).find(|&b| compose(by, b) == 0).unwrap()
    }

    #[test]
    fn tiny_pictures_render_without_panicking() {
        for (w, h) in [(1, 1), (1, 7), (7, 1), (2, 3)] {
            let src = RgbaImage::from_fn(w, h, |x, y| image::Rgba([(x * 90) as u8, (y * 30) as u8, 128, 255]));
            for (k, (_, range)) in DIALS.iter().enumerate() {
                let mut s = with(k, *range).turned(1);
                s.crop = Rect::from_min_max(Pos2::new(0.4, 0.4), Pos2::new(1.0, 1.0));
                let out = render(&src, &s);
                assert!(out.width() >= 1 && out.height() >= 1);
            }
        }
    }

    #[test]
    fn straightening_leaves_no_empty_corners() {
        let src = picture();
        for angle in [-45.0, -7.5, 3.0, 45.0] {
            let out = render(&src, &with(STRAIGHTEN, angle));
            assert_eq!(out.dimensions(), src.dimensions());
            assert!(out.pixels().all(|p| p[3] == 255), "{angle}");
        }
        // Clockwise: what's right of the middle moves down.
        let mut bright_right = RgbaImage::from_pixel(101, 101, image::Rgba([0, 0, 0, 255]));
        for x in 60..70 {
            bright_right.put_pixel(x, 50, image::Rgba([255, 255, 255, 255]));
        }
        let out = straightened(&bright_right, 20.0);
        let (mut upper, mut lower) = (0u32, 0u32);
        for (_, y, p) in out.enumerate_pixels() {
            if y < 50 {
                upper += p[0] as u32;
            } else if y > 50 {
                lower += p[0] as u32;
            }
        }
        assert!(lower > upper);
    }

    #[test]
    fn the_preview_cache_is_reused_until_the_picture_changes() {
        let src = picture();
        let first = prepare(&src, &State::new(0), None);
        assert!(Arc::ptr_eq(&first, &prepare(&src, &with(EXPOSURE, 40.0), Some(first.clone()))));
        let sharp = prepare(&src, &with(SHARPNESS, 40.0), Some(first.clone()));
        assert!(sharp.fine.is_some() && !Arc::ptr_eq(&first, &sharp));
        assert_eq!(sharp.img, first.img, "the blur is added, the picture stays");
        assert!(!Arc::ptr_eq(&sharp, &prepare(&src, &State::new(1), Some(sharp.clone()))));
    }

    #[test]
    fn dragging_a_crop_stays_inside_the_picture() {
        let c = Rect::from_min_max(Pos2::new(0.2, 0.2), Pos2::new(0.8, 0.8));
        let moved = drag_crop(c, [true; 4], Vec2::new(0.5, -0.5), None);
        assert!((moved.min - Pos2::new(0.4, 0.0)).length() < 1e-5, "{moved:?}");
        assert!((moved.size() - c.size()).length() < 1e-5, "keeps its size");
        let left = drag_crop(c, [true, false, false, false], Vec2::new(-1.0, 0.3), None);
        assert_eq!(left, Rect::from_min_max(Pos2::new(0.0, 0.2), Pos2::new(0.8, 0.8)), "stops at the edge");
        let thin = drag_crop(c, [false, false, true, false], Vec2::new(-5.0, 0.0), None);
        assert!((thin.width() - MIN_CROP).abs() < 1e-5, "stops before nothing is left");
    }

    #[test]
    fn a_ratio_holds_from_every_grip() {
        let c = centered_crop(1.5);
        let grips = [
            [true, false, false, false],
            [false, true, false, false],
            [false, false, true, false],
            [false, false, false, true],
            [true, true, false, false],
            [false, false, true, true],
            [false, true, true, false],
        ];
        for grip in grips {
            for d in [Vec2::new(0.3, 0.3), Vec2::new(-0.3, -0.3), Vec2::new(2.0, -2.0), Vec2::new(-2.0, 2.0)] {
                let r = drag_crop(c, grip, d, Some(1.5));
                assert!((r.width() / r.height() - 1.5).abs() < 1e-3, "{grip:?} {d:?}: {r:?}");
                assert!(
                    r.min.x >= -1e-5 && r.min.y >= -1e-5 && r.max.x <= 1.0 + 1e-5 && r.max.y <= 1.0 + 1e-5,
                    "{grip:?} {d:?}: {r:?}"
                );
            }
        }
        assert_eq!(centered_crop(2.0).height(), 0.5);
        assert_eq!(centered_crop(0.5).width(), 0.5);
    }

    #[test]
    fn a_press_finds_the_edge_it_is_near() {
        let c = Rect::from_min_max(Pos2::new(100.0, 100.0), Pos2::new(300.0, 300.0));
        assert_eq!(grip_at(c, Pos2::new(50.0, 200.0)), None);
        assert_eq!(grip_at(c, Pos2::new(200.0, 200.0)), Some([true; 4]), "the middle moves it");
        assert_eq!(grip_at(c, Pos2::new(101.0, 200.0)), Some([true, false, false, false]));
        assert_eq!(grip_at(c, Pos2::new(299.0, 301.0)), Some([false, false, true, true]));
    }

    #[test]
    fn crop_pixels_stay_in_the_picture() {
        assert_eq!(crop_pixels(UNIT, 10, 20), (0, 0, 10, 20));
        let tiny = Rect::from_min_max(Pos2::new(0.999, 0.999), Pos2::new(1.0, 1.0));
        assert_eq!(crop_pixels(tiny, 10, 10), (9, 9, 1, 1));
    }

    /// One frame with no screen: the pointer events, and a dial to get them.
    fn dial_frame(ctx: &egui::Context, events: Vec<egui::Event>, value: &mut f32) {
        let input = egui::RawInput {
            screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(400.0, 300.0))),
            events,
            ..Default::default()
        };
        let mut out = ctx.run_ui(input, |ui| {
            egui::CentralPanel::default().show(ui, |ui| {
                let icon = ui.ctx().load_texture("test", ColorImage::filled([1, 1], Color32::WHITE), TextureOptions::LINEAR);
                dial(ui, "Test", &icon, value, 100.0);
            });
        });
        out.textures_delta.clear();
    }

    fn press(pos: Pos2, pressed: bool) -> egui::Event {
        egui::Event::PointerButton { pos, button: egui::PointerButton::Primary, pressed, modifiers: Default::default() }
    }

    #[test]
    fn dragging_the_crop_box_on_screen_resizes_and_moves_it() {
        let ctx = egui::Context::default();
        let mut e = Edit::new(&ctx, PathBuf::from("nowhere.png"), 0);
        e.cropping = true;
        // The picture sits at 50..250 on both axes.
        let page = Rect::from_min_size(Pos2::new(50.0, 50.0), Vec2::splat(200.0));
        let frame = |e: &mut Edit, events: Vec<egui::Event>| {
            let input = egui::RawInput {
                screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(400.0, 300.0))),
                events,
                ..Default::default()
            };
            let mut out = ctx.run_ui(input, |ui| {
                egui::CentralPanel::default().frame(egui::Frame::NONE).show(ui, |ui| {
                    let all = Rect::from_min_size(Pos2::ZERO, Vec2::new(400.0, 300.0));
                    let resp = ui.allocate_rect(all, Sense::click_and_drag());
                    let painter = ui.painter_at(all);
                    e.crop_ui(&resp, page, &painter, ui.ctx());
                });
            });
            out.textures_delta.clear();
        };
        let drag = |from: Pos2, to: Pos2| {
            vec![
                vec![egui::Event::PointerMoved(from)],
                vec![press(from, true)],
                vec![egui::Event::PointerMoved(from + (to - from) * 0.3)],
                vec![egui::Event::PointerMoved(to)],
                vec![press(to, false)],
                vec![],
            ]
        };
        frame(&mut e, vec![]);
        // The bottom-right corner pulled in by a quarter of the picture.
        for events in drag(Pos2::new(250.0, 250.0), Pos2::new(200.0, 200.0)) {
            frame(&mut e, events);
        }
        let c = e.state.crop;
        assert_eq!(c.min, Pos2::ZERO, "{c:?}");
        assert!((c.max.x - 0.75).abs() < 0.01 && (c.max.y - 0.75).abs() < 0.01, "{c:?}");
        // Pressed in the middle, it moves whole.
        for events in drag(Pos2::new(125.0, 125.0), Pos2::new(145.0, 165.0)) {
            frame(&mut e, events);
        }
        let m = e.state.crop;
        assert!((m.size() - c.size()).length() < 1e-4, "{m:?}");
        assert!((m.min - Pos2::new(0.1, 0.2)).length() < 0.01, "{m:?}");
    }

    #[test]
    fn dragging_a_dial_moves_it_and_a_double_click_zeroes_it() {
        let ctx = egui::Context::default();
        let mut value = 0.0;
        let at = Pos2::new(40.0, 40.0);
        let frames: Vec<Vec<egui::Event>> = vec![
            vec![],
            vec![egui::Event::PointerMoved(at)],
            vec![press(at, true)],
            vec![egui::Event::PointerMoved(at + Vec2::new(10.0, 0.0))],
            vec![egui::Event::PointerMoved(at + Vec2::new(40.0, 0.0))],
            vec![egui::Event::PointerMoved(at + Vec2::new(40.0, -20.0))],
            vec![press(at + Vec2::new(40.0, -20.0), false)],
        ];
        for events in frames {
            dial_frame(&ctx, events, &mut value);
        }
        // 60 points right and up, half a unit each. The first few only start the drag.
        assert!(value > 20.0 && value <= 30.0, "{value}");
        let at = at + Vec2::new(40.0, -20.0);
        for events in
            [vec![press(at, true)], vec![press(at, false)], vec![press(at, true)], vec![press(at, false)], vec![]]
        {
            dial_frame(&ctx, events, &mut value);
        }
        assert_eq!(value, 0.0);
        // The click left it with focus, so the arrow keys step it.
        let key = |key| egui::Event::Key {
            key,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: Default::default(),
        };
        dial_frame(&ctx, vec![], &mut value);
        dial_frame(&ctx, vec![key(Key::ArrowRight), key(Key::ArrowRight), key(Key::ArrowUp)], &mut value);
        assert_eq!(value, 3.0);
        dial_frame(&ctx, vec![key(Key::ArrowLeft)], &mut value);
        assert_eq!(value, 2.0);
    }
}
