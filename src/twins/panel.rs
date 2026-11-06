//! The screen: scan progress, then two pictures side by side with the keep/delete choices, then
//! a list of what's about to go. Draws into whatever `Ui` it gets and takes its colours from
//! that `Ui`'s style, so the host decides where it lives.
//!
//! Nothing is deleted until "Move to Recycle Bin" on the last page.

use super::fingerprint::Picture;
use super::review::{Choice, Review};
use super::scan::{Decode, FileInfo, Options, Scan, Stage};
use eframe::egui::{
    self, Align, Color32, ColorImage, Key, Layout, Pos2, Rect, RichText, Sense, Stroke, TextureHandle, TextureOptions,
    UiBuilder, Vec2, vec2,
};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc::{self, Receiver};
use std::sync::{Arc, Condvar, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

/// Sends one file to the Recycle Bin. `Ok` with the file still there means the user said no in
/// a dialog of the system's.
pub type Remove = fn(&Path) -> Result<(), String>;

pub enum Event {
    /// These files are gone. Sent once, so the host can update its file list.
    Deleted(Vec<PathBuf>),
    /// The user is done with the panel; the host can drop it.
    Closed,
}

const BIN: &str = if cfg!(windows) { "Recycle Bin" } else { "Trash" };
const BAR: f32 = 44.0;
const INFO: f32 = 50.0;
const ACTIONS: f32 = 56.0;
const PRELOAD_PAIRS: usize = 1;
const LOAD_THREADS: usize = 2;
const MAX_DISPLAY_SIDE: usize = 4096;

pub struct Panel {
    page: Page,
    threshold: f32,
    decode: Decode,
    remove: Remove,
}

enum Page {
    Scanning(Scan),
    Reviewing(Box<Reviewing>),
    Removing { worker: JoinHandle<Removed>, done: Arc<AtomicUsize>, total: usize },
    Finished(Removed),
}

struct Reviewing {
    review: Review,
    loader: Loader,
    slots: HashMap<u32, Slot>,
    wanted: Vec<u32>,
    view: View,
    shown: Option<(u32, u32)>,
    /// On the last page, before the pairs have run out.
    finishing: bool,
}

enum Slot {
    Loading,
    Ready(TextureHandle),
    Failed(String),
}

#[derive(Default)]
struct Removed {
    gone: Vec<PathBuf>,
    bytes: u64,
    /// Said no in the system's dialog.
    cancelled: usize,
    failed: Vec<(PathBuf, String)>,
}

impl Panel {
    /// Starts scanning right away. `threshold` is 0.5..=1.0 and can change later on the screen.
    pub fn new(options: Options, threshold: f32, remove: Remove) -> Self {
        let decode = options.decode;
        Self { page: Page::Scanning(Scan::start(options)), threshold, decode, remove }
    }

    /// Where the slider is now, for the host to remember.
    pub fn threshold(&self) -> f32 {
        self.threshold
    }

    pub fn show(&mut self, ui: &mut egui::Ui) -> Option<Event> {
        let full = ui.available_rect_before_wrap();
        ui.allocate_rect(full, Sense::hover());
        let mut root = ui.new_child(UiBuilder::new().max_rect(full));
        let ctx = root.ctx().clone();

        match &mut self.page {
            Page::Scanning(scan) => {
                if let Some(report) = scan.poll() {
                    let side = ctx.input(|i| i.max_texture_side).min(MAX_DISPLAY_SIDE) as u32;
                    let paths = report.files.iter().map(|f| f.path.clone()).collect();
                    self.page = Page::Reviewing(Box::new(Reviewing {
                        review: Review::new(report, self.threshold),
                        loader: Loader::start(paths, self.decode, side, ctx.clone()),
                        slots: HashMap::new(),
                        wanted: Vec::new(),
                        view: View::default(),
                        shown: None,
                        finishing: false,
                    }));
                    ctx.request_repaint();
                    return None;
                }
                if scan.finished() {
                    return Some(Event::Closed);
                }
                ctx.request_repaint_after(Duration::from_millis(50));
                scanning(&mut root, scan)
            }
            Page::Reviewing(state) => {
                let outcome = state.show(&mut root);
                self.threshold = state.review.threshold();
                match outcome {
                    Outcome::Stay => None,
                    Outcome::Close => Some(Event::Closed),
                    Outcome::Delete => {
                        let files: Vec<(PathBuf, u64)> =
                            state.review.marked().into_iter().map(|f| (f.path.clone(), f.bytes)).collect();
                        let done = Arc::new(AtomicUsize::new(0));
                        let total = files.len();
                        let worker = start_removing(files, self.remove, done.clone(), ctx.clone());
                        self.page = Page::Removing { worker, done, total };
                        None
                    }
                }
            }
            Page::Removing { worker, done, total } => {
                centred(&mut root, |ui| {
                    ui.label(format!("Moving to the {BIN}…"));
                    let fraction = done.load(Ordering::Relaxed) as f32 / (*total).max(1) as f32;
                    ui.add(egui::ProgressBar::new(fraction).desired_width(320.0).corner_radius(0));
                });
                if worker.is_finished() {
                    let Page::Removing { worker, .. } = std::mem::replace(&mut self.page, Page::Finished(Removed::default()))
                    else {
                        unreachable!()
                    };
                    let removed = worker.join().unwrap_or_default();
                    let gone = removed.gone.clone();
                    self.page = Page::Finished(removed);
                    ctx.request_repaint();
                    return Some(Event::Deleted(gone));
                }
                None
            }
            Page::Finished(removed) => finished(&mut root, removed),
        }
    }
}

fn scanning(ui: &mut egui::Ui, scan: &Scan) -> Option<Event> {
    let p = scan.progress();
    let mut event = None;
    centred(ui, |ui| {
        let what = match p.stage {
            Stage::Listing => format!("Listing files… {}", p.total),
            Stage::CheckingCopies => format!("Looking for exact copies… {} of {}", p.done, p.total),
            Stage::Reading => format!("Reading pictures… {} of {}", p.done, p.total),
            Stage::Comparing => "Comparing…".to_owned(),
        };
        ui.label(what);
        let fraction = if p.total == 0 { 0.0 } else { p.done as f32 / p.total as f32 };
        ui.add(egui::ProgressBar::new(fraction).desired_width(320.0).corner_radius(0));
        ui.add_space(6.0);
        if button(ui, "Cancel", 100.0).clicked() {
            scan.cancel();
            event = Some(Event::Closed);
        }
    });
    event
}

fn finished(ui: &mut egui::Ui, removed: &Removed) -> Option<Event> {
    let mut event = None;
    centred(ui, |ui| {
        let n = removed.gone.len();
        ui.heading(format!("Moved {} to the {BIN}", files(n)));
        ui.label(format!("{} freed once it's emptied.", bytes(removed.bytes)));
        if removed.cancelled > 0 {
            ui.label(format!("{} left alone: you said no in the system's dialog.", files(removed.cancelled)));
        }
        if !removed.failed.is_empty() {
            ui.colored_label(ui.visuals().error_fg_color, format!("{} couldn't be moved:", files(removed.failed.len())));
            for (path, why) in removed.failed.iter().take(8) {
                ui.label(format!("{}: {why}", file_name(path)));
            }
        }
        ui.add_space(8.0);
        if button(ui, "Close", 100.0).clicked() {
            event = Some(Event::Closed);
        }
    });
    event
}

fn start_removing(files: Vec<(PathBuf, u64)>, remove: Remove, done: Arc<AtomicUsize>, ctx: egui::Context) -> JoinHandle<Removed> {
    thread::spawn(move || {
        let mut result = Removed::default();
        for (path, size) in files {
            match remove(&path) {
                Ok(()) if !path.exists() => {
                    result.gone.push(path);
                    result.bytes += size;
                }
                Ok(()) => result.cancelled += 1,
                Err(e) => result.failed.push((path, e)),
            }
            done.fetch_add(1, Ordering::Relaxed);
            ctx.request_repaint();
        }
        ctx.request_repaint();
        result
    })
}

enum Outcome {
    Stay,
    Close,
    Delete,
}

/// What the keyboard asked for this frame.
#[derive(Clone, Copy)]
enum Command {
    Choose(Choice),
    Undo,
    Close,
}

impl Reviewing {
    fn show(&mut self, ui: &mut egui::Ui) -> Outcome {
        let ctx = ui.ctx().clone();
        let reviewing = !self.finishing && self.review.current().is_some();
        let command = keys(&ctx, reviewing);
        let full = ui.max_rect();
        let bar = Rect::from_min_size(full.min, vec2(full.width(), BAR));
        let body = Rect::from_min_max(Pos2::new(full.min.x, bar.max.y), full.max);

        let mut clicked = None;
        let mut outcome = Outcome::Stay;
        self.top_bar(&mut ui.new_child(UiBuilder::new().max_rect(bar.shrink2(vec2(10.0, 0.0)))), reviewing, &mut clicked);
        let mut inside = ui.new_child(UiBuilder::new().max_rect(body));
        if reviewing {
            self.load();
            self.pair_page(&mut inside, &mut clicked);
        } else {
            self.last_page(&mut inside, &mut outcome, &mut clicked);
        }

        match command.or(clicked) {
            Some(Command::Choose(choice)) => self.review.choose(choice),
            Some(Command::Undo) => {
                self.review.undo();
                self.finishing = false;
            }
            // Never loses marked files on one press: they get shown first.
            Some(Command::Close) => {
                if self.review.marked().is_empty() {
                    outcome = Outcome::Close;
                } else {
                    self.finishing = reviewing;
                }
            }
            None => {}
        }
        if command.is_some() || clicked.is_some() {
            ctx.request_repaint();
        }
        outcome
    }

    fn top_bar(&mut self, ui: &mut egui::Ui, reviewing: bool, clicked: &mut Option<Command>) {
        let rect = ui.max_rect();
        ui.painter().hline(rect.x_range().expand(10.0), rect.max.y - 0.5, ui.visuals().widgets.noninteractive.bg_stroke);
        let marked = self.review.marked().len();
        let summary = self.review.current().map(|c| {
            let how = if c.pair.identical { "Identical files".to_owned() } else { format!("{:.0}% alike", c.pair.score * 100.0) };
            (format!("Set {} of {}", c.set, c.sets), how, c.open_in_set)
        });
        ui.with_layout(Layout::left_to_right(Align::Center), |ui| {
            // A fixed width, or the slider slides away from the pointer as this text changes.
            ui.allocate_ui_with_layout(vec2(190.0, ui.available_height()), Layout::left_to_right(Align::Center), |ui| {
                ui.set_min_width(190.0);
                if let Some((set, how, _)) = &summary {
                    ui.label(RichText::new(how).strong());
                    ui.label(RichText::new(set).weak());
                }
            });
            let mut percent = self.review.threshold() * 100.0;
            ui.label(RichText::new("Alike at least").weak());
            let slider = egui::Slider::new(&mut percent, 50.0..=100.0).step_by(1.0).fixed_decimals(0).suffix("%");
            if ui.add(slider).changed() {
                self.review.set_threshold(percent / 100.0);
            }
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                // The last page has its own way out.
                if reviewing {
                    if button(ui, "Close", 0.0).clicked() {
                        *clicked = Some(Command::Close);
                    }
                    if marked > 0 && button(ui, &format!("Finish ({marked} marked)"), 0.0).clicked() {
                        self.finishing = true;
                    }
                }
                if ui.add_enabled(self.review.can_undo(), plain("Undo   Z", 0.0)).clicked() {
                    *clicked = Some(Command::Undo);
                }
                if let Some((.., open)) = summary
                    && open > 1
                    && reviewing
                    && button(ui, &format!("Keep all {open} left in set   S"), 0.0).clicked()
                {
                    *clicked = Some(Command::Choose(Choice::KeepSet));
                }
            });
        });
    }

    /// Asks for the pictures of this pair and the next, and drops the ones no longer needed.
    fn load(&mut self) {
        let Some(current) = self.review.current() else { return };
        let (left, right) = (current.pair.left, current.pair.right);
        if self.shown != Some((left, right)) {
            self.shown = Some((left, right));
            self.view = View::default();
        }
        let mut wanted = vec![left, right];
        for p in self.review.upcoming(PRELOAD_PAIRS) {
            for f in [p.left, p.right] {
                if !wanted.contains(&f) {
                    wanted.push(f);
                }
            }
        }
        if wanted != self.wanted {
            self.slots.retain(|f, _| wanted.contains(f));
            let missing: Vec<u32> = wanted.iter().copied().filter(|f| !self.slots.contains_key(f)).collect();
            for &f in &missing {
                self.slots.insert(f, Slot::Loading);
            }
            self.loader.want(missing);
            self.wanted = wanted;
        }
    }

    /// At most one upload a frame, so a big picture arriving doesn't stall the buttons.
    fn receive(&mut self, ctx: &egui::Context) {
        let Ok((file, result)) = self.loader.results.try_recv() else { return };
        ctx.request_repaint();
        if !self.slots.contains_key(&file) {
            return;
        }
        let slot = match result {
            Ok(pic) => {
                let image = ColorImage::from_rgba_unmultiplied([pic.width as usize, pic.height as usize], &pic.rgba);
                let options = TextureOptions {
                    mipmap_mode: Some(egui::TextureFilter::Linear),
                    ..TextureOptions::LINEAR
                };
                Slot::Ready(ctx.load_texture(format!("twins {file}"), image, options))
            }
            Err(e) => Slot::Failed(e),
        };
        self.slots.insert(file, slot);
    }

    fn pair_page(&mut self, ui: &mut egui::Ui, clicked: &mut Option<Command>) {
        self.receive(&ui.ctx().clone());
        let Some(current) = self.review.current() else { return };
        let (left_file, right_file) = (current.left.clone(), current.right.clone());
        let (left, right) = (current.pair.left, current.pair.right);

        let body = ui.max_rect();
        let pictures_bottom = body.max.y - INFO - ACTIONS;
        let middle = body.center().x;
        let left_cell = Rect::from_min_max(body.min, Pos2::new(middle - 0.5, pictures_bottom));
        let right_cell = Rect::from_min_max(Pos2::new(middle + 0.5, body.min.y), Pos2::new(body.max.x, pictures_bottom));
        ui.painter().rect_filled(Rect::from_min_max(body.min, Pos2::new(body.max.x, pictures_bottom)), 0.0, ui.visuals().extreme_bg_color);

        // All the input first, or the two would be drawn at different zooms for a frame.
        let cells = [("twins left", left_cell, left), ("twins right", right_cell, right)];
        for (id, cell, file) in cells {
            self.zoom_and_pan(ui, id, cell, file);
        }
        for (_, cell, file) in cells {
            self.picture(ui, cell, file);
        }

        for (cell, file, other) in [(left_cell, &left_file, &right_file), (right_cell, &right_file, &left_file)] {
            let rect = Rect::from_min_max(Pos2::new(cell.min.x, pictures_bottom), Pos2::new(cell.max.x, pictures_bottom + INFO));
            info(&mut ui.new_child(UiBuilder::new().max_rect(rect.shrink2(vec2(12.0, 4.0)))), file, other);
        }

        let row = Rect::from_min_max(Pos2::new(body.min.x, body.max.y - ACTIONS), body.max);
        ui.painter().hline(row.x_range(), row.min.y, ui.visuals().widgets.noninteractive.bg_stroke);
        let width = ((row.width() - 60.0) / 4.0).clamp(90.0, 190.0);
        let mut bar = ui.new_child(UiBuilder::new().max_rect(row).layout(Layout::left_to_right(Align::Center)));
        bar.spacing_mut().item_spacing.x = 8.0;
        bar.add_space((row.width() - width * 4.0 - 24.0).max(0.0) / 2.0);
        let danger = bar.visuals().error_fg_color;
        // Drawn, because not every font has arrows.
        for (text, choice, colour, arrow) in [
            ("Keep left", Choice::KeepLeft, None, Some(vec2(-1.0, 0.0))),
            ("Keep both", Choice::KeepBoth, None, Some(vec2(0.0, 1.0))),
            ("Del  ·  Delete both", Choice::DeleteBoth, Some(danger), None),
            ("Keep right", Choice::KeepRight, None, Some(vec2(1.0, 0.0))),
        ] {
            let label = RichText::new(text);
            let label = if let Some(c) = colour { label.color(c) } else { label };
            let stroke = bar.visuals().widgets.noninteractive.bg_stroke;
            let widget = egui::Button::new(label).min_size(vec2(width, 34.0)).stroke(stroke).corner_radius(0);
            let response = bar.add(widget);
            if let Some(direction) = arrow {
                let rect = response.rect;
                let at = if direction.x > 0.0 { rect.right_center() - vec2(24.0, 0.0) } else { rect.left_center() + vec2(24.0, 0.0) };
                let pen = Stroke::new(1.5, bar.visuals().text_color());
                bar.painter().arrow(at - direction * 6.0, direction * 12.0, pen);
            }
            if response.clicked() {
                *clicked = Some(Command::Choose(choice));
            }
        }
    }

    fn zoom_and_pan(&mut self, ui: &mut egui::Ui, id: &str, cell: Rect, file: u32) {
        let response = ui.interact(cell, egui::Id::new(id), Sense::click_and_drag());
        let Some(Slot::Ready(texture)) = self.slots.get(&file) else { return };
        let aspect = texture.aspect_ratio();
        if let Some(at) = response.hover_pos() {
            let factor = ui.input(|i| i.zoom_delta() * (i.smooth_scroll_delta.y * 0.004).exp());
            if factor != 1.0 {
                self.view.zoom_at(cell, aspect, at, factor);
            }
        }
        self.view.drag(cell, aspect, response.drag_delta());
        if response.double_clicked() {
            self.view = View::default();
        }
    }

    fn picture(&self, ui: &egui::Ui, cell: Rect, file: u32) {
        let painter = ui.painter_at(cell);
        match self.slots.get(&file) {
            Some(Slot::Ready(texture)) => {
                let uv = Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0));
                painter.image(texture.id(), self.view.rect(cell, texture.aspect_ratio()), uv, Color32::WHITE);
            }
            Some(Slot::Failed(why)) => {
                let colour = ui.visuals().error_fg_color;
                painter.text(cell.center(), egui::Align2::CENTER_CENTER, format!("Couldn't open it: {why}"), egui::FontId::proportional(14.0), colour);
            }
            _ => {
                let colour = ui.visuals().weak_text_color();
                painter.text(cell.center(), egui::Align2::CENTER_CENTER, "Loading…", egui::FontId::proportional(14.0), colour);
            }
        }
    }

    /// What's about to be deleted, or a note that there's nothing to do.
    fn last_page(&mut self, ui: &mut egui::Ui, outcome: &mut Outcome, clicked: &mut Option<Command>) {
        let marked: Vec<FileInfo> = self.review.marked().into_iter().cloned().collect();
        let failed = self.review.report().failed.len();
        let scanned = self.review.report().files.len();
        let took = self.review.report().took.as_secs_f32();
        let can_go_back = self.review.current().is_some();
        let finishing = &mut self.finishing;
        let threshold = self.review.threshold();
        let can_undo = self.review.can_undo();
        centred(ui, |ui| {
            if marked.is_empty() {
                ui.heading(if can_undo { "All done" } else { "No look-alikes here" });
                ui.label(format!("{scanned} pictures checked in {took:.1} s, at {:.0}% alike or more.", threshold * 100.0));
            } else {
                ui.heading(format!("{} marked, {}", files(marked.len()), bytes(marked.iter().map(|f| f.bytes).sum())));
                ui.add_space(4.0);
                egui::ScrollArea::vertical().max_height((ui.available_height() - 120.0).clamp(80.0, 420.0)).show(ui, |ui| {
                    ui.set_min_width(420.0);
                    for f in &marked {
                        ui.label(RichText::new(file_name(&f.path)).weak()).on_hover_text(f.path.display().to_string());
                    }
                });
            }
            if failed > 0 {
                ui.add_space(4.0);
                let list: Vec<String> =
                    self.review.report().failed.iter().take(20).map(|(p, why)| format!("{}: {why}", file_name(p))).collect();
                ui.colored_label(ui.visuals().error_fg_color, format!("Skipped {} that couldn't be read", files(failed)))
                    .on_hover_text(list.join("\n"));
            }
            ui.add_space(10.0);
            // Stacked: a row would ignore the centred layout.
            if !marked.is_empty() && button(ui, &format!("Move to {BIN}"), 200.0).clicked() {
                *outcome = Outcome::Delete;
            }
            if can_go_back && *finishing && button(ui, "Back to the pairs", 200.0).clicked() {
                *finishing = false;
            }
            if !can_go_back && can_undo && button(ui, "Undo last choice", 200.0).clicked() {
                *clicked = Some(Command::Undo);
            }
            let leave = if marked.is_empty() { "Close" } else { "Discard and close" };
            if button(ui, leave, 200.0).clicked() {
                *outcome = Outcome::Close;
            }
        });
    }
}

/// Fresh presses only: holding a key must never run through a pile of delete choices. Taken out
/// of the event list before any widget sees them, so a focused slider or button can't react too.
fn keys(ctx: &egui::Context, reviewing: bool) -> Option<Command> {
    ctx.input_mut(|i| {
        let mut command = None;
        i.events.retain(|event| {
            let egui::Event::Key { key, pressed: true, repeat, modifiers, .. } = event else { return true };
            let mapped = match (*key, modifiers.any()) {
                (Key::ArrowLeft, false) if reviewing => Command::Choose(Choice::KeepLeft),
                (Key::ArrowRight, false) if reviewing => Command::Choose(Choice::KeepRight),
                (Key::ArrowDown | Key::Space, false) if reviewing => Command::Choose(Choice::KeepBoth),
                (Key::Delete, false) if reviewing => Command::Choose(Choice::DeleteBoth),
                (Key::S, false) if reviewing => Command::Choose(Choice::KeepSet),
                (Key::Z | Key::Backspace, false) => Command::Undo,
                (Key::Z, true) if modifiers.command && !modifiers.shift && !modifiers.alt => Command::Undo,
                (Key::Escape, _) => Command::Close,
                _ => return true,
            };
            if !repeat && command.is_none() {
                command = Some(mapped);
            }
            false
        });
        command
    })
}

fn info(ui: &mut egui::Ui, file: &FileInfo, other: &FileInfo) {
    ui.spacing_mut().item_spacing.y = 2.0;
    ui.add(egui::Label::new(RichText::new(file_name(&file.path)).strong()).truncate())
        .on_hover_text(file.path.display().to_string());
    // What's better than the other side stands out; what's worse fades.
    let shade = |text: String, order: std::cmp::Ordering| match order {
        std::cmp::Ordering::Greater => RichText::new(text).strong(),
        std::cmp::Ordering::Less => RichText::new(text).weak(),
        std::cmp::Ordering::Equal => RichText::new(text),
    };
    let pixels = |f: &FileInfo| f.size[0] as u64 * f.size[1] as u64;
    ui.horizontal(|ui| {
        ui.label(shade(format!("{} × {}", file.size[0], file.size[1]), pixels(file).cmp(&pixels(other))));
        ui.label(shade(bytes(file.bytes), file.bytes.cmp(&other.bytes)));
        if let Some(date) = file.modified.and_then(date) {
            ui.label(RichText::new(date).weak());
        }
        let extension = file.path.extension().map(|e| e.to_string_lossy().to_uppercase()).unwrap_or_default();
        ui.label(RichText::new(extension).weak());
    });
}

fn plain(text: &str, width: f32) -> egui::Button<'_> {
    egui::Button::new(text).min_size(vec2(width, 26.0)).corner_radius(0)
}

fn button(ui: &mut egui::Ui, text: &str, width: f32) -> egui::Response {
    let stroke: Stroke = ui.visuals().widgets.noninteractive.bg_stroke;
    ui.add(plain(text, width).stroke(stroke))
}

fn centred(ui: &mut egui::Ui, content: impl FnOnce(&mut egui::Ui)) {
    let rect = ui.max_rect();
    let top = rect.shrink2(vec2(20.0, (rect.height() * 0.22).min(180.0)));
    let mut inner = ui.new_child(UiBuilder::new().max_rect(top).layout(Layout::top_down(Align::Center)));
    content(&mut inner);
}

fn file_name(path: &Path) -> String {
    path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default()
}

fn files(n: usize) -> String {
    if n == 1 { "1 file".to_owned() } else { format!("{n} files") }
}

fn bytes(n: u64) -> String {
    match n {
        0..1_000 => format!("{n} B"),
        1_000..1_000_000 => format!("{:.0} KB", n as f64 / 1e3),
        1_000_000..1_000_000_000 => format!("{:.1} MB", n as f64 / 1e6),
        _ => format!("{:.2} GB", n as f64 / 1e9),
    }
}

/// YYYY-MM-DD in UTC, without pulling in a date library for one label.
fn date(time: SystemTime) -> Option<String> {
    let days = (time.duration_since(UNIX_EPOCH).ok()?.as_secs() / 86_400) as i64;
    // Howard Hinnant's civil_from_days.
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let (day, month) = (doy - (153 * mp + 2) / 5 + 1, if mp < 10 { mp + 3 } else { mp - 9 });
    let year = yoe + era * 400 + i64::from(month <= 2);
    Some(format!("{year:04}-{month:02}-{day:02}"))
}

/// Zoom and pan, shared by both pictures so the same spot stays side by side. `pan` is a fraction
/// of the shown picture, which lines them up even when their resolutions differ.
#[derive(Clone, Copy)]
struct View {
    zoom: f32,
    pan: Vec2,
}

impl Default for View {
    fn default() -> Self {
        Self { zoom: 1.0, pan: Vec2::ZERO }
    }
}

impl View {
    const MAX_ZOOM: f32 = 24.0;

    fn size(&self, cell: Rect, aspect: f32) -> Vec2 {
        let fit = (cell.width() / aspect).min(cell.height());
        vec2(fit * aspect, fit) * self.zoom
    }

    fn rect(&self, cell: Rect, aspect: f32) -> Rect {
        let size = self.size(cell, aspect);
        Rect::from_center_size(cell.center() + self.pan * size, size)
    }

    fn clamp(&mut self) {
        self.zoom = self.zoom.clamp(1.0, Self::MAX_ZOOM);
        // At fit size there's nothing to pan; further in, an edge can reach the middle.
        let reach = if self.zoom <= 1.0 { 0.0 } else { 0.5 };
        self.pan = vec2(self.pan.x.clamp(-reach, reach), self.pan.y.clamp(-reach, reach));
    }

    /// Keeps the spot under the pointer where it is.
    fn zoom_at(&mut self, cell: Rect, aspect: f32, pointer: Pos2, factor: f32) {
        let before = self.rect(cell, aspect);
        let spot = (pointer - before.center()) / before.size();
        self.zoom = (self.zoom * factor).clamp(1.0, Self::MAX_ZOOM);
        self.pan = (pointer - cell.center()) / self.size(cell, aspect) - spot;
        self.clamp();
    }

    fn drag(&mut self, cell: Rect, aspect: f32, delta: Vec2) {
        if delta != Vec2::ZERO {
            self.pan += delta / self.size(cell, aspect);
            self.clamp();
        }
    }
}

/// Decodes the pictures on show on a couple of threads. Whatever was asked for last goes first.
struct Loader {
    shared: Arc<LoaderShared>,
    results: Receiver<(u32, Result<Picture, String>)>,
}

#[derive(Default)]
struct LoaderQueue {
    files: Vec<u32>,
    stop: bool,
}

#[derive(Default)]
struct LoaderShared {
    queue: Mutex<LoaderQueue>,
    wake: Condvar,
}

impl Loader {
    fn start(paths: Vec<PathBuf>, decode: Decode, max_side: u32, ctx: egui::Context) -> Self {
        let shared = Arc::new(LoaderShared::default());
        let paths: Arc<Vec<PathBuf>> = Arc::new(paths);
        let (send, results) = mpsc::channel();
        for _ in 0..LOAD_THREADS {
            let (shared, paths, send, ctx) = (shared.clone(), paths.clone(), send.clone(), ctx.clone());
            thread::spawn(move || {
                loop {
                    let file = {
                        let mut queue = shared.queue.lock().unwrap();
                        loop {
                            if queue.stop {
                                return;
                            }
                            if !queue.files.is_empty() {
                                break queue.files.remove(0);
                            }
                            queue = shared.wake.wait(queue).unwrap();
                        }
                    };
                    let result = std::panic::catch_unwind(|| decode(&paths[file as usize], max_side))
                        .unwrap_or_else(|_| Err("the decoder crashed on it".to_owned()));
                    if send.send((file, result)).is_err() {
                        return;
                    }
                    ctx.request_repaint();
                }
            });
        }
        Self { shared, results }
    }

    fn want(&self, files: Vec<u32>) {
        self.shared.queue.lock().unwrap().files = files;
        self.shared.wake.notify_all();
    }
}

impl Drop for Loader {
    fn drop(&mut self) {
        self.shared.queue.lock().unwrap().stop = true;
        self.shared.wake.notify_all();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cell() -> Rect {
        Rect::from_min_size(Pos2::new(100.0, 50.0), vec2(800.0, 600.0))
    }

    #[test]
    fn fits_inside_the_cell_at_start() {
        let v = View::default();
        let wide = v.rect(cell(), 2.0);
        assert_eq!(wide.size(), vec2(800.0, 400.0));
        assert_eq!(wide.center(), cell().center());
        let tall = v.rect(cell(), 0.5);
        assert_eq!(tall.size(), vec2(300.0, 600.0));
    }

    #[test]
    fn zoom_keeps_the_spot_under_the_pointer() {
        let mut v = View::default();
        let at = Pos2::new(300.0, 200.0);
        let before = v.rect(cell(), 4.0 / 3.0);
        let spot = (at - before.min) / before.size();
        v.zoom_at(cell(), 4.0 / 3.0, at, 3.0);
        let after = v.rect(cell(), 4.0 / 3.0);
        let moved = (at - after.min) / after.size();
        assert!((spot - moved).length() < 1e-3, "{spot:?} {moved:?}");
    }

    #[test]
    fn zooming_back_out_recentres() {
        let mut v = View::default();
        v.zoom_at(cell(), 1.0, Pos2::new(120.0, 60.0), 8.0);
        v.drag(cell(), 1.0, vec2(300.0, 300.0));
        v.zoom_at(cell(), 1.0, Pos2::new(500.0, 500.0), 0.01);
        assert_eq!(v.zoom, 1.0);
        assert_eq!(v.pan, Vec2::ZERO);
    }

    #[test]
    fn pictures_of_different_sizes_stay_lined_up() {
        // Same shape, different cells: the same fraction of the picture lands in the same place.
        let mut v = View::default();
        v.zoom_at(cell(), 1.5, Pos2::new(500.0, 300.0), 4.0);
        v.drag(cell(), 1.5, vec2(-120.0, 40.0));
        let other = cell().translate(vec2(800.0, 0.0));
        let (a, b) = (v.rect(cell(), 1.5), v.rect(other, 1.5));
        assert!(((a.min - cell().min) - (b.min - other.min)).length() < 1e-3);
    }

    /// Runs the real panel through egui with no window: pointer, wheel and key events go in the
    /// same way the OS would send them.
    struct Driver {
        ctx: egui::Context,
        panel: Panel,
        pointer: Pos2,
        events: Vec<Event>,
        dir: PathBuf,
    }

    fn wave(width: u32, height: u32, seed: f32) -> image::RgbaImage {
        image::RgbaImage::from_fn(width, height, |x, y| {
            let (u, v) = (x as f32 / width as f32, y as f32 / height as f32);
            let at = |a: f32, b: f32| (((u * a + seed).sin() + (v * b + seed * 1.7).cos()) * 60.0 + 128.0) as u8;
            image::Rgba([at(9.0, 7.0), at(5.0, 11.0), at(13.0, 3.0), 255])
        })
    }

    impl Driver {
        /// Folder: a.png, its byte copy, a smaller version of it, a version with a block painted
        /// on (alike, but below 90%), and one unrelated picture.
        fn new(name: &str, threshold: f32) -> Self {
            let dir = std::env::temp_dir().join(format!("twins-test-{}-{name}", std::process::id()));
            let _ = std::fs::remove_dir_all(&dir);
            std::fs::create_dir_all(&dir).unwrap();
            let a = wave(400, 300, 1.0);
            a.save(dir.join("a.png")).unwrap();
            std::fs::copy(dir.join("a.png"), dir.join("a_copy.png")).unwrap();
            wave(200, 150, 1.0).save(dir.join("a_small.png")).unwrap();
            let mut blocked = a.clone();
            for y in 90..210 {
                for x in 120..280 {
                    blocked.put_pixel(x, y, image::Rgba([235, 40, 40, 255]));
                }
            }
            blocked.save(dir.join("a_blocked.png")).unwrap();
            wave(400, 300, 4.0).save(dir.join("other.png")).unwrap();

            let remove: Remove = |path| std::fs::remove_file(path).map_err(|e| e.to_string());
            let panel = Panel::new(Options::new(&dir), threshold, remove);
            let mut driver = Self { ctx: egui::Context::default(), panel, pointer: Pos2::ZERO, events: Vec::new(), dir };
            driver.until("the scan to end", |p| matches!(p.page, Page::Reviewing(_)));
            driver.until("the pictures to load", |p| {
                let Page::Reviewing(state) = &p.page else { return false };
                state.wanted.len() >= 2 && state.wanted[..2].iter().all(|f| matches!(state.slots.get(f), Some(Slot::Ready(_))))
            });
            driver
        }

        fn frame(&mut self, events: Vec<egui::Event>) {
            let input = egui::RawInput {
                screen_rect: Some(Rect::from_min_size(Pos2::ZERO, vec2(1280.0, 800.0))),
                events,
                predicted_dt: 1.0 / 60.0,
                ..Default::default()
            };
            let (panel, out) = (&mut self.panel, &mut self.events);
            let output = self.ctx.run_ui(input, |ui| {
                egui::CentralPanel::default().frame(egui::Frame::NONE).show(ui, |ui| out.extend(panel.show(ui)));
            });
            output.drop_without_applying_deltas();
        }

        fn until(&mut self, what: &str, done: impl Fn(&Panel) -> bool) {
            for _ in 0..2000 {
                self.frame(Vec::new());
                if done(&self.panel) {
                    return;
                }
                std::thread::sleep(Duration::from_millis(2));
            }
            panic!("timed out waiting for {what}");
        }

        fn state(&self) -> &Reviewing {
            match &self.panel.page {
                Page::Reviewing(state) => state,
                _ => panic!("not on the review page"),
            }
        }

        fn move_to(&mut self, pos: Pos2) {
            self.pointer = pos;
            self.frame(vec![egui::Event::PointerMoved(pos)]);
        }

        fn button(&mut self, pressed: bool) {
            let pos = self.pointer;
            let modifiers = egui::Modifiers::NONE;
            self.frame(vec![egui::Event::PointerButton { pos, button: egui::PointerButton::Primary, pressed, modifiers }]);
        }

        fn click(&mut self, pos: Pos2) {
            self.move_to(pos);
            self.button(true);
            self.button(false);
            self.frame(Vec::new());
        }

        fn key_event(&mut self, key: Key, pressed: bool) {
            let modifiers = egui::Modifiers::NONE;
            self.frame(vec![egui::Event::Key { key, physical_key: None, pressed, repeat: false, modifiers }]);
        }

        fn key(&mut self, key: Key) {
            self.key_event(key, true);
            self.key_event(key, false);
        }

        fn wheel(&mut self, up: f32) {
            for _ in 0..30 {
                let event = egui::Event::MouseWheel {
                    unit: egui::MouseWheelUnit::Point,
                    delta: vec2(0.0, up),
                    phase: egui::TouchPhase::Move,
                    modifiers: egui::Modifiers::NONE,
                };
                self.frame(vec![event]);
            }
            // Eased scrolling runs on for a while after the wheel stops.
            for _ in 0..60 {
                self.frame(Vec::new());
            }
        }

        fn marked(&self) -> Vec<String> {
            self.state().review.marked_paths().iter().map(|p| file_name(p)).collect()
        }

        fn on_disk(&self) -> Vec<String> {
            let mut names: Vec<String> =
                std::fs::read_dir(&self.dir).unwrap().flatten().map(|e| file_name(&e.path())).collect();
            names.sort();
            names
        }
    }

    impl Drop for Driver {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.dir);
        }
    }

    #[test]
    fn keys_choose_undo_and_ignore_a_held_key() {
        let mut d = Driver::new("keys", 0.9);
        assert!(d.state().review.current().unwrap().pair.identical);
        // Pressed again and again with no release is how a held key arrives.
        for _ in 0..6 {
            d.key_event(Key::ArrowLeft, true);
        }
        d.key_event(Key::ArrowLeft, false);
        assert_eq!(d.marked().len(), 1, "a held key must count once");
        d.key(Key::Z);
        assert!(d.marked().is_empty());
        d.key(Key::ArrowDown);
        assert!(d.marked().is_empty());
        assert!(d.state().review.can_undo());
        d.key(Key::Delete);
        assert_eq!(d.marked().len(), 2);
    }

    #[test]
    fn slider_changes_the_threshold_without_rescanning() {
        let mut d = Driver::new("slider", 1.0);
        let before = d.state().review.report().took;
        // a and its byte copy are the only pair at 100%.
        d.key(Key::ArrowLeft);
        assert!(d.state().review.current().is_none());
        // Press along the bar until the slider answers, as a click there would.
        for x in (150..700).step_by(10) {
            d.move_to(Pos2::new(x as f32, 22.0));
            d.button(true);
            d.button(false);
            if d.state().review.threshold() < 1.0 {
                break;
            }
        }
        let now = d.state().review.threshold();
        assert!((0.5..1.0).contains(&now), "slider never moved: {now}");
        assert_eq!(d.panel.threshold(), now);
        assert_eq!(d.state().review.report().took, before);
        d.frame(Vec::new());
        assert!(d.state().review.current().is_some(), "loosening should bring pairs up at {now}");
        assert_eq!(d.marked().len(), 1, "earlier choices stay");
    }

    #[test]
    fn wheel_zooms_drag_pans_and_double_click_resets() {
        let mut d = Driver::new("zoom", 0.9);
        let spot = Pos2::new(320.0, 300.0);
        d.move_to(spot);
        d.wheel(10.0);
        let zoomed = d.state().view;
        assert!(zoomed.zoom > 1.2, "wheel over a picture should zoom: {}", zoomed.zoom);

        d.button(true);
        d.move_to(spot + vec2(40.0, 25.0));
        d.move_to(spot + vec2(60.0, 30.0));
        d.button(false);
        let panned = d.state().view;
        assert_ne!(panned.pan, zoomed.pan, "dragging should pan");

        for _ in 0..2 {
            d.button(true);
            d.button(false);
        }
        assert_eq!(d.state().view.zoom, 1.0, "double click goes back to fit size");

        d.wheel(10.0);
        assert!(d.state().view.zoom > 1.2);
        d.key(Key::ArrowDown);
        d.frame(Vec::new());
        assert_eq!(d.state().view.zoom, 1.0, "the next pair starts at fit size");
    }

    #[test]
    fn buttons_and_the_last_page_delete_only_whats_marked() {
        let mut d = Driver::new("delete", 0.9);
        assert_eq!(d.on_disk().len(), 5);

        // Keep left is the first of the four buttons along the bottom.
        d.click(Pos2::new(300.0, 772.0));
        assert_eq!(d.marked().len(), 1);
        d.key(Key::S); // Keep the rest of the set.
        d.frame(Vec::new());
        assert!(d.state().review.current().is_none());
        let doomed = d.marked();
        assert_eq!(doomed.len(), 1);
        assert_eq!(d.on_disk().len(), 5, "nothing goes before the last button");

        // Move to Recycle Bin is the topmost button of the centred stack.
        for y in (120..500).step_by(8) {
            d.click(Pos2::new(640.0, y as f32));
            if !matches!(d.panel.page, Page::Reviewing(_)) {
                break;
            }
        }
        d.until("the files to go", |p| matches!(p.page, Page::Finished(_)));
        d.frame(Vec::new());
        assert!(matches!(d.events.as_slice(), [Event::Deleted(gone)] if gone.len() == 1));
        let left = d.on_disk();
        assert_eq!(left.len(), 4);
        assert!(!left.contains(&doomed[0]));
    }

    #[test]
    fn escape_shows_marked_files_before_closing() {
        let mut d = Driver::new("escape", 0.9);
        d.key(Key::ArrowLeft);
        d.key(Key::Escape);
        d.frame(Vec::new());
        assert!(d.state().finishing);
        assert!(d.events.is_empty(), "marked files must not be dropped on one key");
        d.key(Key::Escape);
        d.frame(Vec::new());
        assert!(!d.state().finishing, "a second Escape goes back to the pairs");

        let mut clean = Driver::new("escape-clean", 0.9);
        clean.key(Key::Escape);
        assert!(matches!(clean.events.as_slice(), [Event::Closed]));
    }

    #[test]
    fn labels() {
        assert_eq!(bytes(999), "999 B");
        assert_eq!(bytes(1_500_000), "1.5 MB");
        assert_eq!(date(UNIX_EPOCH).unwrap(), "1970-01-01");
        assert_eq!(date(UNIX_EPOCH + Duration::from_secs(1_709_164_800)).unwrap(), "2024-02-29");
    }
}
