//! Videos, through ffmpeg (any format it reads). The poster frame is decoded like an image,
//! so videos sit in the folder, the thumbnails and the layout like pictures. The one on screen
//! plays: ffmpeg decodes the picture into raw RGBA and the sound into floats, the sound card
//! plays the sound and the picture follows its clock.

use eframe::egui::{self, Color32, ColorImage, TextureHandle, TextureOptions};
use std::collections::VecDeque;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::mpsc::{Receiver, TryRecvError, sync_channel};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

pub const EXTENSIONS: &[&str] = &[
    "mp4", "m4v", "mkv", "webm", "mov", "avi", "wmv", "flv", "mpg", "mpeg", "m2v", "ts", "mts", "m2ts", "3gp", "3g2",
    "ogv", "vob", "asf", "rm", "rmvb", "divx", "f4v", "mxf", "y4m", "hevc", "h265", "265", "h264", "264", "avc", "ivf",
];

/// Frames bigger than this on their long side are scaled down while decoding.
const MAX_SIDE: u32 = 2560;
/// Frame rates above this are thinned out.
const MAX_FPS: f64 = 60.0;

pub fn is_video(p: &Path) -> bool {
    let ext = crate::files::ext_lower(p);
    EXTENSIONS.contains(&ext.as_str())
}

pub(crate) fn command(program: &str) -> Command {
    let mut cmd = match bundled() {
        Some(bin) => {
            let mut cmd = Command::new(bin.join(format!("{program}{}", std::env::consts::EXE_SUFFIX)));
            libraries(&mut cmd, bin);
            cmd
        }
        None => Command::new(on_path(program)),
    };
    no_window(&mut cmd);
    cmd
}

/// The bundled ffmpeg's libraries sit in ffmpeg/lib on Linux (next to the .exe on Windows).
#[cfg(all(unix, not(target_os = "macos")))]
fn libraries(cmd: &mut Command, bin: &Path) {
    if let Some(dir) = bin.parent() {
        cmd.env("LD_LIBRARY_PATH", dir.join("lib"));
    }
}
#[cfg(not(all(unix, not(target_os = "macos"))))]
fn libraries(_: &mut Command, _: &Path) {}

/// The ffmpeg that comes with TelescopeView: `ffmpeg/bin` next to the app.
fn bundled() -> Option<&'static Path> {
    static BIN: std::sync::OnceLock<Option<PathBuf>> = std::sync::OnceLock::new();
    BIN.get_or_init(|| {
        let bin = std::env::current_exe().ok()?.parent()?.join("ffmpeg").join("bin");
        bin.join(format!("ffmpeg{}", std::env::consts::EXE_SUFFIX)).is_file().then_some(bin)
    })
    .as_deref()
}

/// An installed ffmpeg. Apps started from the Finder don't get the shell's PATH, so on a Mac
/// Homebrew's folders are tried too.
fn on_path(program: &str) -> PathBuf {
    #[cfg(target_os = "macos")]
    for dir in ["/opt/homebrew/bin", "/usr/local/bin"] {
        let p = Path::new(dir).join(program);
        if p.is_file() {
            return p;
        }
    }
    PathBuf::from(program)
}

/// No console window flashing up for each ffmpeg.
#[cfg(windows)]
fn no_window(cmd: &mut Command) {
    use std::os::windows::process::CommandExt;
    cmd.creation_flags(0x0800_0000);
}
#[cfg(not(windows))]
fn no_window(_: &mut Command) {}

pub(crate) fn ffmpeg_missing(e: std::io::Error) -> String {
    if e.kind() == std::io::ErrorKind::NotFound {
        if cfg!(target_os = "macos") {
            "Videos need ffmpeg: brew install ffmpeg".to_owned()
        } else {
            "Videos need ffmpeg: it comes with the TelescopeView download (or install it)".to_owned()
        }
    } else {
        e.to_string()
    }
}

/// One frame from about a second in (the very start is often black), as PNG bytes.
pub fn poster(path: &Path) -> Result<Vec<u8>, String> {
    // The very first frame: playback starts on the same picture, so nothing jumps.
    let out = command("ffmpeg")
        .args(["-v", "error", "-i"])
        .arg(path)
        .args(["-frames:v", "1", "-f", "image2pipe", "-c:v", "png", "-"])
        .stdin(Stdio::null())
        .stderr(Stdio::piped())
        .output()
        .map_err(ffmpeg_missing)?;
    if out.stdout.is_empty() {
        let err = String::from_utf8_lossy(&out.stderr);
        return Err(err.lines().last().unwrap_or("not a video ffmpeg can read").to_owned());
    }
    Ok(out.stdout)
}

/// Pictures ffmpeg can read when the system has no decoder for them (HEIC on a PC without
/// Microsoft's HEIF extension, or on Linux).
pub const STILLS: &[&str] = &["heic", "heif", "hif", "avif"];

/// Decodes a picture with ffmpeg, returned as PNG bytes. Goes through a temporary file, since
/// HEIF keeps its picture's parts at offsets that need seeking.
pub fn still(bytes: &[u8], ext: &str) -> Result<Vec<u8>, String> {
    static NEXT: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
    let n = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let tmp = std::env::temp_dir().join(format!("telescopeview-{}-{n}.{ext}", std::process::id()));
    std::fs::write(&tmp, bytes).map_err(|e| e.to_string())?;
    let out = command("ffmpeg")
        .args(["-v", "error", "-i"])
        .arg(&tmp)
        .args(["-frames:v", "1", "-f", "image2pipe", "-c:v", "png", "-"])
        .stdin(Stdio::null())
        .stderr(Stdio::piped())
        .output();
    let _ = std::fs::remove_file(&tmp);
    let out = out.map_err(ffmpeg_missing)?;
    if out.stdout.is_empty() {
        let err = String::from_utf8_lossy(&out.stderr);
        return Err(err.lines().last().unwrap_or("ffmpeg couldn't read it").to_owned());
    }
    Ok(out.stdout)
}

pub(crate) struct Info {
    /// Decoded frame size (turned like the phone held it, scaled to `MAX_SIDE`).
    size: [usize; 2],
    fps: f64,
    pub(crate) duration: f64,
    audio: bool,
    pub(crate) source: Source,
}

/// What the file itself says, before the limits playback puts on it.
pub(crate) struct Source {
    /// Turned like the phone held it.
    pub(crate) size: [u32; 2],
    pub(crate) fps: f64,
    /// 0 when the file doesn't say.
    pub(crate) frames: u64,
    pub(crate) codec: String,
}

pub(crate) fn probe(path: &Path) -> Result<Info, String> {
    let out = command("ffprobe")
        .args(["-v", "error", "-show_entries"])
        .arg("stream=codec_name,codec_type,width,height,avg_frame_rate,r_frame_rate,nb_frames:stream_side_data=rotation:format=duration")
        .args(["-of", "default=nw=1"])
        .arg(path)
        .stdin(Stdio::null())
        .output()
        .map_err(ffmpeg_missing)?;
    let text = String::from_utf8_lossy(&out.stdout);
    let (mut w, mut h, mut duration, mut rotation, mut audio) = (0u32, 0u32, 0.0, 0i32, false);
    let (mut avg, mut base) = (0.0, 0.0);
    let (mut frames, mut name, mut codec) = (0, "", String::new());
    let mut in_video = false;
    let rate = |v: &str| match v.split_once('/') {
        Some((a, b)) => a.parse::<f64>().unwrap_or(0.0) / b.parse::<f64>().unwrap_or(0.0).max(1.0),
        None => v.parse().unwrap_or(0.0),
    };
    for line in text.lines() {
        let Some((k, v)) = line.split_once('=') else { continue };
        match k {
            // ffprobe prints the name first, before saying what kind of stream this is.
            "codec_name" => name = v,
            "codec_type" => {
                in_video = v == "video" && w == 0;
                audio |= v == "audio";
                if in_video {
                    codec = name.to_owned();
                }
            }
            "nb_frames" if in_video => frames = v.parse().unwrap_or(0),
            "width" if in_video => w = v.parse().unwrap_or(0),
            "height" if in_video => h = v.parse().unwrap_or(0),
            "avg_frame_rate" if in_video => avg = rate(v),
            "r_frame_rate" if in_video => base = rate(v),
            "rotation" if in_video => rotation = v.parse::<f64>().unwrap_or(0.0) as i32,
            "duration" => duration = v.parse().unwrap_or(duration),
            _ => {}
        }
    }
    if w == 0 || h == 0 {
        return Err("no picture in this file".to_owned());
    }
    if rotation.rem_euclid(180) == 90 {
        std::mem::swap(&mut w, &mut h);
    }
    let scale = (MAX_SIDE as f64 / w.max(h) as f64).min(1.0);
    // Even sizes: some pixel formats won't scale to odd ones.
    let even = |v: u32| (((v as f64 * scale) as u32).max(2) & !1) as usize;
    // A bare stream (.hevc, .h264) has no length, and its average rate is a guess.
    let fps = if duration > 0.0 && avg > 1.0 { avg } else if base > 1.0 { base } else { avg };
    let fps = if fps.is_finite() && fps > 1.0 { fps } else { 30.0 };
    let source = Source { size: [w, h], fps, frames, codec };
    Ok(Info { size: [even(w), even(h)], fps: fps.min(MAX_FPS), duration, audio, source })
}

/// The sound card side: a queue of samples ffmpeg fills and the device drains.
struct Sound {
    queue: Arc<Mutex<VecDeque<f32>>>,
    /// Sample frames the device has played since the last seek.
    played: Arc<AtomicU64>,
    paused: Arc<AtomicBool>,
    volume: Arc<AtomicU32>,
    /// ffmpeg ran out (end of the file).
    done: Arc<AtomicBool>,
    rate: u32,
    channels: usize,
    /// The device stream lives on a thread of its own (it can't move between threads); it
    /// closes when this goes.
    _keep: std::sync::mpsc::Sender<()>,
}

impl Sound {
    fn open(volume: f32, paused: bool) -> Option<Self> {
        let queue = Arc::new(Mutex::new(VecDeque::new()));
        let played = Arc::new(AtomicU64::new(0));
        let paused = Arc::new(AtomicBool::new(paused));
        let vol = Arc::new(AtomicU32::new(volume.to_bits()));
        let parts = (queue.clone(), played.clone(), paused.clone(), vol.clone());
        let (ready_tx, ready) = sync_channel(1);
        let (keep, hold) = std::sync::mpsc::channel::<()>();
        std::thread::Builder::new()
            .name("sound".into())
            .spawn(move || {
                let Some((stream, rate, channels)) = Self::stream(parts) else {
                    let _ = ready_tx.send(None);
                    return;
                };
                let _ = ready_tx.send(Some((rate, channels)));
                let _ = hold.recv(); // Until the Sound goes.
                drop(stream);
            })
            .ok()?;
        let (rate, channels) = ready.recv().ok()??;
        Some(Self { queue, played, paused, volume: vol, done: Arc::new(AtomicBool::new(false)), rate, channels, _keep: keep })
    }

    #[allow(clippy::type_complexity)]
    fn stream(
        (q, p, pa, v): (Arc<Mutex<VecDeque<f32>>>, Arc<AtomicU64>, Arc<AtomicBool>, Arc<AtomicU32>),
    ) -> Option<(cpal::Stream, u32, usize)> {
        use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
        let device = cpal::default_host().default_output_device()?;
        let config = device.default_output_config().ok()?;
        let format = config.sample_format();
        let config = config.config();
        let channels = config.channels as usize;
        // Fills `out` with what's queued (silence when paused or starved) and counts the
        // frames really played: that's the clock the picture follows.
        let fill = move |out: &mut dyn FnMut(usize, f32), len: usize| {
            let paused = pa.load(Ordering::Relaxed);
            let volume = f32::from_bits(v.load(Ordering::Relaxed));
            let mut q = q.lock().unwrap_or_else(|e| e.into_inner());
            let take = if paused { 0 } else { (q.len().min(len) / channels) * channels };
            for (i, s) in q.drain(..take).enumerate() {
                out(i, s * volume);
            }
            for i in take..len {
                out(i, 0.0);
            }
            p.fetch_add((take / channels) as u64, Ordering::Relaxed);
        };
        let err = |e| eprintln!("audio: {e}");
        let stream = match format {
            cpal::SampleFormat::F32 => device
                .build_output_stream(
                    config,
                    move |data: &mut [f32], _: &_| {
                        let len = data.len();
                        fill(&mut |i, s| data[i] = s, len)
                    },
                    err,
                    None,
                )
                .ok()?,
            cpal::SampleFormat::I16 => device
                .build_output_stream(
                    config,
                    move |data: &mut [i16], _: &_| {
                        let len = data.len();
                        fill(&mut |i, s| data[i] = (s.clamp(-1.0, 1.0) * 32767.0) as i16, len)
                    },
                    err,
                    None,
                )
                .ok()?,
            _ => return None,
        };
        stream.play().ok()?;
        Some((stream, config.sample_rate, channels))
    }

    /// Seconds played since the last seek.
    fn played(&self) -> f64 {
        self.played.load(Ordering::Relaxed) as f64 / self.rate as f64
    }

    /// Still playing (more queued, or ffmpeg still going).
    fn live(&self) -> bool {
        !self.done.load(Ordering::Relaxed) || !self.queue.lock().unwrap_or_else(|e| e.into_inner()).is_empty()
    }
}

pub struct Player {
    pub path: PathBuf,
    info: Info,
    sound: Option<Sound>,
    /// Where the last seek landed, and since when the wall clock counts (no or finished sound).
    seek_at: f64,
    anchor: (Instant, f64),
    pub paused: bool,
    pub ended: bool,
    frames: Receiver<(f64, Vec<Color32>)>,
    next: Option<(f64, Vec<Color32>)>,
    pub texture: Option<TextureHandle>,
    /// The frame on screen.
    pub frame: Option<Arc<ColorImage>>,
    children: Vec<Arc<Mutex<Child>>>,
    /// Bumped on every seek, so old reader threads stop.
    generation: Arc<AtomicU64>,
    pub error: Option<String>,
}

/// A player being started on a thread of its own (ffprobe and the sound device take a moment).
pub struct Opening {
    pub path: PathBuf,
    rx: Receiver<Result<Player, String>>,
    pub result: Option<Result<Player, String>>,
}

impl Opening {
    /// `paused`: ready and waiting on its first frame (a neighbour, opened ahead).
    pub fn new(path: &Path, volume: f32, paused: bool) -> Self {
        let (tx, rx) = sync_channel(1);
        let p = path.to_path_buf();
        std::thread::Builder::new()
            .name("video-open".into())
            .spawn(move || {
                let _ = tx.send(Player::start(&p, volume, paused));
            })
            .ok();
        Self { path: path.to_path_buf(), rx, result: None }
    }

    fn poll(&mut self) {
        if self.result.is_none() {
            match self.rx.try_recv() {
                Ok(r) => self.result = Some(r),
                Err(TryRecvError::Disconnected) => self.result = Some(Err("couldn't start".to_owned())),
                Err(TryRecvError::Empty) => {}
            }
        }
    }
}

impl Player {
    fn start(path: &Path, volume: f32, paused: bool) -> Result<Self, String> {
        let info = probe(path)?;
        let sound = if info.audio { Sound::open(volume, paused) } else { None };
        let (_, frames) = sync_channel(1);
        let mut player = Self {
            path: path.to_path_buf(),
            info,
            sound,
            seek_at: 0.0,
            anchor: (Instant::now(), 0.0),
            paused,
            ended: false,
            frames,
            next: None,
            texture: None,
            frame: None,
            children: Vec::new(),
            generation: Arc::new(AtomicU64::new(0)),
            error: None,
        };
        player.seek(0.0);
        Ok(player)
    }

    pub fn duration(&self) -> f64 {
        self.info.duration
    }

    /// Where playback is, in seconds.
    pub fn position(&self) -> f64 {
        if self.paused {
            return self.anchor.1;
        }
        match &self.sound {
            Some(s) if s.live() => self.seek_at + s.played(),
            _ => self.anchor.1 + self.anchor.0.elapsed().as_secs_f64(),
        }
    }

    pub fn set_volume(&self, volume: f32) {
        if let Some(s) = &self.sound {
            s.volume.store(volume.to_bits(), Ordering::Relaxed);
        }
    }

    /// Plays one that was opened ahead, paused.
    fn play(&mut self) {
        if self.paused && !self.ended {
            self.toggle_pause();
        }
    }

    pub fn pause(&mut self) {
        if !self.paused && !self.ended {
            self.toggle_pause();
        }
    }

    pub fn toggle_pause(&mut self) {
        if self.ended {
            self.ended = false;
            self.paused = false;
            self.seek(0.0);
            return;
        }
        let at = self.position();
        self.paused = !self.paused;
        self.anchor = (Instant::now(), at);
        if let Some(s) = &self.sound {
            s.paused.store(self.paused, Ordering::Relaxed);
            // The sound clock carries on from the same spot.
            if !self.paused {
                self.seek_at = at - s.played();
            }
        }
    }

    /// Starts ffmpeg again at `t` seconds (both picture and sound).
    pub fn seek(&mut self, t: f64) {
        let t = t.clamp(0.0, (self.info.duration - 0.05).max(0.0));
        self.stop_children();
        let generation = self.generation.fetch_add(1, Ordering::Relaxed) + 1;
        self.seek_at = t;
        self.anchor = (Instant::now(), t);
        self.next = None;
        self.ended = false;

        let [w, h] = self.info.size;
        let fps = self.info.fps;
        // A bare stream can't seek at all, not even to the start.
        let from = |cmd: &mut Command| {
            if t > 0.0 {
                cmd.args(["-ss", &format!("{t:.3}")]);
            }
        };
        let mut video = command("ffmpeg");
        video.args(["-v", "error", "-hwaccel", "auto"]);
        from(&mut video);
        video
            .arg("-i")
            .arg(&self.path)
            .args(["-map", "0:v:0", "-vf", &format!("scale={w}:{h}"), "-fps_mode", "cfr"])
            .args(["-r", &format!("{fps:.4}"), "-pix_fmt", "rgba", "-f", "rawvideo", "-"]);
        let (tx, rx) = sync_channel(3);
        self.frames = rx;
        match spawn(&mut video) {
            Ok(child) => {
                let child = Arc::new(Mutex::new(child));
                self.children.push(child.clone());
                let gen_now = self.generation.clone();
                std::thread::Builder::new()
                    .name("video".into())
                    .spawn(move || {
                        let Some(mut out) = child.lock().unwrap_or_else(|e| e.into_inner()).stdout.take() else {
                            return;
                        };
                        for n in 0.. {
                            let mut px = vec![Color32::TRANSPARENT; w * h];
                            if out.read_exact(bytemuck::cast_slice_mut(&mut px)).is_err()
                                || gen_now.load(Ordering::Relaxed) != generation
                                || tx.send((t + n as f64 / fps, px)).is_err()
                            {
                                return;
                            }
                        }
                    })
                    .ok();
            }
            Err(e) => self.error = Some(e),
        }

        if let Some(sound) = &mut self.sound {
            sound.queue.lock().unwrap_or_else(|e| e.into_inner()).clear();
            sound.played.store(0, Ordering::Relaxed);
            sound.done.store(false, Ordering::Relaxed);
            sound.paused.store(self.paused, Ordering::Relaxed);
            let mut audio = command("ffmpeg");
            audio.args(["-v", "error"]);
            from(&mut audio);
            audio
                .arg("-i")
                .arg(&self.path)
                .args(["-map", "0:a:0", "-ac", &sound.channels.to_string(), "-ar", &sound.rate.to_string()])
                .args(["-f", "f32le", "-"]);
            if let Ok(child) = spawn(&mut audio) {
                let child = Arc::new(Mutex::new(child));
                self.children.push(child.clone());
                let (queue, done, gen_now) = (sound.queue.clone(), sound.done.clone(), self.generation.clone());
                // About a second queued ahead.
                let ahead = (sound.rate as usize) * sound.channels;
                std::thread::Builder::new()
                    .name("audio".into())
                    .spawn(move || {
                        let Some(mut out) = child.lock().unwrap_or_else(|e| e.into_inner()).stdout.take() else {
                            return;
                        };
                        let mut buf = vec![0f32; 4096];
                        loop {
                            if gen_now.load(Ordering::Relaxed) != generation {
                                return;
                            }
                            if queue.lock().unwrap_or_else(|e| e.into_inner()).len() > ahead {
                                std::thread::sleep(Duration::from_millis(20));
                                continue;
                            }
                            match read_some(&mut out, bytemuck::cast_slice_mut(&mut buf)) {
                                Some(n) if n > 0 => {
                                    queue.lock().unwrap_or_else(|e| e.into_inner()).extend(&buf[..n / 4]);
                                }
                                _ => break,
                            }
                        }
                        if gen_now.load(Ordering::Relaxed) == generation {
                            done.store(true, Ordering::Relaxed);
                        }
                    })
                    .ok();
            }
        }
    }

    /// Shows the frame that's due and says when the next one is. Call every frame.
    pub fn tick(&mut self, ctx: &egui::Context) {
        let now = self.position();
        if !self.paused
            && let Some(s) = &self.sound
            && s.live()
        {
            self.anchor = (Instant::now(), now);
        }
        let mut shown = None;
        loop {
            if self.next.is_none() {
                match self.frames.try_recv() {
                    Ok(f) => self.next = Some(f),
                    Err(TryRecvError::Empty) => break,
                    Err(TryRecvError::Disconnected) => {
                        // Every frame shown: stop on the last one.
                        if !self.paused {
                            self.ended = true;
                            let end = if self.info.duration > 0.0 { now.min(self.info.duration) } else { now };
                            self.anchor = (Instant::now(), end);
                            self.paused = true;
                            if let Some(s) = &self.sound {
                                s.paused.store(true, Ordering::Relaxed);
                            }
                        }
                        break;
                    }
                }
            }
            // Frames carry their time; after a seek (even paused) the first one is due at once.
            match &self.next {
                Some((pts, _)) if *pts <= now + 0.005 || self.texture.is_none() => shown = self.next.take(),
                _ => break,
            }
        }
        if let Some((_, px)) = shown {
            // Shared with the texture, not copied: kept for Select text on a paused frame.
            let image = Arc::new(ColorImage::new(self.info.size, px));
            self.frame = Some(image.clone());
            match &mut self.texture {
                Some(t) => t.set(image, TextureOptions::LINEAR),
                None => self.texture = Some(ctx.load_texture("video", image, TextureOptions::LINEAR)),
            }
        }
        if !self.paused {
            let wait = self.next.as_ref().map_or(0.004, |(pts, _)| (pts - now).clamp(0.001, 0.05));
            ctx.request_repaint_after(Duration::from_secs_f64(wait));
        }
    }

    fn stop_children(&mut self) {
        for c in self.children.drain(..) {
            let _ = c.lock().unwrap_or_else(|e| e.into_inner()).kill();
        }
    }
}

impl Drop for Player {
    fn drop(&mut self) {
        self.generation.fetch_add(1, Ordering::Relaxed);
        self.stop_children();
    }
}

fn spawn(cmd: &mut Command) -> Result<Child, String> {
    cmd.stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::null()).spawn().map_err(ffmpeg_missing)
}

/// Reads whole samples (a multiple of 4 bytes) or nothing at the end.
fn read_some(r: &mut impl Read, buf: &mut [u8]) -> Option<usize> {
    let mut n = r.read(buf).ok()?;
    while n % 4 != 0 {
        let m = r.read(&mut buf[n..]).ok()?;
        if m == 0 {
            return Some(n - n % 4);
        }
        n += m;
    }
    Some(n)
}

impl crate::app::App {
    fn volume(&self) -> f32 {
        if self.settings.muted { 0.0 } else { self.settings.volume }
    }

    /// Starts the video that came on screen (and stops one that left), and opens the videos
    /// either side ahead of time, paused on their first frame. Call every frame.
    pub fn sync_video(&mut self, ctx: &egui::Context) {
        let playable = |p: &PathBuf| is_video(p) && crate::files::archive_of(p).is_none();
        let want = self.current().filter(|p| playable(p) && !self.strip).cloned();
        if self.video.as_ref().map(|v| &v.path) != want.as_ref() {
            self.video = None;
            self.video_drag = None;
        }
        if self.current().is_none_or(|p| self.video_tried.as_ref() != Some(p)) {
            self.video_tried = None;
        }
        let volume = self.volume();
        if let Some(w) = want.as_ref().filter(|w| self.video.is_none() && self.video_tried.as_ref() != Some(*w)) {
            let at = match self.video_opening.iter().position(|o| o.path == *w) {
                Some(i) => i,
                None => {
                    self.video_opening.push(Opening::new(w, volume, false));
                    self.video_opening.len() - 1
                }
            };
            self.video_opening[at].poll();
            if self.video_opening[at].result.is_some() {
                let opened = self.video_opening.remove(at).result;
                self.video_tried = Some(w.clone());
                match opened {
                    Some(Ok(mut p)) => {
                        p.set_volume(volume);
                        p.play();
                        self.video = Some(p);
                    }
                    Some(Err(e)) => self.toast(format!("Can't play {}: {e}", crate::files::file_name(w))),
                    None => {}
                }
            } else {
                ctx.request_repaint_after(Duration::from_millis(30));
            }
        }
        // The neighbours, ready to go.
        let mut ahead = Vec::new();
        if !self.strip {
            for i in [self.index + 1, self.index.wrapping_sub(1)] {
                if let Some(p) = self.files.get(i).filter(|p| playable(p)) {
                    ahead.push(p.clone());
                }
            }
        }
        self.video_opening.retain(|o| want.as_ref() == Some(&o.path) || ahead.contains(&o.path));
        for p in ahead {
            if !self.video_opening.iter().any(|o| o.path == p) {
                self.video_opening.push(Opening::new(&p, volume, true));
            }
        }
        for o in &mut self.video_opening {
            o.poll();
        }
        if let Some(v) = &mut self.video {
            v.tick(ctx);
        }
    }

    /// The video on screen is still starting.
    pub fn video_starting(&self) -> bool {
        self.current()
            .is_some_and(|p| self.video_opening.iter().any(|o| o.path == *p && o.result.is_none()) && self.video.is_none())
    }

    /// Play / pause, the time, a seek bar and the sound, along the bottom of the picture.
    /// Shown while paused or while the mouse moves.
    pub fn video_controls(&mut self, ctx: &egui::Context, rect: egui::Rect) {
        use crate::theme;
        use egui::{Pos2, Rect, RichText, Sense, Vec2};
        let Some(v) = &self.video else { return };
        let (paused, duration, position) = (v.paused, v.duration(), v.position());
        let near = ctx.input(|i| i.pointer.hover_pos()).is_some_and(|p| p.y > rect.bottom() - 90.0 && rect.contains(p));
        let recent = self.last_activity.elapsed() < Duration::from_millis(2500);
        if !(paused || recent || near || self.video_drag.is_some()) {
            return;
        }
        if !paused {
            ctx.request_repaint_after(Duration::from_millis(2600));
        }
        let width = (rect.width() - 32.0).min(900.0);
        let at = Pos2::new(rect.center().x - width / 2.0, rect.bottom() - 16.0 - 40.0);
        let (mut toggle, mut seek, mut mute) = (false, None, false);
        let mut volume = self.settings.volume;
        let (play_icon, pause_icon) = (self.icons.play.clone(), self.icons.pause.clone());
        let sound_icon = if self.settings.muted { self.icons.mute.clone() } else { self.icons.volume.clone() };
        egui::Area::new(egui::Id::new("video-controls")).fixed_pos(at).order(egui::Order::Foreground).show(ctx, |ui| {
            egui::Frame::NONE.fill(theme::bar_overlay()).show(ui, |ui| {
                ui.set_width(width);
                ui.spacing_mut().item_spacing.x = 6.0;
                ui.horizontal(|ui| {
                    ui.set_height(40.0);
                    let (icon, tip) =
                        if paused { (&play_icon, "Play  (Space, K)") } else { (&pause_icon, "Pause  (Space, K)") };
                    toggle = crate::ui::icon_button(ui, icon, tip, false, true).clicked();
                    let shown = self.video_drag.unwrap_or(position);
                    let time = if duration > 0.0 {
                        format!("{} / {}", clock(shown), clock(duration))
                    } else {
                        clock(shown)
                    };
                    ui.label(RichText::new(time).monospace().color(theme::text_dim()));
                    // The seek bar takes what's left, minus room for the sound.
                    let bar_w = (ui.available_width() - 34.0 - 80.0 - 18.0).max(40.0);
                    let (bar, resp) = ui.allocate_exact_size(Vec2::new(bar_w, 40.0), Sense::click_and_drag());
                    let track = bar.shrink2(Vec2::new(4.0, 0.0));
                    let y = bar.center().y;
                    let hot = resp.hovered() || resp.dragged();
                    let thick = if hot { 4.0 } else { 2.0 };
                    let frac = |t: f64| if duration > 0.0 { (t / duration).clamp(0.0, 1.0) as f32 } else { 0.0 };
                    let p = ui.painter();
                    p.rect_filled(
                        Rect::from_x_y_ranges(track.x_range(), y - thick / 2.0..=y + thick / 2.0),
                        0.0,
                        theme::line(),
                    );
                    let x = track.left() + track.width() * frac(shown);
                    p.rect_filled(
                        Rect::from_x_y_ranges(track.left()..=x, y - thick / 2.0..=y + thick / 2.0),
                        0.0,
                        theme::progress(),
                    );
                    p.rect_filled(Rect::from_center_size(Pos2::new(x, y), Vec2::new(3.0, 12.0)), 0.0, theme::text());
                    let t_at = |px: f32| ((px - track.left()) / track.width()).clamp(0.0, 1.0) as f64 * duration;
                    if let Some(pos) = resp.interact_pointer_pos() {
                        if resp.dragged() {
                            self.video_drag = Some(t_at(pos.x));
                        }
                        if resp.clicked() {
                            seek = Some(t_at(pos.x));
                        }
                    }
                    if resp.drag_stopped() {
                        seek = self.video_drag.take();
                    }
                    if hot && let Some(h) = resp.hover_pos() {
                        resp.on_hover_text_at_pointer(clock(t_at(h.x)));
                    }
                    let tip = if self.settings.muted { "Sound on" } else { "Mute" };
                    mute = crate::ui::icon_button(ui, &sound_icon, tip, false, true).clicked();
                    ui.spacing_mut().slider_width = 80.0;
                    let slider = egui::Slider::new(&mut volume, 0.0..=1.0)
                        .show_value(false)
                        .trailing_fill(true)
                        .handle_shape(egui::style::HandleShape::Rect { aspect_ratio: 0.5 });
                    ui.add(slider).on_hover_text("Volume");
                });
            });
        });
        if mute {
            self.settings.muted = !self.settings.muted;
        }
        if volume != self.settings.volume {
            self.settings.volume = volume;
            self.settings.muted = false;
        }
        let vol = self.volume();
        if let Some(v) = &mut self.video {
            v.set_volume(vol);
            if toggle {
                v.toggle_pause();
            }
            if let Some(t) = seek {
                v.seek(t);
            }
        }
    }
}

/// "1:05" or "1:02:05".
pub fn clock(t: f64) -> String {
    let t = t.max(0.0) as u64;
    let (h, m, s) = (t / 3600, t / 60 % 60, t % 60);
    if h > 0 { format!("{h}:{m:02}:{s:02}") } else { format!("{m}:{s:02}") }
}

#[cfg(test)]
mod tests {
    #[test]
    fn clock_text() {
        assert_eq!(super::clock(65.4), "1:05");
        assert_eq!(super::clock(3725.0), "1:02:05");
    }
}
