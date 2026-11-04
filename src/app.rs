//! App state, navigation and the preload cache. Drawing lives in `canvas.rs` (the image) and
//! `ui.rs` (bars, menus, overlays).

use crate::files::{self, SortBy};
use crate::keys::{self, Action, Keymap};
use crate::loader::{Image, Loader};
use crate::system;
use crate::theme::{self, Icons};
use eframe::egui::{self, Key, PointerButton, Rect, TextureFilter, TextureHandle, TextureOptions, Vec2};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

pub const APP_NAME: &str = "TelescopeView";
/// Thumbnails are decoded this many pixels on their long side.
pub const THUMB_SIDE: u32 = 256;
/// Strip mode: pages past the last visible one to have ready, and pages kept above.
const STRIP_AHEAD: usize = 6;
const STRIP_BEHIND: usize = 2;
/// Steps closer together than this count as flipping fast.
const FAST_STEP: Duration = Duration::from_millis(130);
/// Going past the last page arms the jump to the next folder; the jump itself needs a
/// separate push at least this much later, so one long wheel spin can't skip a chapter.
const HOP_GUARD: Duration = Duration::from_millis(400);

const SETTINGS_KEY: &str = "settings";
const HISTORY_MAX: usize = 300;
pub const SLIDESHOW_STEPS: &[u32] = &[1, 2, 3, 5, 8, 15, 30];
/// A decoded image on the GPU: frames (one unless animated), each a set of tiles.
pub struct Pic {
    pub frames: Vec<PicFrame>,
    pub original_size: [u32; 2],
    pub format: String,
    pub has_alpha: bool,
}

pub struct PicFrame {
    pub tiles: Vec<(TextureHandle, Rect)>,
    pub delay: Duration,
}

impl Pic {
    /// Which frame to show `t` after the animation started, and how long until the next one.
    pub fn frame_at(&self, t: Duration) -> (&PicFrame, Option<Duration>) {
        if self.frames.len() < 2 {
            return (&self.frames[0], None);
        }
        let total: Duration = self.frames.iter().map(|f| f.delay).sum();
        let mut t = Duration::from_nanos((t.as_nanos() % total.as_nanos().max(1)) as u64);
        for f in &self.frames {
            if t < f.delay {
                return (f, Some(f.delay - t));
            }
            t -= f.delay;
        }
        (&self.frames[0], Some(self.frames[0].delay))
    }

    pub fn total_time(&self) -> Duration {
        self.frames.iter().map(|f| f.delay).sum()
    }

    /// Which frame is up `t` into the animation.
    pub fn frame_index(&self, t: Duration) -> usize {
        let (f, _) = self.frame_at(t);
        self.frames.iter().position(|x| std::ptr::eq(x, f)).unwrap_or(0)
    }

    /// When frame `k` comes up.
    pub fn frame_start(&self, k: usize) -> Duration {
        self.frames[..k].iter().map(|f| f.delay).sum()
    }

    /// Size after rotating by `quarter_turns`.
    pub fn size_turned(&self, quarter_turns: u8) -> Vec2 {
        let [w, h] = self.original_size;
        if quarter_turns % 2 == 1 { Vec2::new(h as f32, w as f32) } else { Vec2::new(w as f32, h as f32) }
    }
}

/// An orientation: bits 0-1 are quarter turns clockwise, bit 2 mirrors left to right first.
/// Covers all eight ways a picture can lie, like the EXIF orientation tag.
pub const FLIP_H: u8 = 4;
/// Upside down: mirrored, then half a turn.
pub const FLIP_V: u8 = 6;

/// `a`, then `b` on top of it.
pub fn compose(a: u8, b: u8) -> u8 {
    let (ta, tb) = (a & 3, b & 3);
    // A mirror reverses the turns made before it.
    let turns = if b & 4 != 0 { tb + 4 - ta } else { tb + ta };
    (turns & 3) | ((a ^ b) & 4)
}

pub enum Slot {
    Ready(Arc<Pic>),
    Failed(String),
}

/// What is on screen right now. Kept apart from the cache: while the next image is still
/// decoding this keeps showing the previous one instead of a blank frame.
pub struct Shown {
    pub path: PathBuf,
    pub pic: Arc<Pic>,
    pub since: Instant,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug, Default, Serialize, Deserialize)]
pub enum Fit {
    /// Whole image visible.
    #[default]
    Page,
    /// Fill the width and scroll down (comic pages).
    Width,
}

#[derive(Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    pub enlarge_small: bool,
    pub fit: Fit,
    pub slideshow_secs: u32,
    pub sort: SortBy,
    pub sort_desc: bool,
    pub show_info: bool,
    /// Most recent first: a folder (or archive) and the page last viewed in it, relative to
    /// it. Reopening resumes there, and the first few are the "recent" list.
    pub history: Vec<(PathBuf, String)>,
    /// Folders read as one continuous vertical strip (webtoons).
    pub strip_dirs: Vec<PathBuf>,
    /// Strip column width as a share of the window width.
    pub strip_width: f32,
    pub theme: theme::ThemePref,
    /// Two pages side by side, like an open book.
    pub spread: bool,
    /// Right to left (manga): the first page of a spread sits on the right and ← reads on.
    pub rtl: bool,
    /// Spread and direction remembered per series (the folder the chapters are in).
    pub series_layout: Vec<(PathBuf, bool, bool)>,
    pub bookmarks: Vec<Bookmark>,
    pub high_contrast: bool,
    /// No eased scrolling or blinking.
    pub reduce_motion: bool,
    /// Fullscreen keeps the bars on screen instead of hiding them.
    pub keep_bars: bool,
    /// Clicking the left or right third of the page turns it.
    pub click_zones: bool,
    /// The next image keeps the zoom (and the spot zoomed into) instead of fitting again.
    pub lock_zoom: bool,
    /// What going past the last (or first) image does.
    pub at_end: AtEnd,
    /// The mouse wheel scrolls and turns pages instead of zooming (videos always zoom).
    pub wheel_turns: bool,
    /// Folders reopen at the image last viewed in them.
    pub resume: bool,
    pub show_page_bar: bool,
    /// The toolbar buttons that are shown (keys from `ui::TOOLBAR_BUTTONS`).
    pub toolbar: Vec<String>,
    /// Shortcuts changed in Settings: action id to its keys (see `keys::DEFS`).
    pub keys: BTreeMap<String, Vec<String>>,
    /// Wide pages (double-page scans) are read one half at a time.
    pub split_wide: bool,
    /// The thumbnail panel: on, only while the mouse is at its edge, and where it sits.
    pub thumbs_on: bool,
    /// The thumbnail panel stays on screen; otherwise it slides out at its edge.
    pub thumbs_pinned: bool,
    pub thumbs_dock: Dock,
    pub thumb_size: f32,
    /// A small picture of the page under the mouse on the position bar.
    pub bar_preview: bool,
    /// Images decoded ahead of the one on screen (and half as many behind).
    pub preload: usize,
    /// Decoder threads, 0 for automatic.
    pub decode_threads: usize,
    pub thumb_threads: usize,
    /// Opening a file while TelescopeView is open shows it in that window.
    pub single_instance: bool,
    /// Video sound, 0..1.
    pub volume: f32,
    pub muted: bool,
    /// The folder bookmark files were last saved in.
    pub mark_dir: Option<PathBuf>,
    /// Now and then, offer to become the default viewer (Windows and Linux).
    pub ask_default: bool,
    /// Extensions left to other apps when making TelescopeView the default.
    pub default_off: Vec<String>,
    /// When that was last offered, in seconds since 1970 (0: never).
    pub default_asked_at: u64,
    /// ← → pan a zoomed-in image sideways, turning only at its edge.
    pub arrows_pan: bool,
    /// A small "12 / 240" in the corner while the bars are hidden in fullscreen.
    pub page_number_fullscreen: bool,
    /// A checkerboard behind see-through images.
    pub checker: bool,
    pub backdrop: Backdrop,
    /// Hard pixel edges when zoomed in (pixel art, screenshots) instead of smoothing.
    pub sharp_pixels: bool,
    /// Esc with nothing else to close closes the window (quick-look style).
    pub esc_closes: bool,
    pub start_fullscreen: bool,
    pub confirm_delete: bool,
    /// Seconds before the bars and cursor hide in fullscreen.
    pub bars_hide_secs: f32,
    /// Zoom factor per + / − press or wheel notch.
    pub zoom_step: f32,
    /// The slideshow stops at the last image instead of starting over.
    pub slideshow_stops: bool,
}

/// What's behind the image.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default, Serialize, Deserialize)]
pub enum Backdrop {
    #[default]
    Theme,
    Black,
    Gray,
    White,
}

/// Where the thumbnail panel sits.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default, Serialize, Deserialize)]
pub enum Dock {
    #[default]
    Left,
    Right,
    Top,
    Bottom,
    /// A small window of its own, dragged anywhere.
    Floating,
}

/// The page of the Settings panel that is open.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum SettingsPage {
    #[default]
    Viewing,
    Browsing,
    Comics,
    Thumbnails,
    Slideshow,
    Look,
    Window,
    Files,
    Performance,
    Toolbar,
    Shortcuts,
}

/// Going past the last image (or before the first).
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default, Serialize, Deserialize)]
pub enum AtEnd {
    /// Stay there.
    Stop,
    /// Back to the first image of the same folder.
    Loop,
    /// The same move once more opens the next folder (chapter).
    #[default]
    NextFolderOnSecondPush,
    /// Straight on into the next folder.
    NextFolder,
}

/// A page saved to come back to.
#[derive(Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Bookmark {
    /// The folder or archive.
    pub dir: PathBuf,
    /// The page inside it (see `files::relative_name`).
    pub page: String,
    /// Page number when it was saved, 1-based, for showing in lists.
    pub number: usize,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            enlarge_small: true,
            fit: Fit::Page,
            slideshow_secs: 5,
            sort: SortBy::Name,
            sort_desc: false,
            show_info: false,
            history: Vec::new(),
            strip_dirs: Vec::new(),
            strip_width: 1.0,
            theme: theme::ThemePref::System,
            spread: false,
            rtl: false,
            series_layout: Vec::new(),
            bookmarks: Vec::new(),
            high_contrast: false,
            reduce_motion: false,
            keep_bars: false,
            click_zones: true,
            lock_zoom: false,
            at_end: AtEnd::NextFolderOnSecondPush,
            wheel_turns: false,
            resume: true,
            show_page_bar: true,
            toolbar: crate::ui::DEFAULT_TOOLBAR.iter().map(|k| (*k).to_owned()).collect(),
            keys: BTreeMap::new(),
            split_wide: false,
            thumbs_on: false,
            thumbs_pinned: false,
            thumbs_dock: Dock::Left,
            thumb_size: 96.0,
            bar_preview: true,
            preload: 4,
            decode_threads: 0,
            thumb_threads: 2,
            single_instance: true,
            mark_dir: None,
            volume: 0.8,
            muted: false,
            ask_default: true,
            default_off: Vec::new(),
            default_asked_at: 0,
            arrows_pan: true,
            page_number_fullscreen: true,
            checker: true,
            backdrop: Backdrop::Theme,
            sharp_pixels: false,
            esc_closes: false,
            start_fullscreen: false,
            confirm_delete: false,
            bars_hide_secs: 1.8,
            zoom_step: 1.25,
            slideshow_stops: false,
        }
    }
}

/// Where to start in a folder that's being opened.
pub enum Start {
    File(PathBuf),
    /// Where the user left off last time, or the first page.
    Resume,
    First,
    Last,
    /// A page by its name inside the folder (bookmarks).
    Page(String),
}

pub struct App {
    pub loader: Loader,
    /// The open folder or archive.
    pub dir: Option<PathBuf>,
    pub files: Vec<PathBuf>,
    pub index: usize,
    forward: bool,
    last_step: Option<Instant>,
    fast: bool,
    pub cache: HashMap<PathBuf, Slot>,
    /// Decoded but not on the GPU yet (see the upload budget in `refresh`).
    decoded: HashMap<PathBuf, Image>,
    uploaded_in_frame: Option<u64>,
    pub shown: Option<Shown>,
    /// None = follow the fit mode. Otherwise 1.0 = one image pixel per screen pixel.
    pub zoom: Option<f32>,
    pub pan: Vec2,
    /// Where a newly shown page starts when it is taller than the window.
    enter_at_bottom: bool,
    /// Geometry of the last drawn frame, for keyboard scrolling and strip layout.
    pub slack: Vec2,
    pub view_size: Vec2,
    pub ppp: f32,
    pub effective_zoom: Option<f32>,
    /// A zoom picked on the toolbar slider, applied around the middle by the canvas.
    pub zoom_request: Option<f32>,
    pub wheel_acc: f32,
    /// When the wheel last turned a page (touchpad momentum guard).
    pub last_turn: Option<Instant>,
    /// Quarter turns clockwise, per image, for this session.
    pub rotation: HashMap<PathBuf, u8>,
    pub settings: Settings,
    /// Strip mode: reading the open folder as one long vertical strip.
    pub strip: bool,
    /// How far the top of the view is below the top of the current page, in points.
    pub strip_offset: f32,
    /// Scrolling still to be animated, in points.
    pub strip_pending: f32,
    pub strip_tick: Option<Instant>,
    /// Last page drawn in the strip, for preloading.
    pub strip_last_visible: usize,
    /// Height / width of the last page measured, for pages that aren't decoded yet.
    pub strip_aspect: f32,
    pub slideshow: Option<Instant>,
    /// Armed by trying to go past the first/last page: direction, the folder that the same
    /// move again will open, and when it was armed.
    pub hop: Option<(bool, Option<PathBuf>, Instant)>,
    pub toast: Option<(String, Instant)>,
    pub show_help: bool,
    pub show_settings: bool,
    /// Waiting for Enter / Esc before deleting this file ("Ask before deleting").
    pub confirm_delete: Option<PathBuf>,
    /// The folders next to the open one, for the Chapters menu: (parent, list).
    pub chapters: Option<(PathBuf, Vec<PathBuf>)>,
    pub checker_tex: egui::TextureHandle,
    /// Wheel notches turned into zoom (when the wheel zooms), applied around the cursor.
    pub wheel_zoom: f32,
    /// When the wheel or a key last hopped straight into another folder (`AtEnd::NextFolder`).
    last_hop: Option<Instant>,
    pub last_activity: Instant,
    pub icons: Icons,
    pub error: Option<String>,
    title: String,
    pub stats: Stats,
    pub started: Instant,
    /// The video on screen, playing.
    pub video: Option<crate::video::Player>,
    /// The video last started (or that failed to), so a broken one isn't retried every frame.
    pub video_tried: Option<PathBuf>,
    /// Videos being started: the one on screen, and the ones either side.
    pub video_opening: Vec<crate::video::Opening>,
    /// Where the seek bar is being dragged to, in seconds.
    pub video_drag: Option<f64>,
    /// The "make TelescopeView the default" offer is due this session.
    pub offer_default: bool,
    /// The default card was opened from the menu (no "don't ask again", no waiting).
    pub default_menu: bool,
    /// It shows every format, to pick one by one.
    pub default_formats: bool,
    /// The rename box's text while it is open.
    pub renaming: Option<String>,
    /// The "go to page" box's text while it is open.
    pub goto: Option<String>,
    applied_theme: Option<(bool, bool)>,
    reveal: Reveal,
    /// File size and date for the info panel, read once per page rather than every frame.
    pub info_meta: Option<InfoMeta>,
    pub keymap: Keymap,
    /// Settings waits for a key to bind to this action (true: added next to the ones it has).
    pub capturing: Option<(Action, bool)>,
    /// A paused animation: how far into it, frozen.
    pub anim_pause: Option<Duration>,
    pub settings_page: SettingsPage,
    /// Small copies for the thumbnail panel and the position bar (None: couldn't be read).
    pub thumb_loader: Loader,
    pub thumbs: HashMap<PathBuf, Option<TextureHandle>>,
    /// Thumbnails drawn (or wanted) this frame, asked for at the start of the next.
    pub thumb_want: Vec<PathBuf>,
    thumb_asked: Vec<PathBuf>,
    /// The hover-only thumbnail panel stays out until this time (a moment after the mouse leaves).
    pub thumbs_until: f64,
    /// The thumbnail panel is being dragged to another side.
    pub thumbs_drag: bool,
    /// Where the pictures go (between the bars), for working out the side a drag lands on.
    pub thumbs_area: Rect,
    /// The thumbnail list's visible span (start, end) and the picture it was measured at.
    pub thumbs_view: Option<(f32, f32, usize, u8)>,
    /// Which half of a split wide page is up: 0 the first in reading order, 1 the second.
    pub half: u8,
    /// A file to open, from another thread (a second launch, or a paste).
    pub pending_open: Arc<std::sync::Mutex<Option<PathBuf>>>,
    /// Shared with the single-instance listener: whether to take files from new launches.
    pub take_launches: Arc<std::sync::atomic::AtomicBool>,
    max_side: u32,
    /// Files whose rotation was just saved: the on-screen turn goes once they're decoded again.
    pub unrotate: Vec<PathBuf>,
    /// Messages from work done off the UI thread, shown as toasts.
    inbox: Arc<std::sync::Mutex<Vec<String>>>,
}

/// What the info panel reads from disk, once per page: size and dates, and a photo's EXIF.
pub struct InfoMeta {
    pub path: PathBuf,
    pub meta: Option<std::fs::Metadata>,
    pub exif: Vec<(&'static str, String)>,
}

/// `IV_STATS=1` logs to stderr whether each image change was instant or had to wait.
#[derive(Default)]
pub struct Stats {
    enabled: bool,
    hits: u32,
    misses: u32,
    waiting: Option<Instant>,
}

impl App {
    pub fn new(cc: &eframe::CreationContext<'_>, initial: Option<PathBuf>) -> Self {
        theme::apply(&cc.egui_ctx, true, false);
        // Asked from the GPU itself: egui's own `max_texture_side` is still a 2048 placeholder
        // before the first frame, which would shrink every big photo (and slowly).
        let max_side = gpu_max_texture_side(cc).clamp(2048, 16384);
        let settings: Settings = cc.storage.and_then(|s| eframe::get_value(s, SETTINGS_KEY)).unwrap_or_default();
        let (thumb_threads, single_instance) = (settings.thumb_threads, settings.single_instance);
        let mut app = Self {
            loader: Loader::new(cc.egui_ctx.clone(), max_side, settings.decode_threads),
            dir: None,
            files: Vec::new(),
            index: 0,
            forward: true,
            last_step: None,
            fast: false,
            cache: HashMap::new(),
            decoded: HashMap::new(),
            uploaded_in_frame: None,
            shown: None,
            zoom: None,
            pan: Vec2::ZERO,
            enter_at_bottom: false,
            slack: Vec2::ZERO,
            view_size: Vec2::new(1280.0, 760.0),
            ppp: 1.0,
            effective_zoom: None,
            zoom_request: None,
            wheel_acc: 0.0,
            last_turn: None,
            rotation: HashMap::new(),
            keymap: Keymap::new(&settings.keys),
            settings,
            strip: false,
            strip_offset: 0.0,
            strip_pending: 0.0,
            strip_tick: None,
            strip_last_visible: 0,
            strip_aspect: 1.5,
            slideshow: None,
            hop: None,
            toast: None,
            show_help: false,
            show_settings: false,
            confirm_delete: None,
            chapters: None,
            checker_tex: checker_texture(&cc.egui_ctx),
            wheel_zoom: 1.0,
            last_hop: None,
            last_activity: Instant::now(),
            icons: Icons::load(&cc.egui_ctx),
            error: None,
            title: String::new(),
            stats: Stats { enabled: std::env::var_os("IV_STATS").is_some(), ..Default::default() },
            started: Instant::now(),
            offer_default: false,
            default_menu: false,
            default_formats: false,
            video: None,
            video_tried: None,
            video_opening: Vec::new(),
            video_drag: None,
            renaming: None,
            goto: None,
            applied_theme: None,
            reveal: Reveal::Waiting,
            info_meta: None,
            capturing: None,
            anim_pause: None,
            settings_page: SettingsPage::Viewing,
            thumb_loader: Loader::thumbnails(cc.egui_ctx.clone(), THUMB_SIDE, thumb_threads),
            thumbs: HashMap::new(),
            thumb_want: Vec::new(),
            thumb_asked: Vec::new(),
            thumbs_until: 0.0,
            thumbs_drag: false,
            thumbs_area: Rect::NOTHING,
            thumbs_view: None,
            half: 0,
            pending_open: Arc::default(),
            take_launches: Arc::new(std::sync::atomic::AtomicBool::new(single_instance)),
            max_side,
            unrotate: Vec::new(),
            inbox: Arc::default(),
        };
        crate::instance::listen(cc.egui_ctx.clone(), app.take_launches.clone(), app.pending_open.clone());
        app.offer_default = Self::default_offer_due(&app.settings);
        if let Some(p) = initial {
            app.open(&cc.egui_ctx, p);
        }
        // eframe shows the window after its first frame, which is an empty one while the
        // picture is still decoding (and a white one while a big texture is still uploading).
        // Kept off screen until the picture (or the start screen) is really on it; see
        // `reveal_when_ready`.
        system::cloak(cc, true);
        if app.settings.start_fullscreen {
            cc.egui_ctx.send_viewport_cmd(egui::ViewportCommand::Fullscreen(true));
        }
        // Without a file it shows the start screen, with the recent folders to pick from.
        app
    }

    pub fn current(&self) -> Option<&PathBuf> {
        self.files.get(self.index)
    }

    pub fn current_rotation(&self) -> u8 {
        self.current().and_then(|p| self.rotation.get(p)).copied().unwrap_or(0)
    }

    pub fn ready(&self, i: usize) -> Option<&Arc<Pic>> {
        match self.cache.get(self.files.get(i)?) {
            Some(Slot::Ready(p)) => Some(p),
            _ => None,
        }
    }

    pub fn toast(&mut self, msg: impl Into<String>) {
        self.toast = Some((msg.into(), Instant::now()));
    }

    /// A folder or archive resumes where it was left; a file opens its folder at that file.
    pub fn open(&mut self, ctx: &egui::Context, path: PathBuf) {
        let path = std::path::absolute(&path).unwrap_or(path);
        if path.extension().is_some_and(|e| e.eq_ignore_ascii_case(MARK_EXT)) {
            match std::fs::read_to_string(&path).ok().and_then(|t| read_mark(&t)) {
                Some(mark) => self.open_bookmark(ctx, mark),
                None => self.toast(format!("Can't read the bookmark {}", files::file_name(&path))),
            }
        } else if path.is_dir() || files::is_container(&path) {
            self.open_folder(ctx, path, Start::Resume);
        } else if let Some(dir) = path.parent() {
            self.open_folder(ctx, dir.to_path_buf(), Start::File(path));
        }
    }

    /// Returns false (and says why) if there was nothing to open.
    pub fn open_folder(&mut self, ctx: &egui::Context, dir: PathBuf, start: Start) -> bool {
        let mut files = match files::list_pages(&dir, self.settings.sort, self.settings.sort_desc) {
            Ok(f) => f,
            Err(e) => {
                self.toast(format!("Can't open {}: {e}", files::file_name(&dir)));
                return false;
            }
        };
        // A file with an unusual extension still opens if it was picked explicitly.
        if let Start::File(t) = &start
            && t.is_file()
            && !files.contains(t)
        {
            files.push(t.clone());
        }
        if files.is_empty() {
            // A series folder: chapters inside, no pages of its own. Carry on with the chapter
            // read last, or start at the first one.
            if dir.is_dir() {
                let last_read =
                    self.settings.history.iter().map(|(d, _)| d).find(|d| d.parent() == Some(&*dir)).cloned();
                let chapter = last_read
                    .or_else(|| files::child_containers(&dir).into_iter().take(50).find(|c| files::has_pages(c)));
                if let Some(chapter) = chapter {
                    return self.open_folder(ctx, chapter, Start::Resume);
                }
            }
            self.toast(format!("No images in {}", files::file_name(&dir)));
            return false;
        }
        let at_end = matches!(start, Start::Last);
        let by_name = |name: &str| files.iter().position(|f| files::relative_name(&dir, f) == name);
        let index = match start {
            Start::File(t) => files.iter().position(|f| *f == t).unwrap_or(0),
            Start::Resume if self.settings.resume => {
                self.settings.history.iter().find(|(d, _)| *d == dir).and_then(|(_, name)| by_name(name)).unwrap_or(0)
            }
            Start::Resume => 0,
            Start::Page(name) => by_name(&name).unwrap_or(0),
            Start::First => 0,
            Start::Last => files.len() - 1,
        };
        // Moving to the next chapter keeps strip mode; otherwise each folder remembers it.
        let was_strip = self.strip && self.dir.is_some();
        let hopping = self.dir.as_deref().and_then(Path::parent) == dir.parent() && self.dir.as_ref() != Some(&dir);
        self.strip = self.settings.strip_dirs.contains(&dir) || (hopping && was_strip);
        if self.strip && !self.settings.strip_dirs.contains(&dir) {
            self.settings.strip_dirs.push(dir.clone());
            prune(&mut self.settings.strip_dirs);
        }
        // Book layout follows the series; a new one starts the way the last one was read.
        let series = dir.parent();
        if let Some(&(_, spread, rtl)) = self.settings.series_layout.iter().find(|(d, ..)| Some(d.as_path()) == series)
        {
            self.settings.spread = spread;
            self.settings.rtl = rtl;
        }
        self.forward = index + 1 < files.len() || files.len() == 1;
        self.files = files;
        self.dir = Some(dir);
        self.index = index;
        self.error = None;
        self.hop = None;
        self.enter_at_bottom = at_end;
        self.strip_offset = 0.0;
        self.strip_pending = 0.0;
        self.strip_last_visible = index;
        // Old textures go, but `shown` stays on screen until the new folder's first image is ready.
        files::set_open(self.dir.as_deref().unwrap_or(Path::new("")));
        self.cache.clear();
        self.decoded.clear();
        self.remember_position();
        self.refresh(ctx);
        true
    }

    /// Opens a folder saved in history or a bookmark. An external drive that came back under
    /// another letter is found again, and everything saved on it follows.
    pub fn open_saved(&mut self, ctx: &egui::Context, dir: PathBuf, start: Start) {
        let dir = if dir.exists() {
            dir
        } else if let Some((old, new)) = files::find_moved_drive(&dir) {
            let s = &mut self.settings;
            for p in s.history.iter_mut().map(|(d, _)| d).chain(s.bookmarks.iter_mut().map(|b| &mut b.dir)) {
                if let Some(moved) = files::swap_drive(p, &old, &new) {
                    *p = moved;
                }
            }
            files::swap_drive(&dir, &old, &new).unwrap_or(dir)
        } else {
            self.toast("That folder isn't there (drive unplugged?)");
            return;
        };
        self.open_folder(ctx, dir, start);
    }

    /// Re-lists the folder (after changing the sort order) and stays on the same file.
    pub fn resort(&mut self, ctx: &egui::Context) {
        if let (Some(dir), Some(cur)) = (self.dir.clone(), self.current().cloned()) {
            let strip = self.strip;
            self.open_folder(ctx, dir, Start::File(cur));
            self.strip = strip;
        }
    }

    pub fn open_file_dialog(&mut self, ctx: &egui::Context) {
        let all: Vec<&str> = files::EXTENSIONS
            .iter()
            .chain(files::CONTAINERS)
            .chain(crate::video::EXTENSIONS)
            .copied()
            .chain([MARK_EXT])
            .collect();
        let mut dialog = rfd::FileDialog::new()
            .add_filter("Images, videos and comics", &all)
            .add_filter("Videos", crate::video::EXTENSIONS)
            .add_filter("Comic archives and PDF", files::CONTAINERS)
            .add_filter("Bookmark files", &[MARK_EXT])
            .add_filter("All files", &["*"]);
        if let Some(dir) = self.dialog_dir() {
            dialog = dialog.set_directory(dir);
        }
        if let Some(p) = dialog.pick_file() {
            self.open(ctx, p);
        }
    }

    pub fn open_folder_dialog(&mut self, ctx: &egui::Context) {
        let mut dialog = rfd::FileDialog::new();
        if let Some(dir) = self.dialog_dir() {
            dialog = dialog.set_directory(dir);
        }
        if let Some(p) = dialog.pick_folder() {
            self.open_folder(ctx, p, Start::Resume);
        }
    }

    /// The folder dialogs start in: the open one (an archive's folder), or the last one read.
    fn dialog_dir(&self) -> Option<PathBuf> {
        let d = self.dir.clone().or_else(|| self.settings.history.first().map(|(d, _)| d.clone()))?;
        if d.is_dir() { Some(d) } else { d.parent().map(Path::to_path_buf) }
    }

    pub fn remember_position(&mut self) {
        let (Some(dir), Some(cur)) = (self.dir.clone(), self.current()) else {
            return;
        };
        let name = files::relative_name(&dir, cur);
        let h = &mut self.settings.history;
        h.retain(|(d, _)| *d != dir);
        h.insert(0, (dir, name));
        h.truncate(HISTORY_MAX);
    }

    pub fn go_to(&mut self, ctx: &egui::Context, index: usize, forward: bool, at_bottom: bool) {
        if self.files.is_empty() || index >= self.files.len() {
            return;
        }
        if self.strip {
            self.strip_pending = 0.0;
            self.strip_offset = 0.0;
        }
        if index == self.index {
            return;
        }
        let now = Instant::now();
        self.fast = self.last_step.is_some_and(|t| now - t < FAST_STEP);
        self.last_step = Some(now);
        self.index = index;
        self.forward = forward;
        // Coming back to a split wide page lands on its second half.
        self.half = if forward { 0 } else { 1 };
        self.enter_at_bottom = at_bottom;
        self.hop = None;
        if self.slideshow.is_some() {
            self.slideshow = Some(now);
        }
        self.remember_position();
        self.refresh(ctx);
        self.log_step();
    }

    /// Moves `delta` pages. Doesn't wrap: going past the last page arms a hop to the next
    /// folder (the next chapter), and the same move again takes it.
    pub fn step(&mut self, ctx: &egui::Context, delta: isize) {
        self.step_entering(ctx, delta, false);
    }

    fn step_entering(&mut self, ctx: &egui::Context, delta: isize, at_bottom: bool) {
        let n = self.files.len() as isize;
        if n == 0 || delta == 0 {
            return;
        }
        // A wide page split in two: the other half first.
        if delta.abs() == 1 && self.split_now() && (delta > 0) == (self.half == 0) {
            self.half = 1 - self.half;
            self.zoom = None;
            self.pan = Vec2::new(0.0, f32::INFINITY);
            return;
        }
        let target = if self.spread_on() { self.spread_target(delta) } else { self.index as isize + delta };
        if target >= n && self.last_on_screen() as isize == n - 1 {
            self.end_of_folder(ctx, true);
        } else if target < 0 && self.index == 0 && (!self.strip || self.strip_offset <= 0.5) {
            self.end_of_folder(ctx, false);
        } else {
            self.go_to(ctx, target.clamp(0, n - 1) as usize, delta > 0, at_bottom);
        }
    }

    fn end_of_folder(&mut self, ctx: &egui::Context, forward: bool) {
        let edge = if forward { "Last image" } else { "First image" };
        match self.settings.at_end {
            AtEnd::Stop => {
                self.toast(edge);
                return;
            }
            AtEnd::Loop => {
                let n = self.files.len();
                let to = if forward { 0 } else { n.saturating_sub(1) };
                self.go_to(ctx, to, forward, false);
                self.toast(if forward { "Back to the first image" } else { "On to the last image" });
                return;
            }
            AtEnd::NextFolder => {
                // A held key or a long wheel spin would race through every folder: one per pause.
                if self.last_hop.is_some_and(|t| t.elapsed() < HOP_GUARD) {
                    self.last_hop = Some(Instant::now());
                    return;
                }
                self.last_hop = Some(Instant::now());
                match self.dir.as_deref().and_then(|d| files::sibling_folder(d, forward)) {
                    Some(folder) => {
                        let start = if forward { Start::First } else { Start::Last };
                        if self.open_folder(ctx, folder, start) {
                            let name = self.dir.as_deref().map(files::file_name).unwrap_or_default();
                            self.toast(format!("Opened {name}"));
                        }
                    }
                    None => self.toast(edge),
                }
                return;
            }
            AtEnd::NextFolderOnSecondPush => {}
        }
        if let Some((dir, target, armed)) = self.hop.clone()
            && dir == forward
        {
            if armed.elapsed() < HOP_GUARD {
                // Still pushing (a held key, a wheel spin): wait for a pause, then a new push.
                self.hop = Some((dir, target, Instant::now()));
                return;
            }
            match target {
                Some(folder) => {
                    let start = if forward { Start::First } else { Start::Last };
                    if self.open_folder(ctx, folder, start) {
                        self.toast(format!("Opened {}", self.dir.as_deref().map(files::file_name).unwrap_or_default()));
                    } else {
                        self.hop = None;
                    }
                }
                None => self.toast(edge),
            }
            return;
        }
        let target = self.dir.as_deref().and_then(|d| files::sibling_folder(d, forward));
        let msg = match &target {
            Some(d) => {
                format!("{edge}  ·  {} again opens {}", if forward { "next" } else { "back" }, files::file_name(d))
            }
            None => edge.to_owned(),
        };
        self.toast(msg);
        self.hop = Some((forward, target, Instant::now()));
    }

    /// Next / previous folder straight away, from the menu or `]` `[`.
    pub fn hop_folder(&mut self, ctx: &egui::Context, forward: bool) {
        match self.dir.as_deref().and_then(|d| files::sibling_folder(d, forward)) {
            Some(folder) => {
                if self.open_folder(ctx, folder, Start::First) {
                    self.toast(format!("Opened {}", self.dir.as_deref().map(files::file_name).unwrap_or_default()));
                }
            }
            None => self.toast(if forward { "No next folder" } else { "No previous folder" }),
        }
    }

    /// Scrolls by `dy` points (positive = down). A page taller than the window scrolls and
    /// turns at its ends, so reading a comic in fit-width is just "keep scrolling"; in strip
    /// mode it all flows as one.
    pub fn scroll_or_turn(&mut self, ctx: &egui::Context, dy: f32) {
        if self.strip {
            self.strip_pending += dy;
            ctx.request_repaint();
            return;
        }
        let s = self.slack.y;
        let at_edge = s <= 0.5 || if dy > 0.0 { self.pan.y <= -s + 0.5 } else { self.pan.y >= s - 0.5 };
        if at_edge {
            // Scrolling up past the top lands on the bottom of the previous page.
            self.step_entering(ctx, if dy > 0.0 { 1 } else { -1 }, dy < 0.0);
        } else {
            self.pan.y = (self.pan.y - dy).clamp(-s, s);
        }
    }

    // --- book mode ------------------------------------------------------------------------

    pub fn spread_on(&self) -> bool {
        self.settings.spread && !self.strip
    }

    /// The page shown next to page `i` in a spread. The cover (first page) and wide pages
    /// (double-page scans) stand alone. Pages not decoded yet count as normal pages.
    pub fn partner(&self, i: usize) -> Option<usize> {
        let j = i + 1;
        if !self.spread_on() || i == 0 || j >= self.files.len() {
            return None;
        }
        let wide = |k: usize| {
            self.ready(k).is_some_and(|p| {
                let s = p.size_turned(self.rotation.get(&self.files[k]).copied().unwrap_or(0));
                s.x > s.y * 1.05
            })
        };
        if wide(i) || wide(j) { None } else { Some(j) }
    }

    /// The last page on screen: the current one, or its partner in a spread.
    pub fn last_on_screen(&self) -> usize {
        self.partner(self.index).unwrap_or(self.index)
    }

    /// Where `delta` spreads away lands. Past either end it returns an index out of range.
    fn spread_target(&self, delta: isize) -> isize {
        let n = self.files.len() as isize;
        let mut i = self.index as isize;
        for _ in 0..delta.unsigned_abs() {
            if delta > 0 {
                if i >= n {
                    break;
                }
                i += 1 + self.partner(i as usize).is_some() as isize;
            } else {
                if i <= 0 {
                    return -1;
                }
                // Back one page, or two if that page is the second half of a spread.
                let prev = i - 1;
                i = if prev >= 1 && self.partner(prev as usize - 1) == Some(prev as usize) { prev - 1 } else { prev };
            }
        }
        i
    }

    pub fn toggle_spread(&mut self) {
        self.settings.spread = !self.settings.spread;
        if self.settings.spread && self.strip {
            self.strip = false;
            if let Some(dir) = &self.dir {
                self.settings.strip_dirs.retain(|d| d != dir);
            }
        }
        self.zoom = None;
        self.pan = Vec2::new(0.0, f32::INFINITY);
        self.remember_layout();
        self.toast(if self.settings.spread {
            "Two pages side by side  ·  , and . shift the pairs"
        } else {
            "One page at a time"
        });
    }

    pub fn toggle_rtl(&mut self) {
        self.settings.rtl = !self.settings.rtl;
        self.remember_layout();
        self.toast(if self.settings.rtl { "Right to left (manga)  ·  ← reads on" } else { "Left to right" });
    }

    fn remember_layout(&mut self) {
        let Some(series) = self.dir.as_deref().and_then(Path::parent).map(Path::to_path_buf) else {
            return;
        };
        let l = &mut self.settings.series_layout;
        l.retain(|(d, ..)| *d != series);
        l.insert(0, (series, self.settings.spread, self.settings.rtl));
        l.truncate(HISTORY_MAX);
    }

    // --- bookmarks ------------------------------------------------------------------------

    pub fn bookmark_index(&self, i: usize) -> Option<usize> {
        let (dir, page) = (self.dir.as_ref()?, self.files.get(i)?);
        let name = files::relative_name(dir, page);
        self.settings.bookmarks.iter().position(|b| b.dir == *dir && b.page == name)
    }

    pub fn toggle_bookmark(&mut self) {
        let (Some(dir), Some(page)) = (self.dir.clone(), self.current().cloned()) else {
            return;
        };
        match self.bookmark_index(self.index) {
            Some(i) => {
                self.settings.bookmarks.remove(i);
                self.toast("Bookmark removed");
            }
            // A file of its own, wherever the user wants it; cancelling leaves nothing behind.
            None => {
                if self.save_bookmark_file() {
                    let page = files::relative_name(&dir, &page);
                    self.settings.bookmarks.insert(0, Bookmark { dir, page, number: self.index + 1 });
                }
            }
        }
    }

    /// Makes TelescopeView the default for the picked types (on Windows: registers them and
    /// opens its Default apps page, where the user confirms each one).
    pub fn make_default(&mut self) {
        match crate::system::make_default(&self.settings.default_off) {
            Ok(msg) | Err(msg) => self.toast(msg),
        }
    }

    /// Whether to offer becoming the default this time: not more than once a week, and not
    /// when it already is.
    pub fn default_offer_due(settings: &Settings) -> bool {
        const WEEK: u64 = 7 * 24 * 3600;
        !cfg!(target_os = "macos")
            && settings.ask_default
            && now_secs().saturating_sub(settings.default_asked_at) >= WEEK
            && !crate::system::is_default(&settings.default_off)
    }

    pub fn open_bookmark(&mut self, ctx: &egui::Context, b: Bookmark) {
        self.open_saved(ctx, b.dir, Start::Page(b.page));
    }

    /// Saves the page on screen as a little file of its own, anywhere the user likes.
    /// Opening it later (double-click, Open, or drop) continues from that page.
    /// Returns whether a file was saved.
    pub fn save_bookmark_file(&mut self) -> bool {
        let (Some(dir), Some(page)) = (self.dir.clone(), self.current().cloned()) else {
            return false;
        };
        let number = self.index + 1;
        let name = format!("{} - page {number}.{MARK_EXT}", files::file_name(&dir));
        let mut dialog = rfd::FileDialog::new()
            .set_title("Save a bookmark file")
            .add_filter("Bookmark file", &[MARK_EXT])
            .set_file_name(name);
        // Where the last one went, else next to the series.
        let start =
            self.settings.mark_dir.clone().filter(|d| d.is_dir()).or_else(|| dir.parent().map(Path::to_path_buf));
        if let Some(start) = start {
            dialog = dialog.set_directory(start);
        }
        let Some(file) = dialog.save_file() else {
            return false;
        };
        self.settings.mark_dir = file.parent().map(Path::to_path_buf);
        let mark = Bookmark { page: files::relative_name(&dir, &page), dir, number };
        match std::fs::write(&file, mark_text(&mark)) {
            Ok(()) => {
                self.toast(format!("Saved {}  ·  open it to continue from here", files::file_name(&file)));
                true
            }
            Err(e) => {
                self.toast(format!("Couldn't save the bookmark: {e}"));
                false
            }
        }
    }

    /// Jumps to the page typed in the "go to page" box.
    pub fn finish_goto(&mut self, ctx: &egui::Context) {
        if let Some(n) = self.goto.take().and_then(|t| t.trim().parse::<usize>().ok())
            && n >= 1
            && !self.files.is_empty()
        {
            let i = n.min(self.files.len()) - 1;
            self.go_to(ctx, i, i > self.index, false);
        }
    }

    // --- strip mode -----------------------------------------------------------------------

    /// Strip column width for a page, in points.
    pub fn strip_page_width(&self, i: usize) -> f32 {
        let col = self.view_size.x * self.settings.strip_width.clamp(0.1, 1.0);
        match self.ready(i) {
            Some(pic) if !self.settings.enlarge_small => {
                let turns = self.rotation.get(&self.files[i]).copied().unwrap_or(0);
                col.min(pic.size_turned(turns).x / self.ppp)
            }
            _ => col,
        }
    }

    /// Height of page `i` in the strip, in points. Pages not decoded yet are guessed from the
    /// last page measured, so the layout below the view may still settle.
    pub fn strip_page_height(&self, i: usize) -> f32 {
        let w = self.strip_page_width(i);
        match self.cache.get(&self.files[i]) {
            Some(Slot::Ready(pic)) => {
                let turns = self.rotation.get(&self.files[i]).copied().unwrap_or(0);
                let s = pic.size_turned(turns);
                (w * s.y / s.x.max(1.0)).max(1.0)
            }
            Some(Slot::Failed(_)) => 160.0,
            None => (w * self.strip_aspect).max(1.0),
        }
    }

    /// Moves the strip by `dy` points right away. Returns false if it was already at that end;
    /// then `may_hop` arms the jump to the next chapter (dragging doesn't).
    pub fn strip_scroll_now(&mut self, ctx: &egui::Context, dy: f32, may_hop: bool) -> bool {
        let before = (self.index, self.strip_offset);
        self.strip_offset += dy;
        let n = self.files.len();
        let old_index = self.index;
        while self.index + 1 < n && self.strip_offset >= self.strip_page_height(self.index) {
            self.strip_offset -= self.strip_page_height(self.index);
            self.index += 1;
        }
        // Not past the end: the last page's bottom stays at the bottom of the view.
        let view_h = self.view_size.y;
        let mut below = -self.strip_offset;
        for i in self.index..n {
            below += self.strip_page_height(i);
            if below >= view_h {
                break;
            }
        }
        if below < view_h {
            self.strip_offset -= view_h - below;
        }
        while self.strip_offset < 0.0 && self.index > 0 {
            self.index -= 1;
            self.strip_offset += self.strip_page_height(self.index);
        }
        self.strip_offset = self.strip_offset.max(0.0);
        if self.index != old_index {
            self.forward = self.index > old_index || (self.index == old_index && dy > 0.0);
            self.hop = None;
            self.remember_position();
            self.refresh(ctx);
        }
        let moved = self.index != before.0 || (self.strip_offset - before.1).abs() > 0.01;
        if !moved && dy != 0.0 && may_hop {
            self.end_of_folder(ctx, dy > 0.0);
        }
        moved
    }

    pub fn set_strip(&mut self, on: bool) {
        self.strip = on;
        if on && self.settings.spread {
            self.settings.spread = false;
            self.remember_layout();
        }
        self.strip_offset = 0.0;
        self.strip_pending = 0.0;
        self.zoom = None;
        self.pan = Vec2::new(0.0, f32::INFINITY);
        if let Some(dir) = self.dir.clone() {
            self.settings.strip_dirs.retain(|d| *d != dir);
            if on {
                self.settings.strip_dirs.push(dir);
                prune(&mut self.settings.strip_dirs);
            }
        }
    }

    pub fn toggle_strip(&mut self) {
        self.set_strip(!self.strip);
        self.toast(if self.strip { "Strip: pages flow as one long scroll" } else { "Strip off" });
    }

    pub fn rotate(&mut self, clockwise: bool) {
        self.reorient(if clockwise { 1 } else { 3 }, "Rotated");
    }

    /// Mirrors the image left to right, or (`vertical`) upside down.
    pub fn flip(&mut self, vertical: bool) {
        self.reorient(if vertical { FLIP_V } else { FLIP_H }, "Flipped");
    }

    fn reorient(&mut self, by: u8, done: &str) {
        if let Some(cur) = self.current().cloned() {
            let r = self.rotation.entry(cur.clone()).or_insert(0);
            *r = compose(*r, by);
            let turned = *r != 0;
            self.zoom = None;
            self.pan = Vec2::new(0.0, f32::INFINITY);
            if turned && cur.is_file() {
                let save = self.keymap.text(Action::SaveTurn);
                self.toast(format!("{done} on screen  ·  {save} saves it to the file"));
            }
        }
    }

    pub fn dark_mode(&self, ctx: &egui::Context) -> bool {
        match self.settings.theme {
            theme::ThemePref::Dark => true,
            theme::ThemePref::Light => false,
            theme::ThemePref::System => ctx.system_theme() != Some(egui::Theme::Light),
        }
    }

    pub fn set_theme(&mut self, ctx: &egui::Context, pref: theme::ThemePref) {
        self.settings.theme = pref;
        self.apply_theme(ctx);
    }

    /// T: system, dark, light.
    fn cycle_theme(&mut self, ctx: &egui::Context) {
        use theme::ThemePref::*;
        let next = match self.settings.theme {
            System => Dark,
            Dark => Light,
            Light => System,
        };
        self.set_theme(ctx, next);
        self.toast(match next {
            System => "Theme: like the system",
            Dark => "Theme: dark",
            Light => "Theme: light",
        });
    }

    fn apply_theme(&mut self, ctx: &egui::Context) {
        let want = (self.dark_mode(ctx), self.settings.high_contrast);
        if self.applied_theme != Some(want) {
            theme::apply(ctx, want.0, want.1);
            self.applied_theme = Some(want);
        }
    }

    pub fn set_fit(&mut self, fit: Fit) {
        if self.strip {
            self.set_strip(false);
        }
        self.settings.fit = fit;
        self.zoom = None;
        self.pan = Vec2::new(0.0, f32::INFINITY);
    }

    /// Settings back to the defaults; what was read and bookmarked stays.
    /// F5: lists the folder again (new and removed files) and decodes the current image
    /// again, in case it was edited elsewhere.
    pub fn reload(&mut self, ctx: &egui::Context) {
        if let Some(cur) = self.current().cloned() {
            self.cache.remove(&cur);
            self.decoded.remove(&cur);
        }
        self.resort(ctx);
        self.toast("Reloaded");
    }

    /// Sharp pixels changed: textures are made again with the new filtering.
    pub fn reupload(&mut self) {
        self.cache.clear();
        self.decoded.clear();
    }

    /// Chapters of the series the open folder belongs to (the folders next to it).
    pub fn chapter_list(&mut self) -> Vec<PathBuf> {
        let Some(parent) = self.dir.as_deref().and_then(Path::parent).map(Path::to_path_buf) else {
            return Vec::new();
        };
        if self.chapters.as_ref().is_none_or(|(p, _)| *p != parent) {
            let list: Vec<PathBuf> = files::child_containers(&parent).into_iter().take(1000).collect();
            self.chapters = Some((parent, list));
        }
        self.chapters.as_ref().map(|(_, l)| l.clone()).unwrap_or_default()
    }

    pub fn reset_settings(&mut self, ctx: &egui::Context) {
        let old = std::mem::take(&mut self.settings);
        self.settings.history = old.history;
        self.settings.strip_dirs = old.strip_dirs;
        self.settings.series_layout = old.series_layout;
        self.settings.bookmarks = old.bookmarks;
        ctx.set_zoom_factor(1.0);
        self.zoom = None;
        self.hop = None;
        self.keymap = Keymap::new(&self.settings.keys);
        self.toast("Settings reset");
    }

    pub fn toggle_lock_zoom(&mut self) {
        self.settings.lock_zoom = !self.settings.lock_zoom;
        self.toast(if self.settings.lock_zoom {
            "Zoom locked: the next images keep this zoom"
        } else {
            "Zoom unlocked: each image fits again"
        });
    }

    pub fn toggle_enlarge(&mut self) {
        self.settings.enlarge_small = !self.settings.enlarge_small;
        self.zoom = None;
        self.toast(if self.settings.enlarge_small { "Enlarge small images: on" } else { "Enlarge small images: off" });
    }

    pub fn toggle_slideshow(&mut self) {
        self.slideshow = match self.slideshow {
            Some(_) => {
                self.toast("Slideshow stopped");
                None
            }
            None => {
                let what = if self.strip { "Auto-scroll" } else { "Slideshow" };
                self.toast(format!("{what}  ·  {} s", self.settings.slideshow_secs));
                Some(Instant::now())
            }
        };
        keep_display_awake(self.slideshow.is_some());
    }

    pub fn set_slideshow_secs(&mut self, secs: u32) {
        self.settings.slideshow_secs = secs;
        if self.slideshow.is_some() {
            self.toast(format!("Slideshow  ·  {secs} s"));
        }
    }

    pub fn toggle_fullscreen(&self, ctx: &egui::Context) {
        let fs = is_fullscreen(ctx);
        ctx.send_viewport_cmd(egui::ViewportCommand::Fullscreen(!fs));
    }

    fn tick_slideshow(&mut self, ctx: &egui::Context) {
        let Some(since) = self.slideshow else { return };
        let secs = self.settings.slideshow_secs.max(1) as f32;
        if self.strip {
            // Auto-scroll: one window height per interval, smoothly.
            let now = Instant::now();
            let dt = self.strip_tick.map_or(1.0 / 60.0, |t| (now - t).as_secs_f32()).min(0.1);
            self.strip_tick = Some(now);
            if !self.strip_scroll_now(ctx, self.view_size.y / secs * dt, true) {
                self.slideshow = None;
                keep_display_awake(false);
            }
            ctx.request_repaint();
            return;
        }
        let interval = Duration::from_secs_f32(secs);
        // Only counts while the current image is actually on screen, so a slow drive never
        // makes the slideshow skip pages.
        let on_screen = self.shown.as_ref().map(|s| &s.path) == self.current();
        if on_screen && since.elapsed() >= interval {
            // In book mode a step is a whole pair.
            let next = if self.spread_on() { self.spread_target(1) as usize } else { self.index + 1 };
            if next < self.files.len() {
                self.go_to(ctx, next, true, false);
            } else if self.settings.slideshow_stops {
                self.slideshow = None;
                keep_display_awake(false);
                self.toast("Slideshow finished");
                return;
            } else {
                self.go_to(ctx, 0, true, false);
            }
            self.slideshow = Some(Instant::now());
        }
        let left = interval.saturating_sub(self.slideshow.map_or(Duration::ZERO, |t| t.elapsed()));
        ctx.request_repaint_after(left.max(Duration::from_millis(16)));
    }

    /// Indices to keep decoded, highest priority first, current image included.
    pub fn window(&self) -> Vec<usize> {
        let n = self.files.len();
        if n == 0 {
            return Vec::new();
        }
        if self.strip {
            // Everything on screen, then onward, then a little above.
            let last = self.strip_last_visible.clamp(self.index, n - 1).min(self.index + 40);
            let mut out: Vec<usize> = (self.index..=(last + STRIP_AHEAD).min(n - 1)).collect();
            out.extend((self.index.saturating_sub(STRIP_BEHIND)..self.index).rev());
            return out;
        }
        let dir = if self.forward { 1 } else { -1 };
        let mut out = vec![self.index];
        // A spread shows two pages and moves two at a time.
        let spread = self.spread_on() as isize;
        if spread == 1 && self.index + 1 < n {
            out.push(self.index + 1);
        }
        for off in preload_offsets(self.settings.preload, self.fast) {
            for extra in 0..=spread {
                let i = self.index as isize + off * dir * (1 + spread) + extra;
                // The slideshow wraps around, so it wants the first pages ready at the end.
                let i = if self.slideshow.is_some() { i.rem_euclid(n as isize) } else { i };
                if (0..n as isize).contains(&i) && !out.contains(&(i as usize)) {
                    out.push(i as usize);
                }
            }
        }
        out
    }

    /// Takes finished decodes, drops what fell out of the window and tells the loader what's
    /// still missing. Runs whenever the position or the cache changes.
    pub fn refresh(&mut self, ctx: &egui::Context) {
        let wanted: Vec<PathBuf> = self.window().iter().map(|&i| self.files[i].clone()).collect();

        while let Some(d) = self.loader.try_recv() {
            if !wanted.contains(&d.path) || self.cache.contains_key(&d.path) {
                continue;
            }
            match d.result {
                Ok(img) => {
                    self.decoded.insert(d.path, img);
                }
                Err(e) => {
                    self.cache.insert(d.path, Slot::Failed(e));
                }
            }
        }
        self.cache.retain(|p, _| wanted.contains(p));
        self.decoded.retain(|p, _| wanted.contains(p));

        // GPU upload, at most one image per frame and nearest first. A full-size photo is
        // ~100 MB of pixels; several landing in the same frame would stall the UI exactly while
        // the user is flipping. Uploading ahead of time means landing on an image costs nothing.
        let frame = ctx.cumulative_frame_nr();
        if self.uploaded_in_frame != Some(frame)
            && let Some(path) = wanted.iter().find(|p| self.decoded.contains_key(*p)).cloned()
        {
            let img = self.decoded.remove(&path).expect("checked above");
            let pic = upload(ctx, &path, img, self.settings.sharp_pixels);
            self.cache.insert(path.clone(), Slot::Ready(Arc::new(pic)));
            if let Some(k) = self.unrotate.iter().position(|p| *p == path) {
                self.unrotate.remove(k);
                self.rotation.remove(&path);
            }
            self.uploaded_in_frame = Some(frame);
        }
        if !self.decoded.is_empty() {
            ctx.request_repaint();
        }

        self.loader
            .request(wanted.into_iter().filter(|p| !self.cache.contains_key(p) && !self.decoded.contains_key(p)));

        // The current image if it's ready. If the user is flipping faster than we can decode,
        // the closest ready one behind it instead, so a held key keeps the picture moving
        // rather than freezing on the image they started from.
        let n = self.files.len() as isize;
        let back = if self.forward { -1 } else { 1 };
        let pick = (0..=2)
            .map(|k| self.index as isize + back * k)
            .filter(|i| (0..n).contains(i))
            .find_map(|i| self.ready(i as usize).map(|pic| (self.files[i as usize].clone(), pic.clone())));
        // A new page, or the same page decoded again (after saving a rotation).
        if let Some((path, pic)) = pick
            && self.shown.as_ref().is_none_or(|s| s.path != path || !Arc::ptr_eq(&s.pic, &pic))
        {
            if self.current() == Some(&path)
                && let Some(t) = self.stats.waiting.take()
            {
                eprintln!("  ready after {:.0} ms", t.elapsed().as_secs_f64() * 1000.0);
            }
            self.shown = Some(Shown { path, pic, since: Instant::now() });
            self.anim_pause = None;
            // Locked: same zoom and the same spot, for comparing shots. Otherwise fit again.
            if !(self.settings.lock_zoom && self.zoom.is_some()) {
                self.zoom = None;
                // Taller than the window: start at the top (or the bottom when scrolling back up).
                self.pan = Vec2::new(0.0, if self.enter_at_bottom { f32::NEG_INFINITY } else { f32::INFINITY });
            }
        }

        let title = match (self.current(), &self.dir) {
            (Some(p), Some(d)) if files::is_container(d) => {
                format!("{} - {} - {APP_NAME}", files::file_name(p), files::file_name(d))
            }
            (Some(p), _) => format!("{} - {APP_NAME}", files::file_name(p)),
            _ => APP_NAME.to_owned(),
        };
        if title != self.title {
            ctx.send_viewport_cmd(egui::ViewportCommand::Title(title.clone()));
            self.title = title;
        }
    }

    fn log_step(&mut self) {
        if !self.stats.enabled {
            return;
        }
        // A file that failed to decode shows its error immediately, which counts as instant too.
        let hit = self.shown.as_ref().map(|s| &s.path) == self.current()
            || matches!(self.current().and_then(|p| self.cache.get(p)), Some(Slot::Failed(_)));
        self.stats.waiting = (!hit).then(Instant::now);
        if hit {
            self.stats.hits += 1;
        } else {
            self.stats.misses += 1;
        }
        eprintln!(
            "-> {} {} (instant {}/{})",
            files::file_name(&self.files[self.index]),
            if hit { "instant" } else { "NOT READY" },
            self.stats.hits,
            self.stats.hits + self.stats.misses
        );
    }

    fn handle_keys(&mut self, ctx: &egui::Context) {
        // A text box is open: its own keys only (Ctrl+C copies text there, not the image).
        if self.renaming.is_some() || self.goto.is_some() {
            return;
        }
        if let Some(path) = self.confirm_delete.clone() {
            let (yes, no) = ctx.input(|i| (i.key_pressed(Key::Enter), i.key_pressed(Key::Escape)));
            if yes {
                self.confirm_delete = None;
                self.delete_now(ctx, path);
            } else if no {
                self.confirm_delete = None;
            }
            return;
        }
        if self.capturing.is_some() {
            self.capture_key(ctx);
            return;
        }
        // A focused button (Tab) would take arrows and Space instead of the page. The only text
        // boxes are rename and go-to, handled above.
        if let Some(id) = ctx.memory(|m| m.focused()) {
            ctx.memory_mut(|m| m.surrender_focus(id));
        }
        let n = ctx.input(|i| self.keymap.presses(i));
        let count = |a: Action| n[a as usize] as isize;
        let hit = |a: Action| n[a as usize] > 0;

        if hit(Action::Open) {
            self.open_file_dialog(ctx);
        }
        if hit(Action::OpenFolder) {
            self.open_folder_dialog(ctx);
        }
        if hit(Action::Settings) {
            self.show_settings = !self.show_settings;
        }
        if hit(Action::Close) {
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
        }
        if hit(Action::CopyImage) {
            self.copy_image(ctx);
        }
        if hit(Action::CopyPath) {
            self.copy_path(ctx);
        }
        if hit(Action::CopyFile) {
            self.copy_file();
        }
        if hit(Action::Paste) {
            self.paste(ctx);
        }
        if hit(Action::Thumbs) {
            self.toggle_thumbs(ctx);
        }
        if hit(Action::SplitWide) {
            self.settings.split_wide = !self.settings.split_wide;
            self.half = 0;
            self.toast(if self.settings.split_wide { "Wide pages: a half at a time" } else { "Wide pages: whole" });
        }
        if hit(Action::ShowInFolder) {
            self.show_in_explorer();
        }
        if hit(Action::SaveTurn) {
            self.save_rotation(ctx);
        }

        let rtl = self.settings.rtl && !self.strip;
        let pan_sideways = self.settings.arrows_pan && !self.strip && self.slack.x > 0.5;
        // The fixed keys: arrows, Space and the mouse's side buttons. Not with Ctrl or Alt held.
        let (arrows, down, space, mouse) = ctx.input(|i| {
            if i.modifiers.command || i.modifiers.alt {
                return (0, 0, 0, 0);
            }
            let n = |k: Key| i.num_presses(k) as isize;
            let space = if i.modifiers.shift { -n(Key::Space) } else { n(Key::Space) };
            let mouse = i.pointer.button_pressed(PointerButton::Extra2) as isize
                - i.pointer.button_pressed(PointerButton::Extra1) as isize;
            (n(Key::ArrowRight) - n(Key::ArrowLeft), n(Key::ArrowDown) - n(Key::ArrowUp), space, mouse)
        });
        let space = if self.video.is_some() && space != 0 {
            self.toggle_pause();
            0
        } else {
            space
        };
        // Zoomed in wider than the window: the arrows look around first (see below).
        let arrows_turn = !(pan_sideways && arrows != 0);
        // Right to left: the arrows follow the page, so ← reads on.
        let sideways = count(Action::Right) - count(Action::Left) + if arrows_turn { arrows } else { 0 };
        let sideways = if rtl { -sideways } else { sideways };
        let pages = sideways + count(Action::Next) - count(Action::Previous) + mouse;
        let pan_x = if arrows_turn { 0 } else { arrows };
        if pages != 0 {
            if self.strip && pages < 0 && self.strip_offset > 0.5 {
                // Back in strip mode first goes to the top of the page being read.
                self.strip_offset = 0.0;
                self.strip_pending = 0.0;
                self.step(ctx, pages + 1);
            } else {
                self.step(ctx, pages);
            }
        }
        for _ in 0..pan_x.unsigned_abs() {
            let s = self.slack.x;
            let right = pan_x > 0;
            if if right { self.pan.x <= -s + 0.5 } else { self.pan.x >= s - 0.5 } {
                // At the edge: the next press turns the page (mirrored right to left).
                let forward = right != rtl;
                self.step(ctx, if forward { 1 } else { -1 });
                break;
            }
            let dx = self.view_size.x * 0.15;
            self.pan.x = (self.pan.x + if right { -dx } else { dx }).clamp(-s, s);
        }
        let view_h = self.view_size.y;
        for _ in 0..down.unsigned_abs() {
            self.scroll_or_turn(ctx, down.signum() as f32 * view_h * 0.15);
        }
        for _ in 0..space.unsigned_abs() {
            self.scroll_or_turn(ctx, space.signum() as f32 * view_h * 0.85);
        }

        if hit(Action::First) {
            self.go_to(ctx, 0, true, false);
        }
        if hit(Action::Last) && !self.files.is_empty() {
            self.go_to(ctx, self.files.len() - 1, false, false);
        }
        if hit(Action::Reload) {
            self.reload(ctx);
        }
        if hit(Action::NextFolder) {
            self.hop_folder(ctx, true);
        }
        if hit(Action::PreviousFolder) {
            self.hop_folder(ctx, false);
        }
        if hit(Action::FitPage) {
            self.set_fit(Fit::Page);
        }
        if hit(Action::FitWidth) {
            self.set_fit(Fit::Width);
        }
        if hit(Action::Strip) {
            self.toggle_strip();
        }
        if hit(Action::Delete) {
            self.delete_current(ctx);
        }
        if hit(Action::Bookmark) {
            self.toggle_bookmark();
        }
        if hit(Action::SaveBookmark) {
            self.save_bookmark_file();
        }
        if hit(Action::Spread) {
            self.toggle_spread();
        }
        if hit(Action::Rtl) {
            self.toggle_rtl();
        }
        if hit(Action::GoTo) && !self.files.is_empty() {
            self.goto = Some(String::new());
        }
        // , and . shift a spread by one page (books whose pairs start on the other side), or
        // step an animation frame by frame.
        let steps = count(Action::StepOn) - count(Action::StepBack);
        if steps != 0 && !self.files.is_empty() {
            if self.spread_on() {
                let i = (self.index as isize + steps).clamp(0, self.files.len() as isize - 1) as usize;
                self.go_to(ctx, i, steps > 0, false);
            } else {
                self.step_frame(steps);
            }
        }
        if hit(Action::Pause) {
            self.toggle_pause();
        }
        if hit(Action::Rename) {
            self.start_rename();
        }
        if hit(Action::Theme) {
            self.cycle_theme(ctx);
        }
        if hit(Action::Enlarge) {
            self.toggle_enlarge();
        }
        if hit(Action::LockZoom) {
            self.toggle_lock_zoom();
        }
        if hit(Action::RotateRight) {
            self.rotate(true);
        }
        if hit(Action::RotateLeft) {
            self.rotate(false);
        }
        if hit(Action::FlipH) {
            self.flip(false);
        }
        if hit(Action::FlipV) {
            self.flip(true);
        }
        if hit(Action::Slideshow) {
            self.toggle_slideshow();
        }
        if hit(Action::Info) {
            self.settings.show_info = !self.settings.show_info;
        }
        if hit(Action::Help) {
            self.show_help = !self.show_help;
        }
        if hit(Action::Fullscreen) {
            self.toggle_fullscreen(ctx);
        }
        if ctx.input(|i| i.key_pressed(Key::Escape)) {
            // Peels one layer at a time: help and settings, then fullscreen, then the slideshow.
            if self.show_help || self.show_settings {
                self.show_help = false;
                self.show_settings = false;
            } else if is_fullscreen(ctx) {
                ctx.send_viewport_cmd(egui::ViewportCommand::Fullscreen(false));
            } else if self.slideshow.is_some() {
                self.toggle_slideshow();
            } else if self.settings.esc_closes {
                ctx.send_viewport_cmd(egui::ViewportCommand::Close);
            }
        }
    }

    /// Settings is waiting for a key to bind: the next press goes there (Esc cancels).
    fn capture_key(&mut self, ctx: &egui::Context) {
        let Some((action, add)) = self.capturing else { return };
        let (esc, combo) = ctx.input(|i| (i.key_pressed(Key::Escape), i.events.iter().find_map(keys::Combo::of_event)));
        if esc {
            self.capturing = None;
            return;
        }
        let Some(combo) = combo else { return };
        self.capturing = None;
        // A key does one thing: taken from whatever had it.
        if let Some(other) = self.keymap.owner(combo)
            && other != action
        {
            let rest: Vec<_> = self.keymap.get(other).iter().copied().filter(|c| *c != combo).collect();
            keys::set(&mut self.settings.keys, other, &rest);
            self.toast(format!("{} was on \"{}\": moved", combo.text(), keys::def_of(other).label));
        }
        let mut list: Vec<_> = if add { self.keymap.get(action).to_vec() } else { Vec::new() };
        if !list.contains(&combo) {
            list.push(combo);
        }
        keys::set(&mut self.settings.keys, action, &list);
        self.keymap = Keymap::new(&self.settings.keys);
    }

    /// The current page is a wide one (a double-page scan) shown one half at a time.
    pub fn split_now(&self) -> bool {
        self.settings.split_wide
            && !self.strip
            && !self.spread_on()
            && self.ready(self.index).is_some_and(|p| {
                let s = p.size_turned(self.current_rotation());
                s.x > s.y * 1.05
            })
    }

    /// Takes finished thumbnails and asks for the ones drawn last frame that are missing.
    pub fn refresh_thumbs(&mut self, ctx: &egui::Context) {
        while let Some(d) = self.thumb_loader.try_recv() {
            let tex = d.result.ok().map(|mut img| {
                let pixels = img.frames.swap_remove(0).tiles.swap_remove(0).pixels;
                ctx.load_texture(format!("thumb {}", d.path.display()), pixels, TextureOptions::LINEAR)
            });
            self.thumbs.insert(d.path, tex);
        }
        // A long session in many folders: start over rather than grow forever.
        if self.thumbs.len() > 5000 {
            self.thumbs.clear();
        }
        let want: Vec<PathBuf> =
            std::mem::take(&mut self.thumb_want).into_iter().filter(|p| !self.thumbs.contains_key(p)).collect();
        if want != self.thumb_asked {
            self.thumb_loader.request(want.clone());
            self.thumb_asked = want;
        }
    }

    /// New decoder threads after the thread settings changed.
    pub fn restart_loaders(&mut self, ctx: &egui::Context) {
        self.loader = Loader::new(ctx.clone(), self.max_side, self.settings.decode_threads);
        self.thumb_loader = Loader::thumbnails(ctx.clone(), THUMB_SIDE, self.settings.thumb_threads);
        self.thumb_asked.clear();
        // What was being decoded went with the old threads: the new ones get asked again.
        self.refresh(ctx);
    }

    /// The current animation's frame, its frame count, and whether it is paused.
    pub fn anim_state(&self) -> Option<(usize, usize, bool)> {
        let shown = self.shown.as_ref()?;
        let n = shown.pic.frames.len();
        if n < 2 || self.current() != Some(&shown.path) {
            return None;
        }
        let t = self.anim_pause.unwrap_or_else(|| shown.since.elapsed());
        Some((shown.pic.frame_index(t), n, self.anim_pause.is_some()))
    }

    pub fn toggle_pause(&mut self) {
        if let Some(v) = &mut self.video {
            v.toggle_pause();
            return;
        }
        if self.anim_state().is_none() {
            return;
        }
        let Some(shown) = &mut self.shown else { return };
        match self.anim_pause.take() {
            // Carries on from the frame it stopped at.
            Some(t) => shown.since = Instant::now() - t,
            None => {
                let total = shown.pic.total_time().as_nanos().max(1);
                let t = Duration::from_nanos((shown.since.elapsed().as_nanos() % total) as u64);
                self.anim_pause = Some(t);
            }
        }
    }

    /// Pauses an animation and moves `by` frames.
    pub fn step_frame(&mut self, by: isize) {
        // A video jumps 5 seconds instead.
        if let Some(v) = &mut self.video {
            let to = v.position() + 5.0 * by as f64;
            v.seek(to);
            return;
        }
        let Some((frame, n, _)) = self.anim_state() else { return };
        let Some(shown) = &self.shown else { return };
        let k = (frame as isize + by).rem_euclid(n as isize) as usize;
        self.anim_pause = Some(shown.pic.frame_start(k));
    }

    // --- file actions ---------------------------------------------------------------------

    pub fn copy_image(&mut self, ctx: &egui::Context) {
        let Some(path) = self.current().cloned() else {
            return;
        };
        // Textures live on the GPU only, so decode the file again at full size for the clipboard.
        // On another thread: a big photo or a slow drive would freeze the window meanwhile.
        self.in_background(ctx, move || {
            let result = crate::loader::decode(&path, u32::MAX).and_then(|img| {
                let tile = &img.frames[0].tiles[0].pixels;
                let [w, h] = tile.size;
                let bytes: Vec<u8> = tile.pixels.iter().flat_map(|c| c.to_srgba_unmultiplied()).collect();
                with_clipboard(|cb| {
                    cb.set_image(arboard::ImageData { width: w, height: h, bytes: bytes.into() })
                        .map_err(|e| e.to_string())
                })
            });
            match result {
                Ok(()) => "Image copied".to_owned(),
                Err(e) => format!("Couldn't copy: {e}"),
            }
        });
    }

    /// Puts the file itself on the clipboard, to paste into a folder or a chat.
    pub fn copy_file(&mut self) {
        let Some(path) = self.current_file() else {
            self.toast("Pages inside an archive can't be copied as files");
            return;
        };
        match with_clipboard(|cb| cb.set().file_list(&[&path]).map_err(|e| e.to_string())) {
            Ok(()) => self.toast("File copied"),
            Err(e) => self.toast(format!("Couldn't copy: {e}")),
        }
    }

    /// Opens what's on the clipboard: copied files, a path, or a picture (saved to a file first).
    pub fn paste(&mut self, ctx: &egui::Context) {
        let files = with_clipboard(|cb| cb.get().file_list().map_err(|e| e.to_string())).unwrap_or_default();
        if let Some(p) = files.into_iter().find(|p| p.exists()) {
            self.open(ctx, p);
            return;
        }
        let text = with_clipboard(|cb| cb.get_text().map_err(|e| e.to_string())).unwrap_or_default();
        let text = PathBuf::from(text.trim().trim_matches('"'));
        if !text.as_os_str().is_empty() && text.exists() {
            self.open(ctx, text);
            return;
        }
        let image = with_clipboard(|cb| {
            cb.get_image().map(|i| (i.width, i.height, i.bytes.into_owned())).map_err(|e| e.to_string())
        });
        let Ok((w, h, bytes)) = image else {
            self.toast("Nothing to paste: copy a picture or a file first");
            return;
        };
        // Written as a PNG in a folder of its own, then opened like any file.
        let pending = self.pending_open.clone();
        let repaint = ctx.clone();
        self.in_background(ctx, move || {
            let dir = std::env::temp_dir().join("TelescopeView pasted");
            let stamp = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_millis());
            let path = dir.join(format!("pasted {stamp}.png"));
            let saved = std::fs::create_dir_all(&dir).map_err(|e| e.to_string()).and_then(|()| {
                let img = image::RgbaImage::from_raw(w as u32, h as u32, bytes).ok_or("odd picture size")?;
                img.save(&path).map_err(|e| e.to_string())
            });
            match saved {
                Ok(()) => {
                    *pending.lock().unwrap_or_else(|e| e.into_inner()) = Some(path);
                    repaint.request_repaint();
                    "Pasted  ·  kept in the TelescopeView pasted folder".to_owned()
                }
                Err(e) => format!("Couldn't paste: {e}"),
            }
        });
    }

    /// Runs slow work off the UI thread; the message it returns shows as a toast.
    pub fn in_background(&self, ctx: &egui::Context, job: impl FnOnce() -> String + Send + 'static) {
        let (inbox, ctx) = (self.inbox.clone(), ctx.clone());
        std::thread::spawn(move || {
            let msg = job();
            inbox.lock().unwrap_or_else(|e| e.into_inner()).push(msg);
            ctx.request_repaint();
        });
    }

    pub fn copy_path(&mut self, ctx: &egui::Context) {
        if let Some(p) = self.current() {
            ctx.copy_text(p.display().to_string());
            self.toast("Path copied");
        }
    }

    pub fn show_in_explorer(&mut self) {
        let Some(p) = self.current() else { return };
        // A page inside an archive: point at the archive.
        let target = files::archive_of(p).map_or_else(|| p.clone(), |(a, _)| a.to_path_buf());
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            // explorer wants `/select,"path"` as one raw argument; normal quoting breaks it.
            let _ =
                std::process::Command::new("explorer.exe").raw_arg(format!("/select,\"{}\"", target.display())).spawn();
        }
        #[cfg(target_os = "macos")]
        let _ = std::process::Command::new("open").arg("-R").arg(&target).spawn();
        #[cfg(all(unix, not(target_os = "macos")))]
        if let Some(folder) = target.parent() {
            let _ = std::process::Command::new("xdg-open").arg(folder).spawn();
        }
    }

    pub fn bars_visible(&self, ctx: &egui::Context) -> bool {
        !is_fullscreen(ctx)
            || self.settings.keep_bars
            || self.last_activity.elapsed().as_secs_f32() < self.settings.bars_hide_secs
            || egui::Popup::is_any_open(ctx)
            || ctx.input(|i| i.pointer.hover_pos()).is_some_and(|p| {
                let r = ctx.content_rect();
                p.y < r.top() + 48.0 || p.y > r.bottom() - 28.0
            })
    }
}

/// Puts every tile of every frame on the GPU.
fn upload(ctx: &egui::Context, path: &Path, img: Image, sharp: bool) -> Pic {
    let mut opts = TextureOptions::LINEAR.with_mipmap_mode(Some(TextureFilter::Linear));
    if sharp {
        // Mipmaps keep shrinking smooth; only enlarging shows hard pixels.
        opts.magnification = TextureFilter::Nearest;
    }
    let name = path.to_string_lossy();
    let frames = img
        .frames
        .into_iter()
        .enumerate()
        .map(|(f, frame)| PicFrame {
            tiles: frame
                .tiles
                .into_iter()
                .enumerate()
                .map(|(t, tile)| (ctx.load_texture(format!("{name}#{f}.{t}"), tile.pixels, opts), tile.rect))
                .collect(),
            delay: frame.delay,
        })
        .collect();
    Pic { frames, original_size: img.original_size, format: img.format, has_alpha: img.has_alpha }
}

impl eframe::App for App {
    fn save(&mut self, storage: &mut dyn eframe::Storage) {
        eframe::set_value(storage, SETTINGS_KEY, &self.settings);
    }

    fn on_exit(&mut self, _gl: Option<&eframe::glow::Context>) {
        keep_display_awake(false);
    }

    fn ui(&mut self, ui: &mut egui::Ui, frame: &mut eframe::Frame) {
        let ctx = &ui.ctx().clone();
        if let Reveal::After(f) = self.reveal
            && ctx.cumulative_frame_nr() >= f
        {
            system::cloak(frame, false);
            self.reveal = Reveal::Done;
        }

        let dropped: Option<PathBuf> = ctx
            .input(|i| i.raw.dropped_files.iter().map(|f| f.path().to_path_buf()).find(|p| !p.as_os_str().is_empty()));
        if let Some(p) = dropped {
            self.open(ctx, p);
        }
        if ctx.input(|i| i.pointer.delta() != Vec2::ZERO || i.pointer.any_pressed()) {
            self.last_activity = Instant::now();
        }
        self.apply_theme(ctx);
        let done: Vec<String> = std::mem::take(&mut *self.inbox.lock().unwrap_or_else(|e| e.into_inner()));
        if let Some(msg) = done.into_iter().last() {
            self.toast(msg);
        }
        let pending = self.pending_open.lock().unwrap_or_else(|e| e.into_inner()).take();
        if let Some(p) = pending {
            self.open(ctx, p);
            ctx.send_viewport_cmd(egui::ViewportCommand::Minimized(false));
            ctx.send_viewport_cmd(egui::ViewportCommand::Focus);
        }
        self.take_launches.store(self.settings.single_instance, std::sync::atomic::Ordering::Relaxed);
        self.handle_keys(ctx);
        self.refresh_thumbs(ctx);
        self.sync_video(ctx);
        // Back to the small window once the user stops flipping, freeing the far-ahead images.
        if self.fast {
            let idle = self.last_step.map_or(Duration::MAX, |t| t.elapsed());
            if idle > FAST_STEP * 4 {
                self.fast = false;
            } else {
                ctx.request_repaint_after(FAST_STEP * 4 - idle);
            }
        }
        self.tick_slideshow(ctx);
        self.refresh(ctx);
        // The root ui comes with a margin; the bars and the image go edge to edge.
        let full = ctx.content_rect();
        let mut root = ui.new_child(egui::UiBuilder::new().max_rect(full));
        root.set_clip_rect(full);
        self.draw(&mut root, ctx);
        self.reveal_when_ready(ctx);
    }
}

impl App {
    /// The window comes on screen the frame after the first picture (or the start screen, or
    /// an error) was drawn, or after a second at most, for a slow drive.
    fn reveal_when_ready(&mut self, ctx: &egui::Context) {
        if self.reveal != Reveal::Waiting {
            return;
        }
        let failed = matches!(self.current().and_then(|p| self.cache.get(p)), Some(Slot::Failed(_)));
        let waited = self.started.elapsed() > Duration::from_secs(1);
        // Frame 0 only measures the start screen and draws nothing, so content starts at 1.
        let drawn = ctx.cumulative_frame_nr() >= 1;
        if drawn && (self.files.is_empty() || self.shown.is_some() || failed || waited) {
            // A few frames' grace: a frame handed to the GPU isn't on screen the moment it's sent.
            self.reveal = Reveal::After(ctx.cumulative_frame_nr() + 3);
        }
        ctx.request_repaint();
    }
}

/// Startup: the window stays off screen until it has something to show.
#[derive(Clone, Copy, PartialEq)]
enum Reveal {
    Waiting,
    /// Content was drawn; show the window from this frame on.
    After(u64),
    Done,
}

/// 16×16, two grey shades in 8 px squares, repeated across a see-through image.
fn checker_texture(ctx: &egui::Context) -> egui::TextureHandle {
    let px = (0..16 * 16)
        .map(|i| {
            if ((i % 16) / 8 + (i / 16) / 8) % 2 == 0 {
                egui::Color32::from_gray(200)
            } else {
                egui::Color32::from_gray(150)
            }
        })
        .collect();
    let img = egui::ColorImage { size: [16, 16], source_size: Vec2::splat(16.0), pixels: px };
    let opts = TextureOptions { wrap_mode: egui::TextureWrapMode::Repeat, ..TextureOptions::NEAREST };
    ctx.load_texture("checker", img, opts)
}

/// The app's one clipboard handle. Kept for the app's lifetime: on Linux what was copied lives
/// only as long as the handle that copied it.
pub fn now_secs() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

/// The extension of bookmark files.
pub const MARK_EXT: &str = "tvmark";
const MARK_HEADER: &str = "TelescopeView bookmark";

/// A bookmark file's text: a header line, then `key=value` lines.
fn mark_text(b: &Bookmark) -> String {
    format!("{MARK_HEADER}\nfolder={}\npage={}\nnumber={}\n", b.dir.display(), b.page, b.number)
}

fn read_mark(text: &str) -> Option<Bookmark> {
    let mut lines = text.lines();
    if lines.next()?.trim() != MARK_HEADER {
        return None;
    }
    let (mut dir, mut page, mut number) = (None, None, 1);
    for line in lines {
        match line.split_once('=') {
            Some(("folder", v)) => dir = Some(PathBuf::from(v.trim())),
            Some(("page", v)) => page = Some(v.trim().to_owned()),
            Some(("number", v)) => number = v.trim().parse().unwrap_or(1),
            _ => {}
        }
    }
    Some(Bookmark { dir: dir?, page: page?, number })
}

fn with_clipboard<R>(f: impl FnOnce(&mut arboard::Clipboard) -> Result<R, String>) -> Result<R, String> {
    static CLIPBOARD: std::sync::Mutex<Option<arboard::Clipboard>> = std::sync::Mutex::new(None);
    let mut slot = CLIPBOARD.lock().unwrap_or_else(|e| e.into_inner());
    if slot.is_none() {
        *slot = Some(arboard::Clipboard::new().map_err(|e| e.to_string())?);
    }
    f(slot.as_mut().expect("just set"))
}

/// Which pages to decode around the current one, nearest first: `ahead` onward and half as
/// many back. Flipping fast looks twice as far ahead and only one back.
fn preload_offsets(ahead: usize, fast: bool) -> Vec<isize> {
    let ahead = ahead.clamp(1, 32);
    let (ahead, behind) = if fast { (ahead * 2, 1) } else { (ahead, (ahead / 2).max(1)) };
    let mut out = Vec::new();
    let mut back = 0;
    for k in 1..=ahead {
        out.push(k as isize);
        if k % 2 == 0 && back < behind {
            back += 1;
            out.push(-(back as isize));
        }
    }
    while back < behind {
        back += 1;
        out.push(-(back as isize));
    }
    out
}

/// Keeps a remembered-folders list from growing forever: the oldest go first.
fn prune(list: &mut Vec<PathBuf>) {
    if list.len() > 1000 {
        list.drain(..list.len() - 1000);
    }
}

pub fn is_fullscreen(ctx: &egui::Context) -> bool {
    ctx.input(|i| i.viewport().fullscreen.unwrap_or(false))
}

/// Stops the screen from going to sleep while a slideshow runs.
fn keep_display_awake(on: bool) {
    #[cfg(windows)]
    {
        const ES_CONTINUOUS: u32 = 0x8000_0000;
        const ES_DISPLAY_REQUIRED: u32 = 0x0000_0002;
        unsafe extern "system" {
            fn SetThreadExecutionState(flags: u32) -> u32;
        }
        let flags = if on { ES_CONTINUOUS | ES_DISPLAY_REQUIRED } else { ES_CONTINUOUS };
        // SAFETY: a flags-only kernel32 call with no pointers involved.
        unsafe { SetThreadExecutionState(flags) };
    }
    #[cfg(not(windows))]
    let _ = on;
}

fn gpu_max_texture_side(cc: &eframe::CreationContext<'_>) -> u32 {
    if let Some(rs) = &cc.wgpu_render_state {
        return rs.device.limits().max_texture_dimension_2d;
    }
    if let Some(gl) = &cc.gl {
        use eframe::glow::HasContext;
        // SAFETY: plain state query on the context eframe just created on this thread.
        let side = unsafe { gl.get_parameter_i32(eframe::glow::MAX_TEXTURE_SIZE) };
        if side > 0 {
            return side as u32;
        }
    }
    8192
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mark_file_round_trip() {
        let b = Bookmark { dir: PathBuf::from(r"D:\Comics\Series = one\Ch 3"), page: "sub/012.jpg".into(), number: 12 };
        let back = read_mark(&mark_text(&b)).expect("reads back");
        assert_eq!((back.dir, back.page, back.number), (b.dir, b.page, b.number));
        assert!(read_mark("something else\nfolder=x\npage=y").is_none());
    }

    #[test]
    fn preload_order() {
        assert_eq!(preload_offsets(4, false), [1, 2, -1, 3, 4, -2], "the default, nearest first");
        assert_eq!(preload_offsets(1, false), [1, -1]);
        assert_eq!(preload_offsets(4, true).iter().filter(|&&o| o < 0).count(), 1, "flipping: one back");
        assert_eq!(preload_offsets(16, false).len(), 24);
    }

    #[test]
    fn animation_frame_timing() {
        let ctx = egui::Context::default();
        let tex = ctx.load_texture("t", egui::ColorImage::filled([1, 1], egui::Color32::RED), Default::default());
        let frame = |ms| PicFrame { tiles: vec![(tex.clone(), Rect::ZERO)], delay: Duration::from_millis(ms) };
        let pic =
            Pic { frames: vec![frame(100), frame(50)], original_size: [1, 1], format: String::new(), has_alpha: false };
        let at = |ms| {
            let (f, wait) = pic.frame_at(Duration::from_millis(ms));
            (f.delay.as_millis(), wait.unwrap().as_millis())
        };
        assert_eq!(at(0), (100, 100));
        assert_eq!(at(120), (50, 30));
        assert_eq!(at(150), (100, 100), "loops");
    }
}
