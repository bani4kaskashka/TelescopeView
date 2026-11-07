//! Pick a frame: splits a video into every frame as a grid over the whole window, then saves
//! the one you pick at full resolution.
//!
//! One ffmpeg pass streams a small JPEG per frame plus each frame's exact time (the `showinfo`
//! filter). The full frame comes later: a seek to a little before it, then the first frame past
//! the halfway point between it and the one before. That's exactly that frame, even in variable
//! frame rate video and with B-frames around a keyframe.

use crate::theme;
use crate::video::{self, Info, clock};
use eframe::egui::{
    self, Align, Align2, ColorImage, Event, FontId, Key, Layout, Rect, RichText, Sense, Stroke, TextureHandle,
    TextureOptions, Vec2,
};
use image::RgbImage;
use std::collections::{HashMap, VecDeque};
use std::io::{BufRead, BufReader, Read};
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, Sender, channel};
use std::time::{Duration, Instant};

/// Width the thumbnails are decoded at. The grid shows them at this size or smaller.
const THUMB_WIDTH: u32 = 320;
const TEXTURE_LIMIT: usize = 800;
const COMMAND: &str = if cfg!(target_os = "macos") { "Cmd" } else { "Ctrl" };
/// How many frames before the wanted one to seek to. No codec shows a frame more than this far
/// from where it's stored. MP4 indexes keyframes by storage order, so a seek right next to a
/// frame can land on the keyframe it needs to come after, and lose it.
const REORDER: usize = 16;
/// Seconds to back up by when a seek missed.
const FURTHER_BACK: f64 = 5.0;

enum Extract {
    Info(Info),
    /// The next frame's thumbnail, as JPEG. Frames arrive in order.
    Thumb(Vec<u8>),
    /// The time of frame `n`, in seconds.
    Time(usize, f64),
    Finished(Result<(), String>),
}

fn spawn_extract(path: PathBuf, tx: Sender<Extract>, cancel: Arc<AtomicBool>, ctx: egui::Context) {
    std::thread::spawn(move || {
        let result = video::probe(&path).and_then(|info| {
            let _ = tx.send(Extract::Info(info));
            extract(&path, THUMB_WIDTH, &tx, &cancel)
        });
        let _ = tx.send(Extract::Finished(result));
        ctx.request_repaint();
    });
}

fn extract(path: &Path, width: u32, tx: &Sender<Extract>, cancel: &AtomicBool) -> Result<(), String> {
    let mut child = video::command("ffmpeg")
        .args(["-hide_banner", "-nostdin", "-nostats", "-loglevel", "info", "-i"])
        .arg(path)
        .args(["-map", "0:v:0", "-an", "-sn", "-dn"])
        .args(["-vf", &format!("scale={width}:-2:flags=bilinear,showinfo")])
        // Passthrough: one picture per decoded frame, never doubled or dropped.
        .args(["-fps_mode", "passthrough"])
        .args(["-c:v", "mjpeg", "-q:v", "5", "-pix_fmt", "yuvj420p", "-f", "image2pipe", "-"])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(video::ffmpeg_missing)?;

    let stderr = child.stderr.take().ok_or("ffmpeg has no log")?;
    let times = tx.clone();
    let log = std::thread::spawn(move || {
        let mut tail = VecDeque::new();
        for line in BufReader::new(stderr).split(b'\n').map_while(Result::ok) {
            let line = String::from_utf8_lossy(&line);
            if let Some((n, t)) = parse_showinfo(&line) {
                let _ = times.send(Extract::Time(n, t));
            } else if !line.contains("Parsed_showinfo") {
                tail.push_back(line.trim().to_owned());
                if tail.len() > 8 {
                    tail.pop_front();
                }
            }
        }
        tail
    });

    let mut stdout = child.stdout.take().ok_or("ffmpeg has no output")?;
    let mut buf = Vec::with_capacity(1 << 20);
    let mut chunk = vec![0u8; 1 << 16];
    let mut failed = None;
    'read: loop {
        if cancel.load(Ordering::Relaxed) {
            let _ = child.kill();
            break;
        }
        let k = match stdout.read(&mut chunk) {
            Ok(0) => break,
            Ok(k) => k,
            Err(e) => {
                failed = Some(e.to_string());
                break;
            }
        };
        buf.extend_from_slice(&chunk[..k]);
        loop {
            match jpeg_len(&buf) {
                Ok(Some(len)) => {
                    if tx.send(Extract::Thumb(buf.drain(..len).collect())).is_err() {
                        let _ = child.kill();
                        break 'read;
                    }
                }
                Ok(None) => break,
                Err(e) => {
                    failed = Some(e);
                    let _ = child.kill();
                    break 'read;
                }
            }
        }
    }

    let status = child.wait().map_err(|e| e.to_string())?;
    let tail = log.join().unwrap_or_default();
    if cancel.load(Ordering::Relaxed) {
        return Ok(());
    }
    if let Some(e) = failed {
        return Err(e);
    }
    if !status.success() {
        let lines: Vec<_> = tail.into_iter().filter(|l| !l.is_empty()).collect();
        return Err(lines.last().cloned().unwrap_or_else(|| "ffmpeg failed".to_owned()));
    }
    Ok(())
}

/// `[Parsed_showinfo_1 @ 000001] n:  12 pts: 6144 pts_time:0.48 duration: ...`
fn parse_showinfo(line: &str) -> Option<(usize, f64)> {
    if !line.contains("Parsed_showinfo") {
        return None;
    }
    let after_n = &line[line.find("] n:")? + 4..];
    let n = after_n.split_whitespace().next()?.parse().ok()?;
    let after_t = &line[line.find("pts_time:")? + 9..];
    let t = after_t.split_whitespace().next()?.parse().ok()?;
    Some((n, t))
}

/// Length of the complete JPEG at the start of `b`, or `None` if more bytes are needed.
fn jpeg_len(b: &[u8]) -> Result<Option<usize>, String> {
    let bad = || "unexpected data in ffmpeg's output".to_owned();
    if b.len() < 2 {
        return Ok(None);
    }
    if b[0] != 0xFF || b[1] != 0xD8 {
        return Err(bad());
    }
    // Walk the header segments up to start-of-scan; their payloads can contain any byte.
    let mut i = 2;
    loop {
        if i + 4 > b.len() {
            return Ok(None);
        }
        if b[i] != 0xFF {
            return Err(bad());
        }
        let marker = b[i + 1];
        i += 2 + u16::from_be_bytes([b[i + 2], b[i + 3]]) as usize;
        if marker == 0xDA {
            break;
        }
    }
    // In entropy-coded data a literal 0xFF is always followed by 0x00, so FFD9 is the real end.
    while i + 1 < b.len() {
        if b[i] == 0xFF && b[i + 1] == 0xD9 {
            return Ok(Some(i + 2));
        }
        i += 1;
    }
    Ok(None)
}

/// How to reach a frame: a place to seek to (`None`: start from the beginning), and the times
/// between which only this frame falls.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Locate {
    near: Option<f64>,
    from: f64,
    until: f64,
}

/// `seekable`: a bare stream has no length and makes its times up, so a seek in one can't be
/// trusted, even when the time that comes out looks right.
fn locate(times: &[f64], n: usize, seekable: bool) -> Locate {
    Locate {
        near: n.checked_sub(REORDER).filter(|&m| m > 0 && seekable).map(|m| times[m]),
        from: n.checked_sub(1).map_or(f64::NEG_INFINITY, |before| (times[before] + times[n]) / 2.0),
        until: times.get(n + 1).map_or(f64::INFINITY, |next| (times[n] + next) / 2.0),
    }
}

/// One full-resolution frame. Files without an index can land past where a seek asked (MPEG-TS,
/// by a whole group of pictures) or nowhere (bare streams), so a miss seeks further back, then
/// walks from the start: slow, but right.
fn decode_frame(path: &Path, at: Locate) -> Result<RgbImage, String> {
    // The time of what came out says whether a seek found the frame.
    let further = at.near.map(|near| near - FURTHER_BACK).filter(|&seek| seek > 0.0);
    for seek in [at.near, further].into_iter().flatten() {
        if let Ok(image) = one_frame(path, Some(seek), at) {
            return Ok(image);
        }
    }
    one_frame(path, None, at)
}

fn one_frame(path: &Path, seek: Option<f64>, at: Locate) -> Result<RgbImage, String> {
    let start = seek.unwrap_or(0.0);
    let filter = if at.from.is_finite() {
        format!("select=gte(t\\,{:.6}),showinfo", at.from - start)
    } else {
        "showinfo".to_owned()
    };
    let mut cmd = video::command("ffmpeg");
    cmd.args(["-hide_banner", "-nostdin", "-nostats", "-loglevel", "info"]);
    if let Some(seek) = seek {
        cmd.args(["-ss", &format!("{seek:.6}")]);
    }
    let out = cmd
        .arg("-i")
        .arg(path)
        .args(["-map", "0:v:0", "-an", "-sn", "-dn", "-vf", &filter])
        .args(["-frames:v", "1", "-fps_mode", "passthrough"])
        .args(["-pix_fmt", "rgb24", "-c:v", "ppm", "-f", "image2pipe", "-"])
        .stdin(Stdio::null())
        .output()
        .map_err(video::ffmpeg_missing)?;
    let log = String::from_utf8_lossy(&out.stderr);
    if out.stdout.is_empty() {
        return Err(log.lines().last().unwrap_or("ffmpeg returned no frame").trim().to_owned());
    }
    let landed = log.lines().find_map(parse_showinfo).map(|(_, t)| t + start);
    if seek.is_some() && !landed.is_some_and(|t| t >= at.from && t < at.until) {
        return Err("the seek missed the frame".to_owned());
    }
    parse_ppm(&out.stdout)
}

fn parse_ppm(b: &[u8]) -> Result<RgbImage, String> {
    // "P6" <ws> width <ws> height <ws> maxval <single ws> data
    if !b.starts_with(b"P6") {
        return Err("ffmpeg returned an unexpected image format".to_owned());
    }
    let mut fields = [0u32; 3];
    let mut i = 2;
    for f in &mut fields {
        while i < b.len() && b[i].is_ascii_whitespace() {
            i += 1;
        }
        let start = i;
        while i < b.len() && b[i].is_ascii_digit() {
            i += 1;
        }
        *f = std::str::from_utf8(&b[start..i]).ok().and_then(|s| s.parse().ok()).ok_or("bad PPM header")?;
    }
    let [w, h, max] = fields;
    if max != 255 {
        return Err("ffmpeg returned a 16-bit frame".to_owned());
    }
    let size = w as usize * h as usize * 3;
    let data = b.get(i + 1..i + 1 + size).ok_or("truncated frame from ffmpeg")?;
    RgbImage::from_raw(w, h, data.to_vec()).ok_or_else(|| "bad frame size".to_owned())
}

struct Preview {
    frame: usize,
    image: Arc<RgbImage>,
    texture: TextureHandle,
}

enum Job {
    Preview { frame: usize, result: Result<RgbImage, String> },
    Saved(Result<PathBuf, String>),
}

pub struct Picker {
    path: PathBuf,
    info: Option<Info>,
    /// Stays compressed in memory: textures are decoded for what's on screen.
    thumbs: Vec<Vec<u8>>,
    times: Vec<f64>,
    rx: Receiver<Extract>,
    cancel: Arc<AtomicBool>,
    finished: bool,
    stopped: bool,
    error: Option<String>,
    started: Instant,
    took: Duration,
    /// Where the video was when this opened: that frame gets picked once extraction reaches it.
    jump_to: Option<f64>,
    selected: usize,
    selected_at: Instant,
    thumb_size: f32,
    columns: usize,
    rows_visible: usize,
    textures: HashMap<usize, (TextureHandle, u64)>,
    tick: u64,
    preview: Option<Preview>,
    preview_busy: bool,
    jobs_tx: Sender<Job>,
    jobs_rx: Receiver<Job>,
    saving: usize,
    jpeg: bool,
    jpeg_quality: u8,
    out_dir: Option<PathBuf>,
    last_saved: Option<PathBuf>,
    status: String,
    ensure_visible: bool,
    scroll_offset: f32,
    viewport_h: f32,
}

impl Drop for Picker {
    fn drop(&mut self) {
        self.cancel.store(true, Ordering::Relaxed);
    }
}

impl Picker {
    /// `at`: where the video is now, in seconds.
    pub fn new(ctx: &egui::Context, path: PathBuf, at: Option<f64>) -> Self {
        let (tx, rx) = channel();
        let (jobs_tx, jobs_rx) = channel();
        let cancel = Arc::new(AtomicBool::new(false));
        spawn_extract(path.clone(), tx, cancel.clone(), ctx.clone());
        Self {
            path,
            info: None,
            thumbs: Vec::new(),
            times: Vec::new(),
            rx,
            cancel,
            finished: false,
            stopped: false,
            error: None,
            started: Instant::now(),
            took: Duration::ZERO,
            jump_to: at.filter(|&t| t > 0.0),
            selected: 0,
            selected_at: Instant::now(),
            thumb_size: 180.0,
            columns: 1,
            rows_visible: 1,
            textures: HashMap::new(),
            tick: 0,
            preview: None,
            preview_busy: false,
            jobs_tx,
            jobs_rx,
            saving: 0,
            jpeg: false,
            jpeg_quality: 95,
            out_dir: None,
            last_saved: None,
            status: String::new(),
            ensure_visible: false,
            scroll_offset: 0.0,
            viewport_h: 0.0,
        }
    }

    /// Something was saved, so a folder may have a new file in it.
    pub fn saved_any(&self) -> bool {
        self.last_saved.is_some()
    }

    /// Frames that have both a thumbnail and a known time.
    fn count(&self) -> usize {
        self.thumbs.len().min(self.times.len())
    }

    fn locate(&self, n: usize) -> Locate {
        locate(&self.times, n, self.info.as_ref().is_some_and(|i| i.duration > 0.0))
    }

    fn select(&mut self, n: usize) {
        let n = n.min(self.count().saturating_sub(1));
        if n != self.selected {
            self.selected = n;
            self.selected_at = Instant::now();
        }
        self.jump_to = None;
        self.ensure_visible = true;
    }

    fn poll(&mut self, ctx: &egui::Context) {
        while let Ok(message) = self.rx.try_recv() {
            match message {
                Extract::Info(info) => self.info = Some(info),
                Extract::Thumb(jpeg) => self.thumbs.push(jpeg),
                Extract::Time(n, t) => {
                    if n == self.times.len() {
                        self.times.push(t);
                    }
                }
                Extract::Finished(result) => {
                    self.finished = true;
                    self.took = self.started.elapsed();
                    self.stopped = self.cancel.load(Ordering::Relaxed);
                    self.error = result.err();
                }
            }
        }
        let count = self.count();
        if let Some(at) = self.jump_to
            && count > 0
            && (self.finished || self.times[count - 1] >= at)
        {
            let n = self.times[..count].partition_point(|&t| t <= at);
            self.select(n.saturating_sub(1));
        }
        while let Ok(job) = self.jobs_rx.try_recv() {
            match job {
                Job::Preview { frame, result } => {
                    self.preview_busy = false;
                    match result {
                        Ok(image) => {
                            let side = ctx.input(|i| i.max_texture_side).min(4096) as u32;
                            let shown = fit_texture(&image, side);
                            let texture = ctx.load_texture("frames-preview", shown, TextureOptions::LINEAR);
                            self.preview = Some(Preview { frame, image: Arc::new(image), texture });
                        }
                        Err(e) => self.status = format!("Couldn't decode frame {}: {e}", frame + 1),
                    }
                }
                Job::Saved(result) => {
                    self.saving -= 1;
                    match result {
                        Ok(path) => {
                            self.status = format!("Saved {}", path.display());
                            self.last_saved = Some(path);
                        }
                        Err(e) => self.status = format!("Couldn't save: {e}"),
                    }
                }
            }
        }
    }

    /// Fetches the selected frame at full resolution, one ffmpeg at a time.
    fn request_preview(&mut self, ctx: &egui::Context) {
        let n = self.selected;
        if self.preview_busy || n >= self.count() || self.preview.as_ref().is_some_and(|p| p.frame == n) {
            return;
        }
        // Holding an arrow key shouldn't queue a decode per frame.
        let since = self.selected_at.elapsed();
        if since < Duration::from_millis(90) {
            ctx.request_repaint_after(Duration::from_millis(90) - since);
            return;
        }
        let at = self.locate(n);
        let (path, tx, ctx) = (self.path.clone(), self.jobs_tx.clone(), ctx.clone());
        self.preview_busy = true;
        std::thread::spawn(move || {
            let result = decode_frame(&path, at);
            let _ = tx.send(Job::Preview { frame: n, result });
            ctx.request_repaint();
        });
    }

    fn output_dir(&self) -> Option<PathBuf> {
        self.out_dir.clone().or_else(|| self.path.parent().map(Path::to_path_buf))
    }

    fn default_name(&self) -> Option<String> {
        let stem = self.path.file_stem()?.to_string_lossy();
        let t = fmt_time(*self.times.get(self.selected)?).replace(':', "-");
        let ext = if self.jpeg { "jpg" } else { "png" };
        Some(format!("{stem}_frame{:06}_{t}.{ext}", self.selected + 1))
    }

    fn save(&mut self, ctx: &egui::Context, ask: bool) {
        let n = self.selected;
        let Some(name) = self.default_name().filter(|_| n < self.count()) else { return };
        let target = if ask {
            let mut dialog = rfd::FileDialog::new()
                .set_file_name(&name)
                .add_filter("PNG image", &["png"])
                .add_filter("JPEG image", &["jpg", "jpeg"]);
            if let Some(dir) = self.output_dir() {
                dialog = dialog.set_directory(dir);
            }
            match dialog.save_file() {
                Some(p) => p,
                None => return,
            }
        } else {
            let Some(dir) = self.output_dir() else { return };
            unique_path(dir.join(name))
        };
        let jpeg = match crate::files::ext_lower(&target).as_str() {
            "jpg" | "jpeg" => true,
            "png" => false,
            _ => self.jpeg,
        };
        let cached = self.preview.as_ref().filter(|p| p.frame == n).map(|p| p.image.clone());
        let (path, at, quality) = (self.path.clone(), self.locate(n), self.jpeg_quality);
        let (tx, ctx) = (self.jobs_tx.clone(), ctx.clone());
        self.saving += 1;
        self.status = format!("Saving frame {}…", n + 1);
        std::thread::spawn(move || {
            let result = (|| {
                let image = match cached {
                    Some(i) => i,
                    None => Arc::new(decode_frame(&path, at)?),
                };
                write_image(&image, &target, jpeg, quality)?;
                Ok(target)
            })();
            let _ = tx.send(Job::Saved(result));
            ctx.request_repaint();
        });
    }

    /// True when Esc asks to close.
    fn handle_keys(&mut self, ctx: &egui::Context) -> bool {
        // Typing into the frame number box keeps its keys.
        if ctx.egui_wants_keyboard_input() {
            return false;
        }
        // A clicked thumbnail or button grabs focus, and egui would then use the arrows to move
        // that focus around instead of stepping frames.
        if let Some(id) = ctx.memory(|m| m.focused()) {
            ctx.memory_mut(|m| m.surrender_focus(id));
        }
        let (count, s) = (self.count(), self.selected);
        let (cols, page) = (self.columns.max(1), self.columns.max(1) * self.rows_visible.max(1));
        let (target, save, save_as, close) = ctx.input(|i| {
            let moves = [
                (Key::ArrowRight, s + 1),
                (Key::ArrowLeft, s.saturating_sub(1)),
                (Key::ArrowDown, s + cols),
                (Key::ArrowUp, s.saturating_sub(cols)),
                (Key::PageDown, s + page),
                (Key::PageUp, s.saturating_sub(page)),
                (Key::Home, 0),
                (Key::End, count.saturating_sub(1)),
            ];
            let target = moves.iter().filter(|(k, _)| i.key_pressed(*k)).map(|&(_, t)| t).next_back();
            // Not on key repeat: holding Enter saves once.
            let once = |key| {
                i.events
                    .iter()
                    .any(|e| matches!(e, Event::Key { key: k, pressed: true, repeat: false, .. } if *k == key))
            };
            let command_s = i.modifiers.command && once(Key::S);
            (
                target,
                once(Key::Enter) || (command_s && !i.modifiers.shift),
                command_s && i.modifiers.shift,
                i.key_pressed(Key::Escape),
            )
        });
        if let Some(t) = target {
            self.select(t);
        }
        if save {
            self.save(ctx, false);
        }
        if save_as {
            self.save(ctx, true);
        }
        close
    }

    /// Returns true when the user is done with it.
    pub fn show(&mut self, ui: &mut egui::Ui) -> bool {
        let ctx = ui.ctx().clone();
        self.tick += 1;
        self.poll(&ctx);
        let mut close = self.handle_keys(&ctx);
        self.request_preview(&ctx);

        let margin = egui::Margin::symmetric(10, 0);
        let bar = egui::Frame::NONE.fill(theme::bar()).inner_margin(margin);
        egui::Panel::top("frames-top").frame(bar).exact_size(44.0).show(ui, |ui| close |= self.top_bar(ui));
        egui::Panel::bottom("frames-status").frame(bar).exact_size(28.0).show(ui, |ui| self.status_bar(ui));
        if self.count() > 0 {
            let width = ui.max_rect().width();
            let side = egui::Frame::NONE.fill(theme::bar()).inner_margin(egui::Margin::same(10));
            egui::Panel::right("frames-side")
                .frame(side)
                .resizable(true)
                .default_size((width * 0.42).max(420.0))
                .min_size(300.0)
                .max_size((width - 260.0).max(300.0))
                .show(ui, |ui| {
                    // Panels shrink to their content otherwise.
                    ui.set_min_width(ui.available_width());
                    self.side_panel(ui, &ctx);
                });
        }
        let frame = egui::Frame::NONE.fill(ui.visuals().panel_fill).inner_margin(egui::Margin::same(8));
        egui::CentralPanel::default().frame(frame).show(ui, |ui| self.grid(ui, &ctx));

        self.evict_textures();
        if !self.finished {
            ctx.request_repaint_after(Duration::from_millis(60));
        }
        close
    }

    fn top_bar(&mut self, ui: &mut egui::Ui) -> bool {
        let mut close = false;
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            close = ui.button("Close").on_hover_text("Esc").clicked();
            ui.spacing_mut().slider_width = 110.0;
            let top = THUMB_WIDTH as f32;
            ui.add(slim(egui::Slider::new(&mut self.thumb_size, 90.0..=top)).show_value(false));
            ui.label(RichText::new("Size").color(theme::text_dim()));
            ui.with_layout(Layout::left_to_right(Align::Center), |ui| {
                let name = crate::files::file_name(&self.path);
                ui.allocate_ui(Vec2::new(260.0_f32.min(ui.available_width() / 2.0), 44.0), |ui| {
                    ui.add(egui::Label::new(RichText::new(&name).strong()).truncate()).on_hover_text(&name);
                });
                let done = self.count();
                if !self.finished {
                    let estimate = self.info.as_ref().map_or(0, frames_estimate).max(done as u64).max(1);
                    let fraction = done as f32 / estimate as f32;
                    let text = format!("{done} / ~{estimate} frames");
                    ui.add(egui::ProgressBar::new(fraction).desired_width(180.0).corner_radius(0).text(text));
                    if ui.button("Stop").on_hover_text("Stop here and keep the frames so far").clicked() {
                        self.cancel.store(true, Ordering::Relaxed);
                    }
                } else if self.error.is_some() {
                    ui.colored_label(theme::error(), format!("Stopped by an error at {done} frames"));
                } else {
                    let what = if self.stopped { ", stopped after" } else { " in" };
                    let took = self.took.as_secs_f32();
                    ui.label(RichText::new(format!("{done} frames{what} {took:.1} s")).color(theme::text_dim()));
                }
                if let Some(info) = &self.info {
                    let text = format!(
                        "{}x{}  ·  {:.3} fps  ·  {}  ·  {}",
                        info.source.size[0],
                        info.source.size[1],
                        info.source.fps,
                        clock(info.duration),
                        info.source.codec
                    );
                    ui.add(egui::Label::new(RichText::new(text).color(theme::text_dim())).truncate());
                }
            });
        });
        close
    }

    fn status_bar(&mut self, ui: &mut egui::Ui) {
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            let hints = format!("Arrows: step  ·  Enter or double-click: save  ·  {COMMAND}+Shift+S: save as");
            ui.label(RichText::new(hints).color(theme::text_dim()).small());
            ui.with_layout(Layout::left_to_right(Align::Center), |ui| {
                ui.add(egui::Label::new(&self.status).truncate());
            });
        });
    }

    fn side_panel(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        let count = self.count();
        let n = self.selected.min(count - 1);
        let t = self.times[n];

        ui.horizontal(|ui| {
            ui.label(RichText::new(format!("Frame {}", n + 1)).strong());
            ui.label(RichText::new(format!("of {count}")).color(theme::text_dim()));
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                ui.label(RichText::new(fmt_time(t)).monospace().color(theme::text_dim()));
            });
        });

        // The picture, in the room the controls leave. A blurry stand-in until the full frame comes.
        let room = Vec2::new(ui.available_width(), (ui.available_height() - 160.0).max(120.0));
        let (rect, _) = ui.allocate_exact_size(room, Sense::hover());
        ui.painter().rect_filled(rect, 0.0, theme::placeholder());
        let full = self.preview.as_ref().filter(|p| p.frame == n).map(|p| p.texture.clone());
        if let Some(texture) = full.or_else(|| self.thumb_texture(ctx, n)) {
            let shown = Rect::from_center_size(rect.center(), fit(texture.size_vec2(), rect.size()));
            ui.painter().image(texture.id(), shown, whole(), egui::Color32::WHITE);
        }

        ui.add_space(8.0);
        let mut scrub = n + 1;
        ui.spacing_mut().slider_width = (ui.available_width() - 70.0).max(60.0);
        if ui.add(slim(egui::Slider::new(&mut scrub, 1..=count))).changed() {
            self.select(scrub - 1);
        }

        ui.add_space(4.0);
        ui.horizontal(|ui| {
            ui.selectable_value(&mut self.jpeg, false, "PNG").on_hover_text("Lossless");
            ui.selectable_value(&mut self.jpeg, true, "JPEG");
            if self.jpeg {
                ui.spacing_mut().slider_width = 120.0;
                ui.add(slim(egui::Slider::new(&mut self.jpeg_quality, 50..=100)).text("quality"));
            }
        });
        ui.add_space(4.0);
        ui.horizontal(|ui| {
            if ui.button("Change folder…").clicked() {
                let mut dialog = rfd::FileDialog::new();
                if let Some(dir) = self.output_dir() {
                    dialog = dialog.set_directory(dir);
                }
                if let Some(dir) = dialog.pick_folder() {
                    self.out_dir = Some(dir);
                }
            }
            let dir = self.output_dir().map(|d| d.display().to_string()).unwrap_or_default();
            ui.add(egui::Label::new(RichText::new(&dir).color(theme::text_dim())).truncate()).on_hover_text(&dir);
        });
        ui.add_space(4.0);
        ui.horizontal(|ui| {
            let size = info_size(self.info.as_ref());
            let save = ui
                .add(
                    egui::Button::new(RichText::new("Save this frame").strong())
                        .fill(theme::selected())
                        .min_size(Vec2::new(150.0, 32.0)),
                )
                .on_hover_text(format!("Enter or {COMMAND}+S. Saves at the full {size}."));
            if save.clicked() {
                self.save(ctx, false);
            }
            let tip = format!("{COMMAND}+Shift+S");
            if ui.add(egui::Button::new("Save as…").min_size(Vec2::new(0.0, 32.0))).on_hover_text(tip).clicked() {
                self.save(ctx, true);
            }
            let saved = self.last_saved.clone();
            let show = ui.add_enabled(saved.is_some(), egui::Button::new("Show saved").min_size(Vec2::new(0.0, 32.0)));
            if show.clicked()
                && let Some(p) = &saved
            {
                crate::app::reveal(p);
            }
        });
    }

    fn grid(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        let count = self.count();
        if count == 0 {
            ui.centered_and_justified(|ui| match (&self.error, self.finished) {
                (Some(e), _) => ui.colored_label(theme::error(), e),
                (None, true) => ui.label(RichText::new("No frames in this file").color(theme::text_dim())),
                (None, false) => ui.label(RichText::new("Reading the frames…").color(theme::text_dim())),
            });
            return;
        }
        let [w, h] = self.info.as_ref().map_or([16, 9], |i| i.source.size);
        let aspect = h as f32 / w.max(1) as f32;
        let gap = 8.0;
        let room = ui.available_width() - 14.0; // for the scrollbar
        let cols = ((room + gap) / (self.thumb_size + gap)).floor().max(1.0) as usize;
        // Cells stretch so the row fills the width exactly.
        let cell_w = ((room - gap * (cols - 1) as f32) / cols as f32).max(40.0);
        let img_h = (cell_w * aspect).min(cell_w * 1.8);
        let cell_h = img_h + 18.0;
        let rows = count.div_ceil(cols);
        let pitch = cell_h + gap;
        self.columns = cols;
        self.rows_visible = ((self.viewport_h / pitch).floor() as usize).max(1);

        let selected = self.selected;
        let mut area = egui::ScrollArea::vertical().auto_shrink(false);
        if std::mem::take(&mut self.ensure_visible) && self.viewport_h > 0.0 {
            let top = (selected / cols) as f32 * pitch;
            if top < self.scroll_offset {
                area = area.vertical_scroll_offset(top);
            } else if top + cell_h > self.scroll_offset + self.viewport_h {
                area = area.vertical_scroll_offset(top + cell_h - self.viewport_h);
            }
        }

        let (mut clicked, mut double) = (None, None);
        ui.spacing_mut().item_spacing = Vec2::splat(gap);
        let out = area.show_rows(ui, cell_h, rows, |ui, range| {
            for row in range {
                ui.horizontal(|ui| {
                    for i in row * cols..((row + 1) * cols).min(count) {
                        let (rect, resp) = ui.allocate_exact_size(Vec2::new(cell_w, cell_h), Sense::click());
                        let img = Rect::from_min_size(rect.min, Vec2::new(cell_w, img_h));
                        ui.painter().rect_filled(img, 0.0, theme::placeholder());
                        if let Some(texture) = self.thumb_texture(ctx, i) {
                            let shown = Rect::from_center_size(img.center(), fit(texture.size_vec2(), img.size()));
                            ui.painter().image(texture.id(), shown, whole(), egui::Color32::WHITE);
                        }
                        let painter = ui.painter();
                        if i == selected {
                            let stroke = Stroke::new(3.0, theme::text());
                            painter.rect_stroke(img.expand(2.0), 0.0, stroke, egui::StrokeKind::Outside);
                        } else if resp.hovered() {
                            let stroke = Stroke::new(1.0, theme::icon());
                            painter.rect_stroke(img, 0.0, stroke, egui::StrokeKind::Outside);
                        }
                        let number = if i == selected { theme::text() } else { theme::text_dim() };
                        let left = rect.left_bottom() + Vec2::new(2.0, -1.0);
                        painter.text(
                            left,
                            Align2::LEFT_BOTTOM,
                            format!("#{}", i + 1),
                            FontId::proportional(12.0),
                            number,
                        );
                        let right = rect.right_bottom() + Vec2::new(-2.0, -1.0);
                        let time = fmt_time(self.times[i]);
                        painter.text(right, Align2::RIGHT_BOTTOM, time, FontId::monospace(11.0), theme::text_dim());
                        if resp.clicked() {
                            clicked = Some(i);
                        }
                        if resp.double_clicked() {
                            double = Some(i);
                        }
                    }
                });
            }
        });
        self.scroll_offset = out.state.offset.y;
        self.viewport_h = out.inner_rect.height();

        if let Some(i) = clicked {
            self.select(i);
        }
        if let Some(i) = double {
            self.select(i);
            self.save(ctx, false);
        }
    }

    fn thumb_texture(&mut self, ctx: &egui::Context, i: usize) -> Option<TextureHandle> {
        if let Some((texture, used)) = self.textures.get_mut(&i) {
            *used = self.tick;
            return Some(texture.clone());
        }
        let jpeg = self.thumbs.get(i)?;
        let img = image::load_from_memory_with_format(jpeg, image::ImageFormat::Jpeg).ok()?.to_rgb8();
        let picture = ColorImage::from_rgb([img.width() as usize, img.height() as usize], img.as_raw());
        let texture = ctx.load_texture(format!("frames-thumb{i}"), picture, TextureOptions::LINEAR);
        self.textures.insert(i, (texture.clone(), self.tick));
        Some(texture)
    }

    fn evict_textures(&mut self) {
        if self.textures.len() <= TEXTURE_LIMIT {
            return;
        }
        let mut by_age: Vec<_> = self.textures.iter().map(|(&k, (_, used))| (*used, k)).collect();
        by_age.sort_unstable();
        for (_, k) in by_age.into_iter().take(self.textures.len() - TEXTURE_LIMIT / 2) {
            self.textures.remove(&k);
        }
    }
}

fn frames_estimate(info: &Info) -> u64 {
    if info.source.frames > 0 { info.source.frames } else { (info.duration * info.source.fps).round().max(1.0) as u64 }
}

fn info_size(info: Option<&Info>) -> String {
    info.map_or_else(|| "size".to_owned(), |i| format!("{}x{}", i.source.size[0], i.source.size[1]))
}

/// A square handle and a filled track, like the rest of the app.
fn slim<'a>(slider: egui::Slider<'a>) -> egui::Slider<'a> {
    slider.trailing_fill(true).handle_shape(egui::style::HandleShape::Rect { aspect_ratio: 0.5 })
}

fn whole() -> Rect {
    Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0))
}

/// For display only: saving always uses the full frame.
fn fit_texture(img: &RgbImage, side: u32) -> ColorImage {
    let (w, h) = img.dimensions();
    if w.max(h) <= side {
        return ColorImage::from_rgb([w as usize, h as usize], img.as_raw());
    }
    let s = side as f32 / w.max(h) as f32;
    let (nw, nh) = (((w as f32 * s) as u32).max(1), ((h as f32 * s) as u32).max(1));
    let small = image::imageops::resize(img, nw, nh, image::imageops::FilterType::Triangle);
    ColorImage::from_rgb([nw as usize, nh as usize], small.as_raw())
}

/// Encoded in memory first, so a full disk is an error rather than half a file.
fn write_image(img: &RgbImage, path: &Path, jpeg: bool, quality: u8) -> Result<(), String> {
    use image::ImageEncoder;
    let (w, h) = img.dimensions();
    let mut bytes = Vec::new();
    let color = image::ExtendedColorType::Rgb8;
    let encoded = if jpeg {
        image::codecs::jpeg::JpegEncoder::new_with_quality(&mut bytes, quality).write_image(img.as_raw(), w, h, color)
    } else {
        image::codecs::png::PngEncoder::new(&mut bytes).write_image(img.as_raw(), w, h, color)
    };
    encoded.map_err(|e| e.to_string())?;
    std::fs::write(path, bytes).map_err(|e| format!("{}: {e}", path.display()))
}

/// `photo.png` becomes `photo (2).png` if it already exists: saving never overwrites.
pub fn unique_path(p: PathBuf) -> PathBuf {
    if !p.exists() {
        return p;
    }
    let stem = p.file_stem().unwrap_or_default().to_string_lossy().to_string();
    let ext = p.extension().map(|e| format!(".{}", e.to_string_lossy())).unwrap_or_default();
    (2..).map(|k| p.with_file_name(format!("{stem} ({k}){ext}"))).find(|c| !c.exists()).unwrap_or(p)
}

fn fit(content: Vec2, area: Vec2) -> Vec2 {
    content * (area.x / content.x).min(area.y / content.y)
}

/// Down to the millisecond: `clock` stops at seconds, and frames are shorter than that.
fn fmt_time(t: f64) -> String {
    let ms = (t.max(0.0) * 1000.0).round() as u64;
    let (h, m, s, ms) = (ms / 3_600_000, ms / 60_000 % 60, ms / 1000 % 60, ms % 1000);
    if h > 0 { format!("{h}:{m:02}:{s:02}.{ms:03}") } else { format!("{m:02}:{s:02}.{ms:03}") }
}

#[cfg(test)]
mod tests {
    use super::*;
    use egui::{Modifiers, RawInput};

    fn test_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("telescopeview-frames-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// Every frame has its own brightness. `None` when there's no ffmpeg to make it with.
    /// mpeg4 because every ffmpeg build has it, x264 or not; it does B-frames too.
    fn make_video(dir: &Path, name: &str, extra: &[&str]) -> Option<PathBuf> {
        let out = dir.join(name);
        let made = video::command("ffmpeg")
            .args(["-y", "-hide_banner", "-loglevel", "error", "-f", "lavfi", "-i"])
            .arg("nullsrc=s=128x72:r=25:d=3,geq=lum='N*3':cb=128:cr=128,format=yuv420p")
            .args(extra)
            .args(["-c:v", "mpeg4", "-g", "12", "-bf", "2", "-q:v", "1"])
            .arg(&out)
            .stdin(Stdio::null())
            .status();
        match made {
            Ok(s) if s.success() => Some(out),
            _ => {
                eprintln!("no ffmpeg to make a test video with, skipping");
                None
            }
        }
    }

    fn extract_all(path: &Path) -> (Vec<Vec<u8>>, Vec<f64>) {
        let (tx, rx) = channel();
        extract(path, 64, &tx, &AtomicBool::new(false)).unwrap();
        drop(tx);
        let (mut thumbs, mut times) = (Vec::new(), Vec::new());
        for message in rx {
            match message {
                Extract::Thumb(jpeg) => thumbs.push(jpeg),
                Extract::Time(n, t) => {
                    assert_eq!(n, times.len());
                    times.push(t);
                }
                _ => {}
            }
        }
        (thumbs, times)
    }

    /// Lets ffmpeg walk the whole file and pick frame `n` by number.
    fn reference_frame(path: &Path, n: usize) -> RgbImage {
        let out = video::command("ffmpeg")
            .args(["-hide_banner", "-loglevel", "error", "-i"])
            .arg(path)
            .args(["-vf", &format!("select=eq(n\\,{n})"), "-fps_mode", "passthrough", "-frames:v", "1"])
            .args(["-pix_fmt", "rgb24", "-c:v", "ppm", "-f", "image2pipe", "-"])
            .stdin(Stdio::null())
            .output()
            .unwrap();
        parse_ppm(&out.stdout).unwrap()
    }

    fn check_exact_frames(path: &Path, expected: usize) {
        let (thumbs, times) = extract_all(path);
        let seekable = video::probe(path).unwrap().duration > 0.0;
        assert_eq!(thumbs.len(), expected);
        assert_eq!(times.len(), expected);
        assert!(times.windows(2).all(|w| w[0] < w[1]), "times must increase: {times:?}");
        for jpeg in &thumbs {
            image::load_from_memory(jpeg).unwrap();
        }
        // Keyframes every 12: covers them, the frames right after and B-frames.
        for n in [0, 1, 2, 11, 12, 13, 30, 31, 37, 47, 48, expected - 1] {
            let got = decode_frame(path, locate(&times, n, seekable)).unwrap();
            assert_eq!(got.dimensions(), (128, 72));
            assert!(got.as_raw() == reference_frame(path, n).as_raw(), "frame {n} is off");
        }
    }

    #[test]
    fn constant_frame_rate_is_frame_exact() {
        let dir = test_dir("cfr");
        if let Some(path) = make_video(&dir, "cfr.mp4", &[]) {
            check_exact_frames(&path, 75);
        }
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn variable_frame_rate_is_frame_exact() {
        let dir = test_dir("vfr");
        // Irregular gaps after frame 30, written with real times.
        let vfr = [
            "-vf",
            "settb=1/1000,setpts='if(lt(N,30),N/25,N/25+0.37+N*0.013)/TB'",
            "-fps_mode",
            "passthrough",
            "-enc_time_base:v",
            "1:1000",
        ];
        if let Some(path) = make_video(&dir, "vfr.mkv", &vfr) {
            check_exact_frames(&path, 75);
        }
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn mpeg_ts_is_frame_exact() {
        let dir = test_dir("ts");
        if let Some(path) = make_video(&dir, "clip.ts", &[]) {
            check_exact_frames(&path, 75);
        }
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn bare_stream_is_frame_exact() {
        let dir = test_dir("bare");
        if let Some(path) = make_video(&dir, "clip.m4v", &["-f", "m4v"]) {
            check_exact_frames(&path, 75);
        }
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn probe_reads_what_the_file_says() {
        let dir = test_dir("probe");
        if let Some(path) = make_video(&dir, "probe.mp4", &[]) {
            let info = video::probe(&path).unwrap();
            assert_eq!(info.source.size, [128, 72]);
            assert!((info.source.fps - 25.0).abs() < 0.01);
            assert_eq!(frames_estimate(&info), 75);
            assert_eq!(info.source.codec, "mpeg4");
        }
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn seeks_to_well_before_the_frame() {
        let times: Vec<f64> = (0..100).map(|n| n as f64).collect();
        assert_eq!(locate(&times, 0, true), Locate { near: None, from: f64::NEG_INFINITY, until: 0.5 });
        assert_eq!(locate(&times, 1, true), Locate { near: None, from: 0.5, until: 1.5 });
        assert_eq!(locate(&times, REORDER, true).near, None);
        assert_eq!(locate(&times, 50, true), Locate { near: Some(34.0), from: 49.5, until: 50.5 });
        assert_eq!(locate(&times, 50, false).near, None);
        assert_eq!(locate(&times, 99, true).until, f64::INFINITY);
    }

    #[test]
    fn showinfo_lines() {
        let line = "[Parsed_showinfo_1 @ 000001] n:  12 pts:   6144 pts_time:0.48 duration: 512";
        assert_eq!(parse_showinfo(line), Some((12, 0.48)));
        assert_eq!(parse_showinfo("frame= 12 fps=0.0"), None);
    }

    #[test]
    fn jpeg_is_cut_at_its_real_end() {
        // A 0xFFD9 inside a header segment isn't the end; the one after the scan is.
        let jpeg = [0xFF, 0xD8, 0xFF, 0xE0, 0, 4, 0xFF, 0xD9, 0xFF, 0xDA, 0, 2, 1, 2, 0xFF, 0x00, 3, 0xFF, 0xD9];
        assert_eq!(jpeg_len(&jpeg), Ok(Some(jpeg.len())));
        assert_eq!(jpeg_len(&jpeg[..jpeg.len() - 1]), Ok(None));
        assert!(jpeg_len(b"nope").is_err());
    }

    #[test]
    fn saving_never_overwrites() {
        let dir = test_dir("unique");
        let file = dir.join("a.png");
        assert_eq!(unique_path(file.clone()), file);
        std::fs::write(&file, b"x").unwrap();
        assert_eq!(unique_path(file), dir.join("a (2).png"));
        let _ = std::fs::remove_dir_all(dir);
    }

    fn key(key: Key, pressed: bool, repeat: bool) -> Event {
        Event::Key { key, physical_key: None, pressed, repeat, modifiers: Modifiers::NONE }
    }

    /// One frame of the panel with these events. True when it asks to close.
    fn frame(ctx: &egui::Context, picker: &mut Picker, events: Vec<Event>) -> bool {
        let raw = RawInput {
            screen_rect: Some(Rect::from_min_size(egui::Pos2::ZERO, Vec2::new(1200.0, 800.0))),
            events,
            ..Default::default()
        };
        let mut close = false;
        let mut out = ctx.run_ui(raw, |ui| close = picker.show(ui));
        out.textures_delta.clear();
        close
    }

    fn press(ctx: &egui::Context, picker: &mut Picker, k: Key) -> bool {
        let close = frame(ctx, picker, vec![key(k, true, false)]);
        frame(ctx, picker, vec![key(k, false, false)]) || close
    }

    #[test]
    fn keys_step_and_esc_closes() {
        let ctx = egui::Context::default();
        let dir = test_dir("keys");
        let mut picker = Picker::new(&ctx, dir.join("missing.mp4"), None);
        while !picker.finished {
            frame(&ctx, &mut picker, Vec::new());
        }
        assert!(picker.error.is_some(), "a file that isn't there is an error, not a crash");
        picker.times = (0..100).map(|n| n as f64 / 25.0).collect();
        picker.thumbs = vec![Vec::new(); 100];
        frame(&ctx, &mut picker, Vec::new());

        assert!(!press(&ctx, &mut picker, Key::ArrowRight));
        assert_eq!(picker.selected, 1);
        press(&ctx, &mut picker, Key::ArrowLeft);
        press(&ctx, &mut picker, Key::ArrowLeft);
        assert_eq!(picker.selected, 0, "stops at the first frame");
        press(&ctx, &mut picker, Key::End);
        assert_eq!(picker.selected, 99);
        press(&ctx, &mut picker, Key::ArrowRight);
        assert_eq!(picker.selected, 99, "stops at the last frame");
        let cols = picker.columns;
        assert!(cols > 1, "this window fits several across");
        press(&ctx, &mut picker, Key::ArrowUp);
        assert_eq!(picker.selected, 99 - cols);
        press(&ctx, &mut picker, Key::Home);
        assert_eq!(picker.selected, 0);
        assert!(!picker.jpeg, "moving around leaves the format alone");
        assert!(press(&ctx, &mut picker, Key::Escape));
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn the_slider_moves_through_the_frames() {
        let ctx = egui::Context::default();
        let dir = test_dir("slider");
        let mut picker = Picker::new(&ctx, dir.join("missing.mp4"), None);
        picker.times = (0..1000).map(|n| n as f64 / 25.0).collect();
        picker.thumbs = vec![Vec::new(); 1000];
        frame(&ctx, &mut picker, Vec::new());
        let button = |pos, pressed| Event::PointerButton {
            pos,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: Modifiers::NONE,
        };
        // Down the side panel until a press lands on the slider: nothing else is clickable there.
        for y in (450..770).step_by(4) {
            let at = egui::pos2(1000.0, y as f32);
            frame(&ctx, &mut picker, vec![Event::PointerMoved(at)]);
            frame(&ctx, &mut picker, vec![button(at, true)]);
            frame(&ctx, &mut picker, vec![button(at, false)]);
            if picker.selected != 0 {
                break;
            }
        }
        assert!((300..1000).contains(&picker.selected), "landed on {}", picker.selected);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn opens_on_the_frame_the_video_was_at() {
        let ctx = egui::Context::default();
        let dir = test_dir("jump");
        let mut picker = Picker::new(&ctx, dir.join("missing.mp4"), Some(1.01));
        picker.times = (0..50).map(|n| n as f64 / 25.0).collect();
        picker.thumbs = vec![Vec::new(); 50];
        frame(&ctx, &mut picker, Vec::new());
        assert_eq!(picker.selected, 25, "the frame on screen at 1.01 s came at 1.00 s");
        assert!(picker.jump_to.is_none());
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn enter_saves_the_exact_frame_once_at_full_size() {
        let dir = test_dir("save");
        let Some(path) = make_video(&dir, "clip.mp4", &[]) else { return };
        let ctx = egui::Context::default();
        let mut picker = Picker::new(&ctx, path.clone(), None);
        let waited = Instant::now();
        while !picker.finished {
            assert!(waited.elapsed().as_secs() < 60, "extraction took too long");
            frame(&ctx, &mut picker, Vec::new());
            std::thread::sleep(Duration::from_millis(5));
        }
        assert_eq!(picker.count(), 75);
        picker.select(37);
        frame(&ctx, &mut picker, Vec::new());

        // Held down: one press, then repeats. One file, not a pile of them.
        frame(&ctx, &mut picker, vec![key(Key::Enter, true, false)]);
        frame(&ctx, &mut picker, vec![key(Key::Enter, true, true)]);
        frame(&ctx, &mut picker, vec![key(Key::Enter, false, false)]);
        while picker.saving > 0 {
            assert!(waited.elapsed().as_secs() < 90, "saving took too long");
            frame(&ctx, &mut picker, Vec::new());
            std::thread::sleep(Duration::from_millis(5));
        }
        let saved = picker.last_saved.clone().expect(&picker.status);
        assert_eq!(saved.file_name().unwrap(), "clip_frame000038_00-01.480.png");
        let got = image::open(&saved).unwrap().to_rgb8();
        assert!(got.as_raw() == reference_frame(&path, 37).as_raw(), "not frame 37");
        let pngs = std::fs::read_dir(&dir).unwrap().filter(|e| e.as_ref().unwrap().path() != path).count();
        assert_eq!(pngs, 1);

        // A second press doesn't overwrite the first.
        press(&ctx, &mut picker, Key::Enter);
        while picker.saving > 0 {
            frame(&ctx, &mut picker, Vec::new());
            std::thread::sleep(Duration::from_millis(5));
        }
        assert_eq!(picker.last_saved.as_ref().unwrap().file_name().unwrap(), "clip_frame000038_00-01.480 (2).png");

        // A double-click on the first thumbnail picks it and saves it.
        press(&ctx, &mut picker, Key::Home);
        press(&ctx, &mut picker, Key::ArrowRight);
        frame(&ctx, &mut picker, Vec::new());
        let at = egui::pos2(40.0, 80.0);
        let button = |pressed| Event::PointerButton {
            pos: at,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: Modifiers::NONE,
        };
        frame(&ctx, &mut picker, vec![Event::PointerMoved(at)]);
        for pressed in [true, false, true, false] {
            frame(&ctx, &mut picker, vec![button(pressed)]);
        }
        assert_eq!(picker.selected, 0);
        while picker.saving > 0 {
            frame(&ctx, &mut picker, Vec::new());
            std::thread::sleep(Duration::from_millis(5));
        }
        assert_eq!(picker.last_saved.as_ref().unwrap().file_name().unwrap(), "clip_frame000001_00-00.000.png");
        drop(picker);
        let _ = std::fs::remove_dir_all(dir);
    }
}
