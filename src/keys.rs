//! Keyboard shortcuts: one table of actions with their default keys, which the user can change
//! in Settings. Arrows, Space, Esc and the mouse stay fixed.

use eframe::egui::{self, Key};
use std::collections::BTreeMap;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Action {
    Next,
    Previous,
    Right,
    Left,
    First,
    Last,
    NextFolder,
    PreviousFolder,
    GoTo,
    FitPage,
    FitWidth,
    ZoomFit,
    ActualSize,
    ZoomIn,
    ZoomOut,
    LockZoom,
    Enlarge,
    RotateRight,
    RotateLeft,
    FlipH,
    FlipV,
    Strip,
    Spread,
    Rtl,
    StepBack,
    StepOn,
    Pause,
    Slideshow,
    Info,
    Fullscreen,
    Theme,
    Help,
    Settings,
    Open,
    OpenFolder,
    CopyImage,
    CopyPath,
    CopyFile,
    Paste,
    Thumbs,
    SplitWide,
    ShowInFolder,
    SaveTurn,
    Edit,
    Rename,
    Delete,
    Bookmark,
    SaveBookmark,
    Reload,
    SelectText,
    CopyText,
    Close,
}

pub const COUNT: usize = Action::Close as usize + 1;

pub struct Def {
    pub action: Action,
    /// Saved in the settings file; never rename one.
    pub id: &'static str,
    pub label: &'static str,
    pub keys: &'static [&'static str],
    /// Held down, it keeps going (page turns, zoom). Everything else fires once per press.
    pub repeats: bool,
}

const fn def(action: Action, id: &'static str, label: &'static str, keys: &'static [&'static str]) -> Def {
    Def { action, id, label, keys, repeats: false }
}

const fn rep(action: Action, id: &'static str, label: &'static str, keys: &'static [&'static str]) -> Def {
    Def { action, id, label, keys, repeats: true }
}

const MAC: bool = cfg!(target_os = "macos");

use Action as A;
pub const DEFS: [Def; COUNT] = [
    rep(A::Next, "next", "Next image", &["PageDown"]),
    rep(A::Previous, "previous", "Previous image", if MAC { &["PageUp"] } else { &["PageUp", "Backspace"] }),
    rep(A::Right, "right", "Page to the right", &["D"]),
    rep(A::Left, "left", "Page to the left", &["A"]),
    def(A::First, "first", "First image", &["Home"]),
    def(A::Last, "last", "Last image", &["End"]),
    def(A::NextFolder, "next_folder", "Next folder", &["CloseBracket"]),
    def(A::PreviousFolder, "previous_folder", "Previous folder", &["OpenBracket"]),
    def(A::GoTo, "goto", "Go to page", &["G"]),
    def(A::FitPage, "fit", "Fit to window", &["F"]),
    def(A::FitWidth, "fit_width", "Fit width", &["W"]),
    def(A::ZoomFit, "zoom_fit", "Zoom to fit", &["0"]),
    def(A::ActualSize, "actual_size", "Actual size (100%)", &["1"]),
    rep(A::ZoomIn, "zoom_in", "Zoom in", &["Plus", "Equals"]),
    rep(A::ZoomOut, "zoom_out", "Zoom out", &["Minus"]),
    def(A::LockZoom, "lock_zoom", "Lock the zoom for the next images", &["L"]),
    def(A::Enlarge, "enlarge", "Enlarge small images", &["E"]),
    def(A::RotateRight, "rotate_right", "Rotate right", &["R"]),
    def(A::RotateLeft, "rotate_left", "Rotate left", &["Shift+R"]),
    def(A::FlipH, "flip_h", "Flip left to right", &["X"]),
    def(A::FlipV, "flip_v", "Flip upside down", &["Y"]),
    def(A::Strip, "strip", "Strip: one long scroll (webtoons)", &["V"]),
    def(A::Spread, "spread", "Two pages side by side", &["P"]),
    def(A::Rtl, "rtl", "Right to left (manga)", &["M"]),
    rep(A::StepBack, "step_back", "Shift pairs back / animation frame back", &["Comma"]),
    rep(A::StepOn, "step_on", "Shift pairs on / animation frame on", &["Period"]),
    def(A::Pause, "pause", "Pause / play an animation", &["K"]),
    def(A::Slideshow, "slideshow", "Slideshow", &["S"]),
    def(A::Info, "info", "Info", &["I"]),
    def(A::Fullscreen, "fullscreen", "Fullscreen", &["F11", "Enter"]),
    def(A::Theme, "theme", "Theme: system / dark / light", &["T"]),
    def(A::Help, "help", "Keyboard shortcuts", &["H", "F1", "Questionmark"]),
    def(A::Settings, "settings", "Settings", &["Ctrl+Comma"]),
    def(A::Open, "open", "Open file", &["Ctrl+O"]),
    def(A::OpenFolder, "open_folder", "Open folder", &["Ctrl+Shift+O"]),
    def(A::CopyImage, "copy_image", "Copy image", &["Ctrl+C"]),
    def(A::CopyPath, "copy_path", "Copy path", &["Ctrl+Shift+C"]),
    def(A::CopyFile, "copy_file", "Copy the file (to paste in a folder)", &["Ctrl+Shift+F"]),
    def(A::Paste, "paste", "Paste: open a copied picture or file", &["Ctrl+V"]),
    def(A::Thumbs, "thumbs", "Thumbnails", &["N"]),
    def(A::SplitWide, "split_wide", "Split wide pages in two", &["J"]),
    def(A::ShowInFolder, "show_in_folder", "Show in folder", &["Ctrl+E"]),
    def(A::SaveTurn, "save_turn", "Save rotation / flip to the file", &["Ctrl+S"]),
    def(A::Edit, "edit", "Edit the picture", &["Shift+E"]),
    def(A::Rename, "rename", "Rename", &["F2"]),
    // macOS has no Delete key on most keyboards; Cmd+Backspace is the Finder's "move to Trash".
    def(A::Delete, "delete", "Delete", if MAC { &["Delete", "Ctrl+Backspace"] } else { &["Delete"] }),
    def(A::Bookmark, "bookmark", "Bookmark the page", &["B"]),
    def(A::SaveBookmark, "save_bookmark", "Save a bookmark file (open it to continue)", &["Ctrl+B"]),
    def(A::Reload, "reload", "Reload the folder", &["F5"]),
    def(A::SelectText, "select_text", "Select text in the picture", &["Shift+T"]),
    def(A::CopyText, "copy_text", "Copy the selected text", &["Ctrl+T"]),
    def(A::Close, "close", "Close the window", &["Ctrl+W", "Ctrl+Q"]),
];

/// A key plus Ctrl (Cmd on macOS) and Shift.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Combo {
    pub ctrl: bool,
    pub shift: bool,
    pub key: Key,
}

/// Keys typed with Shift on most layouts (+ ? and friends): Shift is part of the key there.
fn shift_free(key: Key) -> bool {
    use Key::*;
    matches!(
        key,
        Colon
            | Comma
            | Minus
            | Period
            | Plus
            | Equals
            | Semicolon
            | Backslash
            | Slash
            | Pipe
            | Questionmark
            | Exclamationmark
            | OpenBracket
            | CloseBracket
            | OpenCurlyBracket
            | CloseCurlyBracket
            | Backtick
            | Quote
    )
}

impl Combo {
    pub fn parse(s: &str) -> Option<Combo> {
        let mut c = Combo { ctrl: false, shift: false, key: Key::Escape };
        let mut key = None;
        for part in s.split('+') {
            match part {
                "Ctrl" | "Cmd" => c.ctrl = true,
                "Shift" => c.shift = true,
                _ => key = Key::from_name(part),
            }
        }
        c.key = key?;
        Some(c)
    }

    /// As saved in the settings file.
    pub fn id(&self) -> String {
        let mut s = String::new();
        if self.ctrl {
            s += "Ctrl+";
        }
        if self.shift {
            s += "Shift+";
        }
        s + self.key.name()
    }

    /// As shown to people.
    pub fn text(&self) -> String {
        let key = match self.key {
            Key::ArrowRight => "→",
            Key::ArrowLeft => "←",
            Key::ArrowUp => "↑",
            Key::ArrowDown => "↓",
            Key::PageDown => "PgDn",
            Key::PageUp => "PgUp",
            Key::Delete => "Del",
            Key::Escape => "Esc",
            Key::Minus => "−",
            k => k.symbol_or_name(),
        };
        let ctrl = if MAC { "Cmd+" } else { "Ctrl+" };
        format!("{}{}{key}", if self.ctrl { ctrl } else { "" }, if self.shift { "Shift+" } else { "" })
    }

    fn matches(&self, key: Key, m: egui::Modifiers) -> bool {
        key == self.key && m.command == self.ctrl && !m.alt && (shift_free(key) || m.shift == self.shift)
    }

    /// The combo of a key press, if it can be a shortcut (Esc is kept for closing things).
    pub fn of_event(event: &egui::Event) -> Option<Combo> {
        match press(event) {
            Some((key, _, modifiers)) if key != Key::Escape => {
                Some(Combo { ctrl: modifiers.command, shift: modifiers.shift && !shift_free(key), key })
            }
            _ => None,
        }
    }
}

/// A key press as shortcuts see it: the key, whether it's a repeat, and the modifiers held.
/// egui turns Ctrl+C, Ctrl+X and Ctrl+V into clipboard events and drops the press (Ctrl+V
/// entirely when the clipboard holds a picture, not text). Their release still comes through,
/// so for those three the release counts as the press.
fn press(e: &egui::Event) -> Option<(Key, bool, egui::Modifiers)> {
    match e {
        egui::Event::Key { key, pressed: true, repeat, modifiers, .. } => Some((*key, *repeat, *modifiers)),
        egui::Event::Key { key: key @ (Key::C | Key::X | Key::V), pressed: false, modifiers, .. } if modifiers.command => {
            Some((*key, false, *modifiers))
        }
        _ => None,
    }
}

/// The keys in use: the defaults with the user's changes on top.
#[derive(Clone)]
pub struct Keymap {
    binds: Vec<Vec<Combo>>,
}

impl Keymap {
    /// `custom` maps an action id to its keys, for the actions the user changed.
    pub fn new(custom: &BTreeMap<String, Vec<String>>) -> Keymap {
        let binds = DEFS
            .iter()
            .map(|d| match custom.get(d.id) {
                Some(keys) => keys.iter().filter_map(|k| Combo::parse(k)).collect(),
                None => d.keys.iter().filter_map(|k| Combo::parse(k)).collect(),
            })
            .collect();
        Keymap { binds }
    }

    pub fn get(&self, a: Action) -> &[Combo] {
        &self.binds[a as usize]
    }

    /// "F11  Enter", or empty when nothing is bound.
    pub fn text(&self, a: Action) -> String {
        self.get(a).iter().map(Combo::text).collect::<Vec<_>>().join("  ")
    }

    /// "Label  (F)" for tooltips.
    pub fn tip(&self, label: &str, a: Action) -> String {
        let keys = self.text(a);
        if keys.is_empty() { label.to_owned() } else { format!("{label}  ({keys})") }
    }

    /// How many times each action was pressed this frame.
    pub fn presses(&self, i: &egui::InputState) -> [usize; COUNT] {
        let mut n = [0; COUNT];
        for e in &i.events {
            if let Some((key, repeat, modifiers)) = press(e) {
                for (k, d) in DEFS.iter().enumerate() {
                    if (d.repeats || !repeat) && self.binds[k].iter().any(|c| c.matches(key, modifiers)) {
                        n[k] += 1;
                    }
                }
            }
        }
        n
    }

    /// The action already using `combo`, if any.
    pub fn owner(&self, combo: Combo) -> Option<Action> {
        DEFS.iter().find(|d| self.get(d.action).contains(&combo)).map(|d| d.action)
    }
}

pub fn def_of(a: Action) -> &'static Def {
    &DEFS[a as usize]
}

/// Sets `a`'s keys in `custom`, dropping the entry when they're back to the defaults.
pub fn set(custom: &mut BTreeMap<String, Vec<String>>, a: Action, keys: &[Combo]) {
    let d = def_of(a);
    let ids: Vec<String> = keys.iter().map(Combo::id).collect();
    let defaults: Vec<String> = d.keys.iter().filter_map(|k| Combo::parse(k)).map(|c| c.id()).collect();
    if ids == defaults {
        custom.remove(d.id);
    } else {
        custom.insert(d.id.to_owned(), ids);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn table_is_in_order_and_parses() {
        for (i, d) in DEFS.iter().enumerate() {
            assert_eq!(d.action as usize, i, "{} out of order", d.id);
            for k in d.keys {
                let c = Combo::parse(k).unwrap_or_else(|| panic!("bad key {k}"));
                assert_eq!(Combo::parse(&c.id()), Some(c));
            }
        }
        let ids: std::collections::HashSet<_> = DEFS.iter().map(|d| d.id).collect();
        assert_eq!(ids.len(), COUNT, "ids must be unique");
    }

    #[test]
    fn no_default_key_is_used_twice() {
        let map = Keymap::new(&BTreeMap::new());
        let mut seen = std::collections::HashMap::new();
        for d in &DEFS {
            for c in map.get(d.action) {
                if let Some(other) = seen.insert(c.id(), d.id) {
                    panic!("{} is on both {other} and {}", c.id(), d.id);
                }
            }
        }
    }

    #[test]
    fn custom_keys_replace_defaults() {
        let mut custom = BTreeMap::new();
        let x = Combo::parse("Ctrl+X").unwrap();
        set(&mut custom, Action::FitPage, &[x]);
        assert_eq!(Keymap::new(&custom).get(Action::FitPage), &[x]);
        set(&mut custom, Action::FitPage, &[Combo::parse("F").unwrap()]);
        assert!(custom.is_empty(), "back to the default drops the entry");
    }

    #[test]
    fn shift_counts_for_letters_not_symbols() {
        let r = Combo::parse("R").unwrap();
        let shift = egui::Modifiers { shift: true, ..Default::default() };
        assert!(r.matches(Key::R, egui::Modifiers::NONE));
        assert!(!r.matches(Key::R, shift));
        assert!(Combo::parse("Plus").unwrap().matches(Key::Plus, shift));
    }

    #[test]
    fn ctrl_c_and_ctrl_v_count_on_release() {
        let map = Keymap::new(&BTreeMap::new());
        let ctrl = egui::Modifiers::COMMAND;
        let key = |key, pressed, modifiers| egui::Event::Key { key, physical_key: None, pressed, repeat: false, modifiers };
        let presses = |events: Vec<egui::Event>| {
            let input = egui::RawInput { events, ..Default::default() };
            let state = egui::InputState::default().begin_pass(input, false, 1.0, egui::Options::default().input_options);
            map.presses(&state)
        };
        // What egui hands over for Ctrl+C: the copy event, then only the release.
        let n = presses(vec![egui::Event::Copy, key(Key::C, false, ctrl)]);
        assert_eq!(n[Action::CopyImage as usize], 1);
        // Ctrl+V with a picture on the clipboard: just the release.
        assert_eq!(presses(vec![key(Key::V, false, ctrl)])[Action::Paste as usize], 1);
        // Other releases are not presses, and Ctrl+T still comes as a press.
        assert_eq!(presses(vec![key(Key::C, false, egui::Modifiers::NONE)]), [0; COUNT]);
        assert_eq!(presses(vec![key(Key::T, true, ctrl), key(Key::T, false, ctrl)])[Action::CopyText as usize], 1);
    }
}
