//! Walks a folder on background threads and works out which pictures look alike.
//!
//! Files of the same size get their bytes hashed first, so exact copies are named as such and
//! only one of them has to be decoded. Scoring keeps everything from `MIN_SCORE` up, which lets
//! the threshold move afterwards without doing any of this again.

use super::fingerprint::{self, Fingerprint, Picture};
use std::collections::HashMap;
use std::hash::Hasher;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicU8, AtomicUsize, Ordering};
use std::sync::Arc;
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant, SystemTime};

/// Lowest similarity the slider can ask for.
pub const MIN_SCORE: f32 = 0.5;
/// Enough for the thumbnail to be averaged from plenty of pixels.
const DECODE_SIDE: u32 = 256;
/// Decoding many huge pictures at once can use gigabytes.
const MAX_THREADS: usize = 8;

pub type Decode = fn(&Path, u32) -> Result<Picture, String>;

#[derive(Clone)]
pub struct Options {
    pub folder: PathBuf,
    pub subfolders: bool,
    pub is_image: fn(&Path) -> bool,
    pub decode: Decode,
}

impl Options {
    pub fn new(folder: impl Into<PathBuf>) -> Self {
        Self { folder: folder.into(), subfolders: false, is_image: fingerprint::is_image, decode: fingerprint::decode }
    }
}

#[derive(Clone, Debug)]
pub struct FileInfo {
    pub path: PathBuf,
    pub bytes: u64,
    pub modified: Option<SystemTime>,
    /// Zero when the picture couldn't be read.
    pub size: [u32; 2],
}

#[derive(Clone, Copy, Debug)]
pub struct Pair {
    /// Index into `Report::files`. `left` is the likelier keeper: more pixels, then bigger
    /// file, then older.
    pub left: u32,
    pub right: u32,
    /// 0..1, how alike.
    pub score: f32,
    /// The two files hold the same bytes.
    pub identical: bool,
}

#[derive(Default)]
pub struct Report {
    pub files: Vec<FileInfo>,
    pub pairs: Vec<Pair>,
    /// Pictures that wouldn't decode, and why.
    pub failed: Vec<(PathBuf, String)>,
    pub took: Duration,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Stage {
    Listing,
    CheckingCopies,
    Reading,
    Comparing,
}

#[derive(Clone, Copy, Debug)]
pub struct Progress {
    pub stage: Stage,
    pub done: usize,
    pub total: usize,
}

#[derive(Default)]
struct Shared {
    stage: AtomicU8,
    done: AtomicUsize,
    total: AtomicUsize,
    cancel: AtomicBool,
}

impl Shared {
    fn enter(&self, stage: Stage, total: usize) {
        self.total.store(total, Ordering::Relaxed);
        self.done.store(0, Ordering::Relaxed);
        self.stage.store(stage as u8, Ordering::Relaxed);
    }
    fn cancelled(&self) -> bool {
        self.cancel.load(Ordering::Relaxed)
    }
}

/// A running scan. Dropping it cancels the scan.
pub struct Scan {
    shared: Arc<Shared>,
    worker: Option<JoinHandle<Option<Report>>>,
}

impl Scan {
    pub fn start(options: Options) -> Self {
        let shared = Arc::new(Shared::default());
        let for_worker = shared.clone();
        let worker = thread::Builder::new()
            .name("twins scan".into())
            .spawn(move || run(&options, &for_worker))
            .expect("couldn't start a thread");
        Self { shared, worker: Some(worker) }
    }

    pub fn progress(&self) -> Progress {
        let stage = match self.shared.stage.load(Ordering::Relaxed) {
            0 => Stage::Listing,
            1 => Stage::CheckingCopies,
            2 => Stage::Reading,
            _ => Stage::Comparing,
        };
        Progress { stage, done: self.shared.done.load(Ordering::Relaxed), total: self.shared.total.load(Ordering::Relaxed) }
    }

    pub fn cancel(&self) {
        self.shared.cancel.store(true, Ordering::Relaxed);
    }

    /// The result, once. `None` while it runs, and after a cancel.
    pub fn poll(&mut self) -> Option<Report> {
        if !self.worker.as_ref()?.is_finished() {
            return None;
        }
        self.worker.take()?.join().ok().flatten()
    }

    pub fn finished(&self) -> bool {
        self.worker.as_ref().is_none_or(|w| w.is_finished())
    }

    /// Blocks until the scan ends. For tools like `scan_cli`; a UI uses `poll`.
    #[allow(dead_code)]
    pub fn wait(mut self) -> Option<Report> {
        self.worker.take()?.join().ok().flatten()
    }
}

impl Drop for Scan {
    fn drop(&mut self) {
        self.cancel();
    }
}

fn run(options: &Options, shared: &Shared) -> Option<Report> {
    let started = Instant::now();
    let threads = thread::available_parallelism().map_or(4, |n| n.get().saturating_sub(1)).clamp(1, MAX_THREADS);

    shared.enter(Stage::Listing, 0);
    let mut files = Vec::new();
    list(&options.folder, options, &mut files, shared);
    if shared.cancelled() {
        return None;
    }

    // Bytes first: only files that share their size can be copies.
    let mut by_bytes: HashMap<u64, Vec<usize>> = HashMap::new();
    for (i, f) in files.iter().enumerate() {
        by_bytes.entry(f.bytes).or_default().push(i);
    }
    let same_size: Vec<usize> = by_bytes.into_values().filter(|g| g.len() > 1).flatten().collect();
    shared.enter(Stage::CheckingCopies, same_size.len());
    let hashes = in_parallel(&same_size, threads, shared, |&i| hash_file(&files[i].path).ok());
    if shared.cancelled() {
        return None;
    }
    // One entry per distinct content. `owner[file]` says which picture stands in for that file.
    let mut groups: HashMap<(u64, u64), usize> = HashMap::new();
    let mut owner: Vec<usize> = (0..files.len()).collect();
    for (&i, hash) in same_size.iter().zip(&hashes) {
        if let Some(Some(hash)) = hash {
            owner[i] = *groups.entry((files[i].bytes, *hash)).or_insert(i);
        }
    }

    let unique: Vec<usize> = (0..files.len()).filter(|&i| owner[i] == i).collect();
    shared.enter(Stage::Reading, unique.len());
    let decode = options.decode;
    let read = in_parallel(&unique, threads, shared, |&i| {
        let pic = decode(&files[i].path, DECODE_SIDE)?;
        let fp = Fingerprint::of(&pic).ok_or_else(|| "no pixels".to_owned())?;
        Ok::<_, String>((fp, pic.full_size))
    });
    if shared.cancelled() {
        return None;
    }

    let mut failed = Vec::new();
    let mut prints: Vec<(usize, Fingerprint)> = Vec::with_capacity(unique.len());
    for (&i, result) in unique.iter().zip(read) {
        match result {
            Some(Ok((fp, size))) => {
                files[i].size = size;
                prints.push((i, fp));
            }
            Some(Err(e)) => failed.push((files[i].path.clone(), e)),
            None => failed.push((files[i].path.clone(), "the decoder crashed on it".to_owned())),
        }
    }
    for i in 0..files.len() {
        files[i].size = files[owner[i]].size;
    }

    shared.enter(Stage::Comparing, prints.len());
    let alike = compare(&prints, threads, shared);
    if shared.cancelled() {
        return None;
    }

    // Spread the scores over the copies, and add the copies themselves.
    let mut members: HashMap<usize, Vec<usize>> = HashMap::new();
    for (i, &o) in owner.iter().enumerate() {
        members.entry(o).or_default().push(i);
    }
    let mut pairs = Vec::new();
    for group in members.values() {
        for (n, &a) in group.iter().enumerate() {
            for &b in &group[n + 1..] {
                pairs.push(pair(&files, a, b, 1.0, true));
            }
        }
    }
    for &(a, b, score) in &alike {
        for &ma in &members[&a] {
            for &mb in &members[&b] {
                pairs.push(pair(&files, ma, mb, score, false));
            }
        }
    }
    pairs.sort_by(|a, b| b.identical.cmp(&a.identical).then(b.score.total_cmp(&a.score)));

    failed.sort();
    Some(Report { files, pairs, failed, took: started.elapsed() })
}

fn list(dir: &Path, options: &Options, out: &mut Vec<FileInfo>, shared: &Shared) {
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    for entry in entries.flatten() {
        if shared.cancelled() {
            return;
        }
        let Ok(kind) = entry.file_type() else { continue };
        let path = entry.path();
        if kind.is_dir() {
            if options.subfolders {
                list(&path, options, out, shared);
            }
        } else if kind.is_file() && (options.is_image)(&path) {
            let Ok(meta) = entry.metadata() else { continue };
            out.push(FileInfo { path, bytes: meta.len(), modified: meta.modified().ok(), size: [0, 0] });
            shared.total.fetch_add(1, Ordering::Relaxed);
        }
    }
}

/// Runs `work` over `items` on `threads` threads and keeps their order. An item is `None` only
/// when the scan was cancelled before reaching it. One that panics (a decoder bug on a bad file)
/// also ends up `None` instead of taking the scan down.
fn in_parallel<T: Sync, R: Send>(items: &[T], threads: usize, shared: &Shared, work: impl Fn(&T) -> R + Sync) -> Vec<Option<R>> {
    let next = AtomicUsize::new(0);
    let results: Vec<Mutex<Option<R>>> = items.iter().map(|_| Mutex::new(None)).collect();
    thread::scope(|scope| {
        for _ in 0..threads.min(items.len()) {
            scope.spawn(|| {
                loop {
                    let i = next.fetch_add(1, Ordering::Relaxed);
                    if i >= items.len() || shared.cancelled() {
                        return;
                    }
                    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| work(&items[i])));
                    *results[i].lock().unwrap() = result.ok();
                    shared.done.fetch_add(1, Ordering::Relaxed);
                }
            });
        }
    });
    results.into_iter().map(|m| m.into_inner().unwrap()).collect()
}

fn hash_file(path: &Path) -> std::io::Result<u64> {
    let mut file = std::fs::File::open(path)?;
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    let mut buffer = vec![0u8; 1 << 20];
    loop {
        let n = file.read(&mut buffer)?;
        if n == 0 {
            return Ok(hasher.finish());
        }
        hasher.write(&buffer[..n]);
    }
}

/// Every picture against every later one. Returns (file, file, score).
fn compare(prints: &[(usize, Fingerprint)], threads: usize, shared: &Shared) -> Vec<(usize, usize, f32)> {
    let indices: Vec<usize> = (0..prints.len()).collect();
    let found = in_parallel(&indices, threads, shared, |&i| {
        let (file_a, a) = &prints[i];
        let mut hits = Vec::new();
        for (file_b, b) in &prints[i + 1..] {
            if a.might_match(b) {
                let score = a.similarity(b);
                if score >= MIN_SCORE {
                    hits.push((*file_a, *file_b, score));
                }
            }
        }
        hits
    });
    found.into_iter().flatten().flatten().collect()
}

fn pair(files: &[FileInfo], a: usize, b: usize, score: f32, identical: bool) -> Pair {
    let pixels = |f: &FileInfo| f.size[0] as u64 * f.size[1] as u64;
    let (fa, fb) = (&files[a], &files[b]);
    let order = pixels(fb)
        .cmp(&pixels(fa))
        .then(fb.bytes.cmp(&fa.bytes))
        .then(fa.modified.cmp(&fb.modified))
        .then(fa.path.cmp(&fb.path));
    let (left, right) = if order.is_gt() { (b, a) } else { (a, b) };
    Pair { left: left as u32, right: right as u32, score, identical }
}
