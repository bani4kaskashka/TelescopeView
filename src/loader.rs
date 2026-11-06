//! Background decoding. A few worker threads pull paths off a priority queue, decode them to
//! RGBA and hand them back to the UI thread, which uploads them as textures before they are
//! ever shown. The UI replaces the whole queue every time the user moves, so the image they
//! are about to look at is always first in line.

use crate::files;
use eframe::egui::{self, Color32, ColorImage, Pos2, Rect};
use image::codecs::{gif::GifDecoder, png::PngDecoder, webp::WebPDecoder};
use image::metadata::Orientation;
use image::{AnimationDecoder, ColorType, DynamicImage, ImageDecoder, ImageFormat, ImageReader};
use std::collections::{HashSet, VecDeque};
use std::io::Cursor;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Condvar, Mutex, OnceLock};
use std::thread;
use std::time::Duration;

/// Total pixels kept for one still image. Past this it is scaled down (a 30000 x 30000 scan
/// would be 3.6 GB as RGBA). Long webtoon strips (800 x 60000 = 48 MP) stay full size.
const PIXEL_BUDGET: u64 = 64_000_000;
/// Total pixels across all frames of an animation; longer ones are cut short.
const ANIMATION_BUDGET: u64 = 48_000_000;
/// PDF pages are rendered this many pixels on their long side.
const PDF_LONG_SIDE: u32 = 3000;

pub struct Decoded {
    pub path: PathBuf,
    pub result: Result<Image, String>,
}

pub struct Image {
    /// One for a still image, several for an animation.
    pub frames: Vec<Frame>,
    /// Size on disk after EXIF rotation. The pixels may be fewer (see `PIXEL_BUDGET`).
    pub original_size: [u32; 2],
    /// For the info panel ("JPG", "PDF page", ...).
    pub format: String,
    /// Any see-through pixel (gets a checkerboard behind it).
    pub has_alpha: bool,
}

pub struct Frame {
    /// Usually one. Images bigger than a GPU texture are split into several.
    pub tiles: Vec<Tile>,
    /// How long the frame stays up in an animation.
    pub delay: Duration,
}

pub struct Tile {
    pub pixels: ColorImage,
    /// Where the tile sits in the image, in 0..1 coordinates.
    pub rect: Rect,
}

const FULL: Rect = Rect { min: Pos2::ZERO, max: Pos2::new(1.0, 1.0) };

struct State {
    queue: VecDeque<PathBuf>,
    /// Picked up by a worker and not yet received by the UI. Tracked until the UI takes the
    /// result so a path can never be queued twice while its pixels sit in the channel.
    in_flight: HashSet<PathBuf>,
    max_side: u32,
    /// Thumbnail loader: every picture comes back shrunk to fit this many pixels (0: full size).
    thumb: u32,
    shutdown: bool,
}

struct Shared {
    state: Mutex<State>,
    wake: Condvar,
}

pub struct Loader {
    shared: Arc<Shared>,
    rx: Receiver<Decoded>,
}

impl Loader {
    /// `threads` 0 picks a number for this machine.
    pub fn new(ctx: egui::Context, max_side: u32, threads: usize) -> Self {
        Self::start(ctx, max_side, threads, 0)
    }

    /// Small copies for the thumbnail panel, `side` pixels at most, on their own threads so
    /// they never hold up the picture being looked at.
    pub fn thumbnails(ctx: egui::Context, side: u32, threads: usize) -> Self {
        Self::start(ctx, 4096, threads.max(1), side)
    }

    fn start(ctx: egui::Context, max_side: u32, threads: usize, thumb: u32) -> Self {
        let state = State { queue: VecDeque::new(), in_flight: HashSet::new(), max_side, thumb, shutdown: false };
        let shared = Arc::new(Shared { state: Mutex::new(state), wake: Condvar::new() });
        let (tx, rx) = mpsc::channel();
        // Half the logical cores (one per physical core), up to 6: past that the extra threads
        // mostly decode images the user has already flipped past.
        let auto = (thread::available_parallelism().map_or(4, |n| n.get()) / 2).clamp(2, 6);
        let threads = if threads == 0 { auto } else { threads };
        let name = if thumb > 0 { "thumb" } else { "decode" };
        for i in 0..threads {
            let (shared, tx, ctx) = (shared.clone(), tx.clone(), ctx.clone());
            thread::Builder::new()
                .name(format!("{name}-{i}"))
                .spawn(move || worker(shared, tx, ctx))
                .expect("spawn decoder thread");
        }
        Self { shared, rx }
    }

    /// Replaces the queue with `wanted`, highest priority first. Paths already being decoded
    /// are left to finish; everything that is no longer wanted is dropped.
    pub fn request(&self, wanted: impl IntoIterator<Item = PathBuf>) {
        let mut s = self.shared.state.lock().unwrap();
        let queue: VecDeque<PathBuf> = wanted.into_iter().filter(|p| !s.in_flight.contains(p)).collect();
        s.queue = queue;
        drop(s);
        self.shared.wake.notify_all();
    }

    pub fn try_recv(&self) -> Option<Decoded> {
        let d = self.rx.try_recv().ok()?;
        self.shared.state.lock().unwrap().in_flight.remove(&d.path);
        Some(d)
    }
}

impl Drop for Loader {
    fn drop(&mut self) {
        // Workers notice on their next wake-up; one stuck in a long decode just finishes and exits.
        self.shared.state.lock().unwrap().shutdown = true;
        self.shared.wake.notify_all();
    }
}

fn worker(shared: Arc<Shared>, tx: Sender<Decoded>, ctx: egui::Context) {
    loop {
        let (path, max_side, thumb) = {
            let mut s = shared.state.lock().unwrap();
            loop {
                if s.shutdown {
                    return;
                }
                if let Some(p) = s.queue.pop_front() {
                    s.in_flight.insert(p.clone());
                    break (p, s.max_side, s.thumb);
                }
                s = shared.wake.wait(s).unwrap();
            }
        };
        // A decoder bug on one odd file must not kill the thread (its path would then stay
        // "in flight" forever); it just becomes an error for that image.
        let mut result = std::panic::catch_unwind(|| {
            let img = decode(&path, max_side)?;
            Ok(if thumb > 0 { shrink(img, thumb) } else { img })
        })
        .unwrap_or_else(|_| Err("decoder crashed on this file".to_owned()));
        if let Ok(img) = &mut result {
            img.has_alpha = has_alpha(img);
        }
        if tx.send(Decoded { path, result }).is_err() {
            return;
        }
        ctx.request_repaint();
    }
}

/// The first frame, scaled down to fit `side` pixels, as one small tile.
fn shrink(img: Image, side: u32) -> Image {
    let frame = &img.frames[0];
    let first = &frame.tiles[0];
    // Size of the whole decoded picture (it may be split into several tiles).
    let full_w = first.pixels.size[0] as f32 / first.rect.width().max(1e-6);
    let full_h = first.pixels.size[1] as f32 / first.rect.height().max(1e-6);
    let scale = (side as f32 / full_w.max(full_h)).min(1.0);
    let (w, h) = (((full_w * scale).round() as u32).max(1), ((full_h * scale).round() as u32).max(1));
    let mut out = image::RgbaImage::new(w, h);
    for t in &frame.tiles {
        let [tw, th] = t.pixels.size;
        let bytes: Vec<u8> = bytemuck::cast_slice(&t.pixels.pixels).to_vec();
        let Some(src) = image::RgbaImage::from_raw(tw as u32, th as u32, bytes) else { continue };
        let x = (t.rect.min.x * w as f32).round() as u32;
        let y = (t.rect.min.y * h as f32).round() as u32;
        let rw = ((t.rect.width() * w as f32).round() as u32).max(1);
        let rh = ((t.rect.height() * h as f32).round() as u32).max(1);
        // Box-filtered: fast even from a 50 MP photo, and plenty sharp at this size.
        let small = image::imageops::thumbnail(&src, rw, rh);
        image::imageops::replace(&mut out, &small, x as i64, y as i64);
    }
    // The pixels are already premultiplied (they came from Color32s).
    let pixels = ColorImage::from_rgba_premultiplied([w as usize, h as usize], out.as_raw()).pixels;
    let tile = Tile {
        pixels: ColorImage { size: [w as usize, h as usize], source_size: egui::Vec2::new(w as f32, h as f32), pixels },
        rect: FULL,
    };
    Image {
        frames: vec![Frame { tiles: vec![tile], delay: Duration::ZERO }],
        original_size: img.original_size,
        format: img.format,
        has_alpha: false,
    }
}

/// Looked up here on the decode thread, not while drawing. JPEGs can't have any.
fn has_alpha(img: &Image) -> bool {
    !img.format.starts_with("JP")
        && img.frames.iter().any(|f| f.tiles.iter().any(|t| t.pixels.pixels.iter().any(|c| c.a() < 255)))
}

/// For the duplicate finder: the first frame as one picture of at most `side` pixels. The same
/// way the thumbnails get theirs, as `decode`'s `max_side` only says where to cut it into tiles.
pub fn decode_for_twins(path: &Path, side: u32) -> Result<crate::twins::Picture, String> {
    let img = std::panic::catch_unwind(|| decode(path, 4096).map(|img| shrink(img, side)))
        .unwrap_or_else(|_| Err("decoder crashed on this file".to_owned()))?;
    let tile = &img.frames[0].tiles[0].pixels;
    Ok(crate::twins::Picture {
        width: tile.size[0] as u32,
        height: tile.size[1] as u32,
        rgba: tile.pixels.iter().flat_map(|c| c.to_srgba_unmultiplied()).collect(),
        full_size: img.original_size,
    })
}

pub fn decode(path: &Path, max_side: u32) -> Result<Image, String> {
    if let Some((archive, entry)) = files::archive_of(path)
        && files::ext_lower(archive) == "pdf"
    {
        let n = files::pdf_page_number(&entry).ok_or("not a PDF page")?;
        let bmp = pdf_page(archive, n, PDF_LONG_SIDE.min(max_side))?;
        let mut img = decode_bytes(&bmp, "bmp", max_side)?;
        img.format = "PDF page".to_owned();
        return Ok(img);
    }
    if crate::video::is_video(path) && files::archive_of(path).is_none() {
        let png = crate::video::poster(path)?;
        let mut img = decode_bytes(&png, "png", max_side)?;
        img.format = format!("Video ({})", files::ext_lower(path).to_uppercase());
        return Ok(img);
    }
    let bytes = files::read_page(path).map_err(|e| e.to_string())?;
    if bytes.is_empty() {
        return Err("empty file".to_owned());
    }
    decode_bytes(&bytes, &files::ext_lower(path), max_side)
}

#[cfg(windows)]
fn pdf_page(pdf: &Path, n: u32, long_side: u32) -> Result<Vec<u8>, String> {
    crate::winimg::pdf_render(pdf, n, long_side)
}

#[cfg(not(windows))]
fn pdf_page(_: &Path, _: u32, _: u32) -> Result<Vec<u8>, String> {
    Err("PDF needs Windows".to_owned())
}

pub fn decode_bytes(bytes: &[u8], ext: &str, max_side: u32) -> Result<Image, String> {
    let head = &bytes[..bytes.len().min(512)];
    if matches!(ext, "svg" | "svgz") || looks_like_svg(head) {
        return decode_svg(bytes, max_side);
    }
    if ext == "psd" || head.starts_with(b"8BPS") {
        let psd = psd::Psd::from_bytes(bytes).map_err(|e| e.to_string())?;
        let (w, h) = (psd.width(), psd.height());
        return Ok(from_rgba(psd.rgba(), w, h, max_side, "PSD"));
    }
    if head.starts_with(&[0xFF, 0x0A]) || head.starts_with(b"\0\0\0\x0CJXL \r\n\x87\n") {
        let decoder = jxl_oxide::integration::JxlDecoder::new(Cursor::new(bytes)).map_err(|e| e.to_string())?;
        return decode_still(decoder, max_side, "JPEG XL");
    }
    // TGA has no signature, so the extension decides when the content doesn't.
    let format = image::guess_format(bytes).ok().or_else(|| ImageFormat::from_extension(ext));
    let builtin = match format {
        Some(format) => decode_builtin(bytes, format, max_side),
        None => Err("unknown format".to_owned()),
    };
    match builtin {
        Ok(img) => Ok(img),
        // Formats the Rust decoders don't know (HEIC, AVIF, RAW, JPEG XR), and files they
        // reject that Windows' own decoders are sometimes more forgiving with. Then ffmpeg, for
        // HEIC without Windows' HEIF extension (or on Linux).
        Err(e) => match system_decode(bytes, max_side, ext) {
            Ok(img) => Ok(img),
            Err(system) => match ffmpeg_decode(bytes, ext, max_side) {
                Some(Ok(img)) => Ok(img),
                _ if format.is_some() => Err(e),
                _ => Err(system),
            },
        },
    }
}

fn ffmpeg_decode(bytes: &[u8], ext: &str, max_side: u32) -> Option<Result<Image, String>> {
    if !crate::video::STILLS.contains(&ext) {
        return None;
    }
    let png = match crate::video::still(bytes, ext) {
        Ok(png) => png,
        Err(e) => return Some(Err(e)),
    };
    Some(decode_builtin(&png, ImageFormat::Png, max_side).map(|mut img| {
        img.format = ext.to_ascii_uppercase();
        img
    }))
}

#[cfg(windows)]
fn system_decode(bytes: &[u8], max_side: u32, ext: &str) -> Result<Image, String> {
    let (rgba, [w, h], original) = crate::winimg::decode(bytes, max_side)?;
    let mut img = from_rgba(rgba, w, h, u32::MAX, &ext.to_ascii_uppercase());
    img.original_size = original;
    Ok(img)
}

#[cfg(not(windows))]
fn system_decode(_: &[u8], _: u32, _: &str) -> Result<Image, String> {
    Err("unsupported format".to_owned())
}

fn looks_like_svg(head: &[u8]) -> bool {
    let text = String::from_utf8_lossy(head);
    let t = text.trim_start_matches('\u{feff}').trim_start();
    (t.starts_with("<?xml") || t.starts_with("<svg") || t.starts_with("<!--")) && text.contains("<svg")
}

fn format_name(format: ImageFormat) -> String {
    format.extensions_str().first().map_or_else(|| format!("{format:?}"), |e| e.to_ascii_uppercase())
}

fn decode_builtin(bytes: &[u8], format: ImageFormat, max_side: u32) -> Result<Image, String> {
    let name = format_name(format);
    let e = |e: image::ImageError| e.to_string();
    match format {
        ImageFormat::Gif => {
            let d = GifDecoder::new(Cursor::new(bytes)).map_err(e)?;
            return decode_animation(d.into_frames(), max_side, &name);
        }
        ImageFormat::WebP => {
            let d = WebPDecoder::new(Cursor::new(bytes)).map_err(e)?;
            if d.has_animation() {
                return decode_animation(d.into_frames(), max_side, &name);
            }
        }
        ImageFormat::Png => {
            let d = PngDecoder::new(Cursor::new(bytes)).map_err(e)?;
            if d.is_apng().map_err(e)? {
                return decode_animation(d.apng().map_err(e)?.into_frames(), max_side, "APNG");
            }
        }
        _ => {}
    }
    let mut reader = ImageReader::with_format(Cursor::new(bytes), format);
    // The default 512 MB allocation cap rejects big scans; they get scaled down anyway.
    reader.no_limits();
    decode_still(reader.into_decoder().map_err(e)?, max_side, &name)
}

/// The size to scale to when `w x h` is over the pixel budget.
fn fit_budget(w: u32, h: u32) -> Option<(u32, u32)> {
    let px = w as u64 * h as u64;
    (px > PIXEL_BUDGET).then(|| {
        let s = (PIXEL_BUDGET as f64 / px as f64).sqrt();
        (((w as f64 * s) as u32).max(1), ((h as f64 * s) as u32).max(1))
    })
}

fn still(tiles: Vec<Tile>, original_size: [u32; 2], format: &str) -> Image {
    Image {
        frames: vec![Frame { tiles, delay: Duration::ZERO }],
        original_size,
        format: format.to_owned(),
        has_alpha: false,
    }
}

/// One frame from straight-alpha RGBA, scaled to the pixel budget and split into tiles.
fn from_rgba(rgba: Vec<u8>, w: u32, h: u32, max_side: u32, format: &str) -> Image {
    let mut img = image::RgbaImage::from_raw(w, h, rgba).expect("buffer matches its size");
    let original_size = [w, h];
    if let Some((nw, nh)) = fit_budget(w, h) {
        img = image::imageops::resize(&img, nw, nh, image::imageops::FilterType::Triangle);
    }
    let (w, h) = (img.width() as usize, img.height() as usize);
    let pixels = ColorImage::from_rgba_unmultiplied([w, h], img.as_raw()).pixels;
    still(tile(pixels, w, h, max_side), original_size, format)
}

/// Splits an image bigger than one GPU texture into a grid of textures. A long strip stays
/// at full resolution this way instead of being shrunk to 16384 pixels tall.
fn tile(pixels: Vec<Color32>, w: usize, h: usize, max_side: u32) -> Vec<Tile> {
    let side = (max_side as usize).max(1);
    if w <= side && h <= side {
        return vec![Tile { pixels: ColorImage::new([w, h], pixels), rect: FULL }];
    }
    let mut tiles = Vec::new();
    for y0 in (0..h).step_by(side) {
        for x0 in (0..w).step_by(side) {
            let (tw, th) = (side.min(w - x0), side.min(h - y0));
            let mut px = Vec::with_capacity(tw * th);
            for y in y0..y0 + th {
                px.extend_from_slice(&pixels[y * w + x0..y * w + x0 + tw]);
            }
            let rect = Rect::from_min_max(
                Pos2::new(x0 as f32 / w as f32, y0 as f32 / h as f32),
                Pos2::new((x0 + tw) as f32 / w as f32, (y0 + th) as f32 / h as f32),
            );
            tiles.push(Tile { pixels: ColorImage::new([tw, th], px), rect });
        }
    }
    tiles
}

fn decode_animation(frames: image::Frames<'_>, max_side: u32, format: &str) -> Result<Image, String> {
    let mut out = Vec::new();
    let mut total = 0u64;
    let mut original_size = [0, 0];
    for frame in frames {
        let frame = match frame {
            Ok(f) => f,
            // A broken frame late in a long GIF: play what decoded fine.
            Err(_) if !out.is_empty() => break,
            Err(e) => return Err(e.to_string()),
        };
        let (num, den) = frame.delay().numer_denom_ms();
        let ms = num.checked_div(den).unwrap_or(0);
        // Like browsers: "as fast as possible" delays play at 10 fps.
        let delay = Duration::from_millis(if ms < 20 { 100 } else { ms as u64 });
        let mut buf = frame.into_buffer();
        original_size = [buf.width(), buf.height()];
        if buf.width() > max_side || buf.height() > max_side {
            let s = max_side as f32 / buf.width().max(buf.height()) as f32;
            let (w, h) = (((buf.width() as f32 * s) as u32).max(1), ((buf.height() as f32 * s) as u32).max(1));
            buf = image::imageops::resize(&buf, w, h, image::imageops::FilterType::Triangle);
        }
        total += buf.width() as u64 * buf.height() as u64;
        if !out.is_empty() && total > ANIMATION_BUDGET {
            break;
        }
        let pixels = ColorImage::from_rgba_unmultiplied([buf.width() as usize, buf.height() as usize], buf.as_raw());
        out.push(Frame { tiles: vec![Tile { pixels, rect: FULL }], delay });
    }
    if out.is_empty() {
        return Err("no frames".to_owned());
    }
    if out.len() == 1 {
        out[0].delay = Duration::ZERO;
    }
    Ok(Image { frames: out, original_size, format: format.to_owned(), has_alpha: false })
}

fn decode_svg(bytes: &[u8], max_side: u32) -> Result<Image, String> {
    use resvg::{tiny_skia, usvg};
    static FONTS: OnceLock<Arc<usvg::fontdb::Database>> = OnceLock::new();
    let fonts = FONTS.get_or_init(|| {
        let mut db = usvg::fontdb::Database::new();
        db.load_system_fonts();
        Arc::new(db)
    });
    let opts = usvg::Options { fontdb: fonts.clone(), ..Default::default() };
    let tree = usvg::Tree::from_data(bytes, &opts).map_err(|e| e.to_string())?;
    let size = tree.size();
    let natural = size.width().max(size.height()).max(1.0);
    // Vector art: rendered big enough to stay sharp when enlarged, within the texture limit.
    let s = natural.max(2048.0).min(max_side as f32) / natural;
    let (w, h) = (((size.width() * s).round() as u32).max(1), ((size.height() * s).round() as u32).max(1));
    let mut pixmap = tiny_skia::Pixmap::new(w, h).ok_or("SVG too large")?;
    resvg::render(&tree, tiny_skia::Transform::from_scale(s, s), &mut pixmap.as_mut());
    let pixels = ColorImage::from_rgba_premultiplied([w as usize, h as usize], pixmap.data());
    let original = [(size.width().round() as u32).max(1), (size.height().round() as u32).max(1)];
    Ok(still(vec![Tile { pixels, rect: FULL }], original, "SVG"))
}

/// Still images through the `image` crate (and JPEG XL, which plugs into it).
fn decode_still(mut decoder: impl ImageDecoder, max_side: u32, format: &str) -> Result<Image, String> {
    let orientation = decoder.orientation().unwrap_or(Orientation::NoTransforms);
    let (w, h) = decoder.dimensions();
    let channels = match decoder.color_type() {
        ColorType::L8 => 1,
        ColorType::Rgb8 => 3,
        ColorType::Rgba8 => 4,
        _ => 0,
    };
    if channels == 0 || fit_budget(w, h).is_some() {
        let mut img = DynamicImage::from_decoder(decoder).map_err(|e| e.to_string())?;
        img.apply_orientation(orientation);
        let (w, h) = (img.width(), img.height());
        return Ok(from_rgba(img.into_rgba8().into_raw(), w, h, max_side, format));
    }

    // Fast path for 8-bit gray/RGB/RGBA (nearly every JPEG, PNG and WebP; manga scans are gray). Decodes straight into
    // the texture's own allocation and widens it in place: one 4 bytes/pixel buffer per image
    // instead of three. With several decoders running at once the extra copies were the
    // bottleneck, not the decoding itself.
    let (wu, hu) = (w as usize, h as usize);
    let n = wu * hu;
    let mut pixels = vec![Color32::TRANSPARENT; n];
    let bytes: &mut [u8] = bytemuck::cast_slice_mut(&mut pixels);
    decoder.read_image(&mut bytes[..n * channels]).map_err(|e| e.to_string())?;
    if channels == 1 {
        for i in (0..n).rev() {
            let v = bytes[i];
            bytes[i * 4..i * 4 + 4].copy_from_slice(&[v, v, v, 255]);
        }
    } else if channels == 3 {
        // Back to front, so no pixel is overwritten before it has been read.
        for i in (0..n).rev() {
            let (r, g, b) = (bytes[i * 3], bytes[i * 3 + 1], bytes[i * 3 + 2]);
            bytes[i * 4..i * 4 + 4].copy_from_slice(&[r, g, b, 255]);
        }
    } else {
        for [r, g, b, a] in bytes.as_chunks_mut::<4>().0 {
            if *a != 255 {
                [*r, *g, *b, *a] = Color32::from_rgba_unmultiplied(*r, *g, *b, *a).to_array();
            }
        }
    }
    let (pixels, [ow, oh]) = orient(pixels, wu, hu, orientation);
    Ok(still(tile(pixels, ow, oh, max_side), [ow as u32, oh as u32], format))
}

/// Applies an EXIF orientation (same meaning as `DynamicImage::apply_orientation`).
fn orient(src: Vec<Color32>, w: usize, h: usize, o: Orientation) -> (Vec<Color32>, [usize; 2]) {
    use Orientation::*;
    let (ow, oh) = match o {
        NoTransforms => return (src, [w, h]),
        Rotate180 | FlipHorizontal | FlipVertical => (w, h),
        Rotate90 | Rotate270 | Rotate90FlipH | Rotate270FlipH => (h, w),
    };
    let mut out = Vec::with_capacity(ow * oh);
    for dy in 0..oh {
        for dx in 0..ow {
            // Source pixel for each output pixel.
            let (sx, sy) = match o {
                NoTransforms => (dx, dy),
                Rotate180 => (w - 1 - dx, h - 1 - dy),
                FlipHorizontal => (w - 1 - dx, dy),
                FlipVertical => (dx, h - 1 - dy),
                Rotate90 => (dy, h - 1 - dx),
                Rotate270 => (w - 1 - dy, dx),
                Rotate90FlipH => (dy, dx),
                Rotate270FlipH => (w - 1 - dy, h - 1 - dx),
            };
            out.push(src[sy * w + sx]);
        }
    }
    (out, [ow, oh])
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::{Rgb, RgbImage};

    /// The hand-written orientation mapping must match the image crate's for all 8 cases.
    #[test]
    fn orient_matches_image_crate() {
        let img = RgbImage::from_fn(3, 2, |x, y| Rgb([x as u8 * 40, y as u8 * 90, 7]));
        let src: Vec<Color32> = img.pixels().map(|p| Color32::from_rgb(p[0], p[1], p[2])).collect();
        use Orientation::*;
        for o in
            [NoTransforms, Rotate90, Rotate180, Rotate270, FlipHorizontal, FlipVertical, Rotate90FlipH, Rotate270FlipH]
        {
            let mut want = DynamicImage::ImageRgb8(img.clone());
            want.apply_orientation(o);
            let want_px: Vec<Color32> = want.to_rgb8().pixels().map(|p| Color32::from_rgb(p[0], p[1], p[2])).collect();
            let (got, size) = orient(src.clone(), 3, 2, o);
            assert_eq!(size, [want.width() as usize, want.height() as usize], "{o:?}");
            assert_eq!(got, want_px, "{o:?}");
        }
    }

    /// Wide enough to come back from `decode` in tiles: the far corner has to be in the picture.
    #[test]
    fn twins_get_the_whole_picture_not_its_first_tile() {
        let dir = std::env::temp_dir().join(format!("tv-twins-decode-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("wide.png");
        let img = RgbImage::from_fn(5000, 300, |x, _| Rgb(if x < 4500 { [0, 0, 0] } else { [255, 255, 255] }));
        img.save(&path).unwrap();
        let pic = decode_for_twins(&path, 256).unwrap();
        std::fs::remove_dir_all(&dir).unwrap();
        assert_eq!((pic.width, pic.height), (256, 15));
        assert_eq!(pic.full_size, [5000, 300]);
        assert_eq!(pic.rgba.len(), 256 * 15 * 4);
        assert_eq!(&pic.rgba[..4], &[0, 0, 0, 255]);
        assert_eq!(&pic.rgba[255 * 4..256 * 4], &[255, 255, 255, 255]);
    }

    #[test]
    fn tall_images_are_tiled_not_shrunk() {
        let (w, h) = (3, 10);
        let px: Vec<Color32> = (0..w * h).map(|i| Color32::from_gray(i as u8)).collect();
        let tiles = tile(px, w, h, 4);
        assert_eq!(tiles.len(), 3);
        assert_eq!(tiles[2].pixels.size, [3, 2]);
        assert_eq!(tiles[1].pixels.pixels[0], Color32::from_gray(12), "second tile starts at row 4");
        assert_eq!(tiles[2].rect.max, Pos2::new(1.0, 1.0));
    }

    #[test]
    fn svg_and_gif_decode() {
        let svg = br#"<svg xmlns="http://www.w3.org/2000/svg" width="20" height="10"><rect width="20" height="10" fill="red"/></svg>"#;
        let img = decode_bytes(svg, "svg", 4096).unwrap();
        assert_eq!(img.original_size, [20, 10]);
        assert_eq!(img.frames[0].tiles[0].pixels.size, [2048, 1024]);

        let mut gif = Vec::new();
        {
            let mut enc = image::codecs::gif::GifEncoder::new(&mut gif);
            for g in [0u8, 255] {
                let buf = image::RgbaImage::from_pixel(4, 4, image::Rgba([g, g, g, 255]));
                enc.encode_frame(image::Frame::from_parts(buf, 0, 0, image::Delay::from_numer_denom_ms(50, 1)))
                    .unwrap();
            }
        }
        let img = decode_bytes(&gif, "gif", 4096).unwrap();
        assert_eq!(img.frames.len(), 2);
        assert_eq!(img.frames[0].delay, Duration::from_millis(50));
    }
}

/// Test material for trying the app by hand: `IV_FIXTURES=<dir> cargo test make_fixtures -- --ignored`.
#[cfg(test)]
mod fixtures {
    use image::{Rgb, RgbImage, Rgba, RgbaImage};
    use std::io::Write;
    use std::path::Path;

    fn page(w: u32, h: u32, seed: u32) -> RgbImage {
        RgbImage::from_fn(w, h, |x, y| {
            let band = (y / 200 + seed) % 6;
            let base = [[200, 60, 60], [60, 160, 90], [60, 90, 200], [220, 180, 50], [150, 60, 180], [40, 170, 170]]
                [band as usize];
            let grid = if x % 100 < 3 || y % 100 < 3 { 40 } else { 0 };
            Rgb([base[0] - grid, base[1].max(grid) - grid, base[2] - grid.min(base[2])])
        })
    }

    fn save(img: &RgbImage, p: &Path) {
        img.save(p).unwrap();
    }

    fn jpg_bytes(img: &RgbImage) -> Vec<u8> {
        let mut c = std::io::Cursor::new(Vec::new());
        img.write_to(&mut c, image::ImageFormat::Jpeg).unwrap();
        c.into_inner()
    }

    #[test]
    #[ignore]
    fn make_fixtures() {
        let root = std::path::PathBuf::from(std::env::var("IV_FIXTURES").expect("IV_FIXTURES"));
        let fm = root.join("formats");
        std::fs::create_dir_all(&fm).unwrap();
        let p = page(1600, 1100, 0);
        image::DynamicImage::ImageRgb8(p.clone()).to_rgba16().save(fm.join("pic.ff")).unwrap();
        for ext in ["png", "bmp", "tiff", "qoi", "tga", "webp"] {
            save(&p, &fm.join(format!("pic.{ext}")));
        }
        std::fs::write(fm.join("photo.jpg"), jpg_bytes(&p)).unwrap();
        // An EXIF-rotated photo (orientation 6) without touching the pixels.
        std::fs::write(
            fm.join("rotated_exif.jpg"),
            crate::actions::jpeg_rotate_for_tests(&jpg_bytes(&page(1200, 800, 2)), 1),
        )
        .unwrap();
        image::DynamicImage::ImageRgb8(p.clone()).to_rgb32f().save(fm.join("pic.exr")).unwrap();
        image::DynamicImage::ImageRgb8(p.clone()).to_rgb32f().save(fm.join("pic.hdr")).unwrap();
        RgbaImage::from_fn(900, 600, |x, y| Rgba([255, 120, 0, ((x + y) % 256) as u8]))
            .save(fm.join("alpha.png"))
            .unwrap();
        std::fs::write(fm.join("broken.jpg"), &jpg_bytes(&p)[..5000]).unwrap();
        std::fs::write(fm.join("vector.svg"), br##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 120 80"><rect width="120" height="80" fill="#246"/><circle cx="60" cy="40" r="28" fill="#fc3"/><text x="60" y="76" font-size="9" text-anchor="middle" fill="#fff">SVG text</text></svg>"##).unwrap();
        {
            let mut gif = std::fs::File::create(fm.join("anim.gif")).unwrap();
            let mut enc = image::codecs::gif::GifEncoder::new(&mut gif);
            enc.set_repeat(image::codecs::gif::Repeat::Infinite).unwrap();
            for i in 0..12u32 {
                let f = RgbaImage::from_fn(320, 240, |x, _| {
                    if (x / 20) % 12 == i { Rgba([255, 255, 255, 255]) } else { Rgba([30, 30, 90, 255]) }
                });
                enc.encode_frame(image::Frame::from_parts(f, 0, 0, image::Delay::from_numer_denom_ms(80, 1))).unwrap();
            }
        }

        // A webtoon: two chapters of strips, one of them taller than any GPU texture.
        for (ch, n) in [("Chapter 1", 8u32), ("Chapter 2", 4)] {
            let d = root.join("webtoon").join(ch);
            std::fs::create_dir_all(&d).unwrap();
            for i in 0..n {
                let h = if ch == "Chapter 1" && i == 3 { 40_000 } else { 2400 + i * 300 };
                std::fs::write(d.join(format!("{:03}.jpg", i + 1)), jpg_bytes(&page(800, h, i))).unwrap();
            }
        }

        // Comic archives.
        let comics = root.join("comics");
        std::fs::create_dir_all(&comics).unwrap();
        let pages: Vec<Vec<u8>> = (0..6).map(|i| jpg_bytes(&page(1400, 2000, i))).collect();
        let mut zip = zip::ZipWriter::new(std::fs::File::create(comics.join("Vol 1.cbz")).unwrap());
        for (i, b) in pages.iter().enumerate() {
            zip.start_file(format!("Vol 1/{}.jpg", i + 1), zip::write::SimpleFileOptions::default()).unwrap();
            zip.write_all(b).unwrap();
        }
        zip.finish().unwrap();
        let src = root.join("tmp7z");
        std::fs::create_dir_all(&src).unwrap();
        for (i, b) in pages.iter().enumerate() {
            std::fs::write(src.join(format!("p{}.jpg", i + 10)), b).unwrap();
        }
        sevenz_rust2::compress_to_path(&src, comics.join("Vol 2.cb7")).unwrap();
        std::fs::remove_dir_all(&src).unwrap();
        let mut tar = tar::Builder::new(std::fs::File::create(comics.join("Vol 3.cbt")).unwrap());
        for (i, b) in pages.iter().enumerate() {
            let mut h = tar::Header::new_gnu();
            h.set_size(b.len() as u64);
            h.set_mode(0o644);
            h.set_cksum();
            tar.append_data(&mut h, format!("{:02}.jpg", i + 1), &b[..]).unwrap();
        }
        tar.finish().unwrap();
    }
}
