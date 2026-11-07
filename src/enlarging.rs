//! Save an enlarged copy: runs the picture on screen through `crate::enlarge` on its own
//! thread and writes the result next to it. The file it came from is never touched.

use crate::actions::{can_write, write_edited};
use crate::files;
use image::RgbaImage;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering::Relaxed};
use std::sync::mpsc::{Receiver, TryRecvError, channel};

/// In `progress`: the enlarging is done and the file is being written.
const SAVING: u32 = 1000;

/// WebP stops at this many pixels a side, so anything past it is written as PNG.
const MAX_SIDE_KEEPING_FORMAT: usize = 16383;

/// A result bigger than this isn't offered: it takes 4 GB just to hold, and running out of memory
/// while writing the file would take the whole app down.
const MAX_PIXELS: u64 = 1_000_000_000;

/// Whether a picture of this size can come out of it.
pub fn fits([w, h]: [u32; 2]) -> bool {
    u64::from(w) * u64::from(h) <= MAX_PIXELS
}

pub struct Job {
    pub source: PathBuf,
    /// Thousandths done, or `SAVING`.
    progress: Arc<AtomicU32>,
    cancel: Arc<AtomicBool>,
    result: Receiver<Result<PathBuf, String>>,
}

impl Job {
    /// `doublings` is 1 for 2x, 2 for 4x. `turns` is how the picture is turned on screen.
    pub fn start(source: PathBuf, doublings: u32, turns: u8) -> Self {
        let progress = Arc::new(AtomicU32::new(0));
        let cancel = Arc::new(AtomicBool::new(false));
        let (tx, result) = channel();
        let (from, done, stop) = (source.clone(), progress.clone(), cancel.clone());
        std::thread::spawn(move || {
            let _ = tx.send(work(&from, doublings, turns, &done, &stop));
        });
        Job { source, progress, cancel, result }
    }

    pub fn finished(&self) -> Option<Result<PathBuf, String>> {
        match self.result.try_recv() {
            Ok(r) => Some(r),
            Err(TryRecvError::Disconnected) => Some(Err("it crashed".to_owned())),
            Err(TryRecvError::Empty) => None,
        }
    }

    /// 0.0 to 1.0, or `None` once it's writing the file (too late to cancel).
    pub fn progress(&self) -> Option<f32> {
        let p = self.progress.load(Relaxed);
        (p < SAVING).then_some(p as f32 / SAVING as f32)
    }

    pub fn cancel(&self) {
        self.cancel.store(true, Relaxed);
    }

    pub fn cancelled(&self) -> bool {
        self.cancel.load(Relaxed)
    }
}

fn work(source: &Path, doublings: u32, turns: u8, progress: &AtomicU32, cancel: &AtomicBool) -> Result<PathBuf, String> {
    let img = crate::edit::oriented(&crate::edit::read_pixels(source)?.0, turns);
    let (mut w, mut h) = (img.width() as usize, img.height() as usize);
    let mut rgba = img.into_raw();
    // One core left over, so the window stays smooth.
    let threads = std::thread::available_parallelism().map_or(1, |n| n.get().saturating_sub(1).max(1));
    let options = crate::enlarge::Options { threads, ..Default::default() };
    for pass in 0..doublings {
        // The second doubling has four times the pixels, so it takes four times as long.
        let (start, share) = match (doublings, pass) {
            (1, _) => (0.0, 1.0),
            (_, 0) => (0.0, 0.2),
            _ => (0.2, 0.8),
        };
        rgba = crate::enlarge::enlarge_with(&rgba, w, h, &options, &|done| {
            progress.store(((start + done * share) * 999.0) as u32, Relaxed);
            !cancel.load(Relaxed)
        })
        .map_err(|e| e.to_string())?;
        (w, h) = (w * 2, h * 2);
    }
    progress.store(SAVING, Relaxed);
    let out = RgbaImage::from_raw(w as u32, h as u32, rgba).ok_or("the picture came out too big")?;
    let target = target_for(source, 1 << doublings, w.max(h));
    write_edited(out, source, &target)?;
    Ok(target)
}

/// `photo.jpg` becomes `photo 2x.jpg`, in the same format if this app can write it, and never
/// over another file.
fn target_for(source: &Path, scale: u32, long_side: usize) -> PathBuf {
    let stem = source.file_stem().map_or("picture".into(), |s| s.to_string_lossy());
    let keep = can_write(source) && long_side <= MAX_SIDE_KEEPING_FORMAT;
    let ext = if keep { files::ext_lower(source) } else { "png".to_owned() };
    crate::frames::unique_path(source.with_file_name(format!("{stem} {scale}x.{ext}")))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, Instant};

    fn folder(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("tv-enlarging-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// Enough detail that the network has something to do, with one see-through corner.
    fn picture(w: u32, h: u32) -> RgbaImage {
        RgbaImage::from_fn(w, h, |x, y| {
            let a = if x < 8 && y < 8 { 0 } else { 255 };
            image::Rgba([(x * 7 % 256) as u8, (y * 5 % 256) as u8, ((x ^ y) % 256) as u8, a])
        })
    }

    fn wait(job: &Job) -> Result<PathBuf, String> {
        let until = Instant::now() + Duration::from_secs(60);
        loop {
            if let Some(r) = job.finished() {
                return r;
            }
            assert!(Instant::now() < until, "still running");
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    #[test]
    fn saves_next_to_the_picture_and_leaves_it_alone() {
        let dir = folder("beside");
        let source = dir.join("photo.png");
        picture(60, 40).save(&source).unwrap();
        let before = std::fs::read(&source).unwrap();

        let out = wait(&Job::start(source.clone(), 1, 0)).unwrap();
        assert_eq!(out, dir.join("photo 2x.png"));
        let got = image::open(&out).unwrap().to_rgba8();
        assert_eq!(got.dimensions(), (120, 80));
        // Alpha came along.
        assert_eq!(got.get_pixel(2, 2)[3], 0);
        assert_eq!(got.get_pixel(100, 60)[3], 255);
        assert_eq!(std::fs::read(&source).unwrap(), before);

        // A second time doesn't write over the first.
        let again = wait(&Job::start(source, 1, 0)).unwrap();
        assert_eq!(again, dir.join("photo 2x (2).png"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn four_times_keeps_the_format_and_the_turn_on_screen() {
        let dir = folder("turn");
        let source = dir.join("photo.jpg");
        image::DynamicImage::ImageRgba8(picture(60, 40)).to_rgb8().save(&source).unwrap();
        let out = wait(&Job::start(source, 2, 1)).unwrap();
        assert_eq!(out, dir.join("photo 4x.jpg"));
        assert_eq!(image::image_dimensions(&out).unwrap(), (160, 240));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn cancelling_leaves_nothing_behind() {
        let dir = folder("cancel");
        let source = dir.join("photo.png");
        picture(1500, 1500).save(&source).unwrap();
        let job = Job::start(source, 2, 0);
        job.cancel();
        assert!(wait(&job).is_err());
        assert!(job.cancelled());
        let left: Vec<_> = std::fs::read_dir(&dir).unwrap().flatten().map(|e| e.file_name()).collect();
        assert_eq!(left, ["photo.png"]);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn huge_results_are_not_offered() {
        assert!(fits([32000, 24000]));
        assert!(!fits([64000, 48000]));
        // Past what a u32 multiplication could hold.
        assert!(!fits([256000, 192000]));
    }

    #[test]
    fn formats_it_cannot_write_become_png() {
        let dir = folder("names");
        assert_eq!(target_for(&dir.join("a.heic"), 2, 100), dir.join("a 2x.png"));
        assert_eq!(target_for(&dir.join("a.gif"), 4, 100), dir.join("a 4x.png"));
        assert_eq!(target_for(&dir.join("a.webp"), 2, 100), dir.join("a 2x.webp"));
        assert_eq!(target_for(&dir.join("a.webp"), 2, 16384), dir.join("a 2x.png"));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
