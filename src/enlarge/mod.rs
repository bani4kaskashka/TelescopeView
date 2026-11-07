//! Tiny native 2x image enlarger.
//!
//! Runs on the CPU of any machine: no GPU, no runtime, no files to download. The model is a
//! few KB of numbers compiled into the binary.
//!
//! ```no_run
//! # let (rgba, width, height) = (vec![0u8; 16], 2, 2);
//! let bigger = enlarge::enlarge(&rgba, width, height, &enlarge::Options::default()).unwrap();
//! assert_eq!(bigger.len(), width * 2 * height * 2 * 4);
//! ```
//!
//! This blocks until it is done, so call it from a worker thread. Use [`enlarge_with`] for a
//! progress bar and a cancel button.

// The standalone enlarger as it is, so fixes copy across: the app uses only part of its API.
#![allow(dead_code, clippy::chunks_exact_to_as_chunks, clippy::needless_range_loop)]

mod model;
mod plane;

pub use model::{Model, ModelError};

use model::Work;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Mutex, OnceLock};

/// Which method does the work.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    /// Plain math: sharp interpolation with halo control. Close to instant.
    Fast,
    /// The built-in neural network: also cleans up compression damage and blur.
    Detail,
}

#[derive(Clone, Copy, Debug)]
pub struct Options<'a> {
    pub mode: Mode,
    /// How much enhancement to add. 0 is plain interpolation, 1 is the intended look.
    pub strength: f32,
    /// Worker threads. 0 uses every core.
    pub threads: usize,
    /// Use this model instead of the built-in one (only for [`Mode::Detail`]).
    pub model: Option<&'a Model>,
}

impl Default for Options<'_> {
    fn default() -> Self {
        Options { mode: Mode::Detail, strength: 1.0, threads: 0, model: None }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Error {
    /// The buffer is not `width * height * 4` bytes, or a side is zero.
    BadSize,
    /// There isn't enough memory for the result.
    OutOfMemory,
    /// The progress callback returned false.
    Cancelled,
    /// A model failed to load.
    Model(ModelError),
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Error::BadSize => f.write_str("the buffer does not match the image size"),
            Error::OutOfMemory => f.write_str("not enough memory for the enlarged image"),
            Error::Cancelled => f.write_str("cancelled"),
            Error::Model(e) => write!(f, "bad model: {e}"),
        }
    }
}

impl std::error::Error for Error {}

/// The model compiled into this library.
pub fn builtin_model() -> &'static Model {
    static MODEL: OnceLock<Model> = OnceLock::new();
    MODEL.get_or_init(|| {
        Model::from_bytes(include_bytes!("default.enl")).expect("built-in model is valid")
    })
}

/// Enlarges straight-alpha RGBA8 pixels to twice the width and height.
pub fn enlarge(
    rgba: &[u8],
    width: usize,
    height: usize,
    options: &Options,
) -> Result<Vec<u8>, Error> {
    enlarge_with(rgba, width, height, options, &|_| true)
}

/// Same as [`enlarge`], and reports progress from 0.0 to 1.0. Return false to cancel.
///
/// The callback runs on the worker threads, so keep it quick.
pub fn enlarge_with(
    rgba: &[u8],
    width: usize,
    height: usize,
    options: &Options,
    progress: &(dyn Fn(f32) -> bool + Sync),
) -> Result<Vec<u8>, Error> {
    run(rgba, width, height, options, progress, TILE)
}

/// Input pixels per tile side. Small enough that a tile's feature maps stay in the CPU cache.
const TILE: usize = 64;

fn run(
    rgba: &[u8],
    width: usize,
    height: usize,
    options: &Options,
    progress: &(dyn Fn(f32) -> bool + Sync),
    tile: usize,
) -> Result<Vec<u8>, Error> {
    let expected = width.checked_mul(height).and_then(|n| n.checked_mul(4));
    if width == 0 || height == 0 || expected != Some(rgba.len()) {
        return Err(Error::BadSize);
    }
    let out_len = rgba.len().checked_mul(4).ok_or(Error::OutOfMemory)?;
    let mut out = Vec::new();
    out.try_reserve_exact(out_len).map_err(|_| Error::OutOfMemory)?;
    out.resize(out_len, 0u8);

    let model: Option<&Model> = match (options.mode, options.model) {
        (Mode::Fast, _) => None,
        (Mode::Detail, Some(custom)) => Some(custom),
        (Mode::Detail, None) => Some(builtin_model()),
    };
    let margin = model.map_or(plane::FAST_MARGIN, |m| m.radius().max(plane::MIN_MARGIN));

    let threads = match options.threads {
        0 => std::thread::available_parallelism().map_or(1, |n| n.get()),
        n => n,
    };
    let total = height.div_ceil(tile) * width.div_ceil(tile);
    let done = AtomicUsize::new(0);
    let cancelled = AtomicBool::new(false);

    // Output bands are handed out whole, so threads never share a byte of the result.
    let band_bytes = tile * 2 * width * 2 * 4;
    let bands = Mutex::new(out.chunks_mut(band_bytes).enumerate());
    let source = Source { rgba, width, height };

    std::thread::scope(|scope| {
        for _ in 0..threads.min(total) {
            scope.spawn(|| {
                let mut scratch = Scratch::new(tile, margin, model);
                loop {
                    let next = bands.lock().unwrap_or_else(|e| e.into_inner()).next();
                    let Some((row, band)) = next else { break };
                    let y0 = row * tile;
                    let th = tile.min(height - y0);
                    let mut x0 = 0;
                    while x0 < width {
                        if cancelled.load(Ordering::Relaxed) {
                            return;
                        }
                        let tw = tile.min(width - x0);
                        let area = Tile { x0, y0, tw, th, margin };
                        scratch.process(&source, area, band, model, options.strength);
                        x0 += tile;
                        let finished = done.fetch_add(1, Ordering::Relaxed) + 1;
                        if !progress(finished as f32 / total as f32) {
                            cancelled.store(true, Ordering::Relaxed);
                            return;
                        }
                    }
                }
            });
        }
    });

    if cancelled.load(Ordering::Relaxed) {
        return Err(Error::Cancelled);
    }
    Ok(out)
}

struct Source<'a> {
    rgba: &'a [u8],
    width: usize,
    height: usize,
}

#[derive(Clone, Copy)]
struct Tile {
    x0: usize,
    y0: usize,
    tw: usize,
    th: usize,
    margin: usize,
}

/// One thread's buffers, reused for every tile so the loop doesn't allocate.
struct Scratch {
    y: Vec<f32>,
    cb: Vec<f32>,
    cr: Vec<f32>,
    a: Vec<f32>,
    big_y: Vec<f32>,
    big_cb: Vec<f32>,
    big_cr: Vec<f32>,
    big_a: Vec<f32>,
    plain_y: Vec<f32>,
    boosted: Vec<f32>,
    temp: Vec<f32>,
    work: Work,
}

impl Scratch {
    fn new(tile: usize, margin: usize, model: Option<&Model>) -> Self {
        let extended = (tile + 2 * margin) * (tile + 2 * margin);
        let big = 4 * tile * tile;
        Scratch {
            y: vec![0.0; extended],
            cb: vec![0.0; extended],
            cr: vec![0.0; extended],
            a: vec![0.0; extended],
            big_y: vec![0.0; big],
            big_cb: vec![0.0; big],
            big_cr: vec![0.0; big],
            big_a: vec![0.0; big],
            plain_y: vec![0.0; big],
            boosted: vec![0.0; extended],
            temp: vec![0.0; 2 * tile * (tile + 4)],
            work: model.map_or_else(Work::default, |m| m.work_for(tile + 2 * margin)),
        }
    }

    fn process(
        &mut self,
        source: &Source,
        area: Tile,
        band: &mut [u8],
        model: Option<&Model>,
        strength: f32,
    ) {
        let Tile { tw, th, margin, .. } = area;
        let (ew, eh) = (tw + 2 * margin, th + 2 * margin);
        let has_alpha = self.extract(source, area, ew, eh);

        let t = &mut self.temp;
        plane::interpolate(&self.cb, ew, margin, tw, th, &mut self.big_cb, t);
        plane::interpolate(&self.cr, ew, margin, tw, th, &mut self.big_cr, t);
        if has_alpha {
            plane::interpolate(&self.a, ew, margin, tw, th, &mut self.big_a, t);
            plane::interpolate(&self.y, ew, margin, tw, th, &mut self.plain_y, t);
        }
        match model {
            Some(m) => {
                plane::interpolate(&self.y, ew, margin, tw, th, &mut self.big_y, t);
                m.add_detail(&self.y, ew, eh, margin, tw, th, strength, &mut self.big_y, &mut self.work);
            }
            None => {
                let boosted = &mut self.boosted;
                plane::sharpen(&self.y, ew, margin, tw, th, strength, &mut self.big_y, t, boosted);
            }
        }
        if has_alpha {
            // Brightness is premultiplied, so changing it where alpha is partial would shift the
            // colour of soft edges. Fade the enhancement out over them.
            for ((y, plain), a) in self.big_y.iter_mut().zip(&self.plain_y).zip(&self.big_a) {
                let w = a.clamp(0.0, 1.0).powi(4);
                *y = plain + (*y - plain) * w;
            }
        }

        // Write this tile into its place in the band.
        let out_w = source.width * 2;
        let out_x = area.x0 * 2;
        let (ow, oh) = (tw * 2, th * 2);
        for row in 0..oh {
            let start = (row * out_w + out_x) * 4;
            let dst = &mut band[start..start + ow * 4];
            let i = row * ow;
            let ys = &self.big_y[i..i + ow];
            let cbs = &self.big_cb[i..i + ow];
            let crs = &self.big_cr[i..i + ow];
            for (x, px) in dst.chunks_exact_mut(4).enumerate() {
                let alpha = if has_alpha { self.big_a[i + x].clamp(0.0, 1.0) } else { 1.0 };
                // Colors were premultiplied for filtering; undo that.
                let un = if has_alpha && alpha > 1.0 / 255.0 { 1.0 / alpha } else { 1.0 };
                let (r, g, b) = plane::to_rgb(ys[x] * un, cbs[x] * un, crs[x] * un);
                px[0] = to_byte(r);
                px[1] = to_byte(g);
                px[2] = to_byte(b);
                px[3] = to_byte(alpha);
            }
        }
    }

    /// Copies the tile plus its margin into float planes, repeating edge pixels outside the
    /// picture. Returns whether any pixel is see-through.
    fn extract(&mut self, source: &Source, area: Tile, ew: usize, eh: usize) -> bool {
        let Source { rgba, width, height } = *source;
        let m = area.margin as isize;
        let mut has_alpha = false;
        for ey in 0..eh {
            let sy = (area.y0 as isize + ey as isize - m).clamp(0, height as isize - 1) as usize;
            let row = &rgba[sy * width * 4..(sy + 1) * width * 4];
            for ex in 0..ew {
                let sx = (area.x0 as isize + ex as isize - m).clamp(0, width as isize - 1) as usize;
                let px = &row[sx * 4..sx * 4 + 4];
                let (y, cb, cr) = plane::to_ycc(px[0], px[1], px[2]);
                let a = px[3] as f32 / 255.0;
                has_alpha |= px[3] != 255;
                let i = ey * ew + ex;
                // Premultiplied, so the color under transparent pixels can't bleed in.
                self.y[i] = y * a;
                self.cb[i] = cb * a;
                self.cr[i] = cr * a;
                self.a[i] = a;
            }
        }
        has_alpha
    }
}

#[inline]
fn to_byte(v: f32) -> u8 {
    (v * 255.0 + 0.5).clamp(0.0, 255.0) as u8
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A picture with hard edges, gradients, and a little of everything in alpha.
    fn test_image(w: usize, h: usize, alpha: bool) -> Vec<u8> {
        let mut v = Vec::with_capacity(w * h * 4);
        for y in 0..h {
            for x in 0..w {
                let edge = if (x / 7 + y / 5) % 2 == 0 { 220 } else { 30 };
                v.push(edge as u8);
                v.push((x * 255 / w.max(1)) as u8);
                v.push((y * 255 / h.max(1)) as u8);
                v.push(if alpha && x > w / 2 { (y * 255 / h.max(1)) as u8 } else { 255 });
            }
        }
        v
    }

    fn opts(mode: Mode) -> Options<'static> {
        Options { mode, ..Options::default() }
    }

    #[test]
    fn output_is_twice_the_size() {
        for mode in [Mode::Fast, Mode::Detail] {
            let img = test_image(37, 91, false);
            let out = enlarge(&img, 37, 91, &opts(mode)).unwrap();
            assert_eq!(out.len(), 74 * 182 * 4);
        }
    }

    #[test]
    fn tiny_images_work() {
        for mode in [Mode::Fast, Mode::Detail] {
            for (w, h) in [(1, 1), (1, 9), (9, 1), (2, 2)] {
                let img = test_image(w, h, false);
                assert_eq!(enlarge(&img, w, h, &opts(mode)).unwrap().len(), w * h * 16);
            }
        }
    }

    #[test]
    fn rejects_a_buffer_of_the_wrong_size() {
        let opt = Options::default();
        assert_eq!(enlarge(&[0; 15], 2, 2, &opt), Err(Error::BadSize));
        assert_eq!(enlarge(&[], 0, 0, &opt), Err(Error::BadSize));
        assert_eq!(enlarge(&[0; 4], usize::MAX, 2, &opt), Err(Error::BadSize));
    }

    #[test]
    fn flat_color_stays_flat_with_fast() {
        let img: Vec<u8> = [90u8, 140, 200, 255].repeat(100 * 100);
        let out = enlarge(&img, 100, 100, &opts(Mode::Fast)).unwrap();
        for px in out.chunks_exact(4) {
            for (got, want) in px.iter().zip([90i32, 140, 200, 255]) {
                assert!((*got as i32 - want).abs() <= 1, "{px:?}");
            }
        }
    }

    #[test]
    fn tile_size_leaves_no_seams() {
        // The tile margin covers everything a pixel can see, so tile size must not matter.
        for mode in [Mode::Fast, Mode::Detail] {
            let img = test_image(150, 110, true);
            let a = run(&img, 150, 110, &opts(mode), &|_| true, 64).unwrap();
            let b = run(&img, 150, 110, &opts(mode), &|_| true, 23).unwrap();
            assert!(a == b, "{mode:?} differs between tile sizes");
        }
    }

    #[test]
    fn thread_count_gives_the_same_picture() {
        let img = test_image(150, 110, false);
        let one = enlarge(&img, 150, 110, &Options { threads: 1, ..Options::default() }).unwrap();
        let all = enlarge(&img, 150, 110, &Options::default()).unwrap();
        assert!(one == all);
    }

    #[test]
    fn opaque_stays_opaque_and_clear_stays_clear() {
        let img = test_image(100, 80, false);
        let out = enlarge(&img, 100, 80, &Options::default()).unwrap();
        assert!(out.chunks_exact(4).all(|p| p[3] == 255));

        let clear = [255u8, 0, 0, 0].repeat(80 * 60);
        let out = enlarge(&clear, 80, 60, &Options::default()).unwrap();
        assert!(out.chunks_exact(4).all(|p| p[3] == 0));
    }

    #[test]
    fn colour_under_transparency_does_not_bleed() {
        // White opaque half next to a fully clear half whose hidden colour is pure red.
        let (w, h) = (40, 40);
        let mut img = Vec::new();
        for _ in 0..h {
            for x in 0..w {
                img.extend_from_slice(if x < w / 2 { &[255, 255, 255, 255] } else { &[255, 0, 0, 0] });
            }
        }
        let out = enlarge(&img, w, h, &opts(Mode::Fast)).unwrap();
        for p in out.chunks_exact(4).filter(|p| p[3] > 32) {
            assert!(p[1] > 200 && p[2] > 200, "red bled into {p:?}");
        }
    }

    #[test]
    fn cancel_stops_the_work() {
        let img = test_image(200, 200, false);
        let r = enlarge_with(&img, 200, 200, &Options::default(), &|_| false);
        assert_eq!(r, Err(Error::Cancelled));
    }

    #[test]
    fn progress_reaches_the_end() {
        let img = test_image(200, 200, false);
        let last = Mutex::new(0.0f32);
        enlarge_with(&img, 200, 200, &Options::default(), &|p| {
            let mut l = last.lock().unwrap();
            *l = l.max(p);
            true
        })
        .unwrap();
        assert_eq!(*last.lock().unwrap(), 1.0);
    }

    #[test]
    fn strength_zero_on_detail_is_plain_interpolation() {
        // With no enhancement both modes come down to the same interpolation.
        let img = test_image(90, 90, false);
        let a = enlarge(&img, 90, 90, &Options { strength: 0.0, ..Options::default() }).unwrap();
        let b = enlarge(&img, 90, 90, &Options { strength: 0.0, mode: Mode::Fast, ..Options::default() });
        assert!(a == b.unwrap());
    }
}
