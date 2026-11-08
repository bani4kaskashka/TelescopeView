//! Select text (Shift+T, the menus, or holding the left button on a picture that can't move):
//! the pages on screen are read for words, a drag across them picks letters and Ctrl+T copies
//! them. A click picks one word, a double-click its line, Ctrl+A the whole page. Esc or the
//! same key again stops it.

use crate::app::{App, Drawn, compose};
use crate::canvas::turn;
use crate::ocr::{self, Caret, Word};
use eframe::egui::{self, Color32, PointerButton, Pos2, Rect};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

/// A press this close to a word (in word heights) is on it.
const ON_A_WORD: f32 = 0.6;
/// Holding the left button this long (seconds) on a picture that can't move starts selecting.
const HOLD: f64 = 0.35;

#[derive(Default)]
pub struct TextSelect {
    /// Each page on screen, with the video frame it was read from (0 for files).
    pages: HashMap<PathBuf, (usize, Page)>,
    /// The page picked on, and the spots between letters the pick runs from and to.
    picked: Option<(PathBuf, Caret, Caret)>,
    /// Started by holding the button: a click that picks nothing stops it again.
    held: bool,
}

enum Page {
    Reading(ocr::Job),
    Read(Vec<Word>),
    Failed,
}

fn picked_color() -> Color32 {
    Color32::from_rgba_unmultiplied(50, 115, 235, 110)
}

/// What's there to pick, faintly.
fn hint_color() -> Color32 {
    Color32::from_rgba_unmultiplied(50, 115, 235, 40)
}

impl TextSelect {
    fn words(&self, path: &PathBuf) -> Option<&[Word]> {
        words_in(&self.pages, path)
    }
}

/// Apart from `TextSelect::words` so a closure can borrow just the pages.
fn words_in<'a>(pages: &'a HashMap<PathBuf, (usize, Page)>, path: &PathBuf) -> Option<&'a [Word]> {
    match pages.get(path) {
        Some((_, Page::Read(w))) => Some(w),
        _ => None,
    }
}

impl App {
    pub fn toggle_text_select(&mut self) {
        if self.text.take().is_some() || self.files.is_empty() || self.edit.is_some() {
            return;
        }
        // A frame that keeps changing can't be read.
        if self.video.as_ref().is_some_and(|v| !v.paused) {
            self.toggle_pause();
        }
        self.text = Some(TextSelect::default());
        self.toast("Select text: drag over the words, Esc to stop");
    }

    /// Holding the left button still on a page that has nothing to pan starts selecting, so a
    /// drag goes on to pick words. Not on a playing video. Before `text_layer`.
    pub fn hold_to_select(&mut self, ctx: &egui::Context, resp: &egui::Response) {
        // A page that fits can be a few millionths over from f32 rounding: not something to pan.
        if self.text.is_some() || self.edit.is_some() || self.slack.max_elem() > 0.5 || !resp.is_pointer_button_down_on() {
            return;
        }
        if self.video.as_ref().is_some_and(|v| !v.paused) {
            return;
        }
        let (down, origin, since) = ctx.input(|i| {
            let held = i.pointer.press_start_time().map(|t| i.time - t);
            (i.pointer.primary_down() && !i.pointer.middle_down(), i.pointer.press_origin(), held)
        });
        let on_page = origin.is_some_and(|p| self.drawn.iter().any(|d| d.clip.contains(p)));
        let (true, true, Some(since)) = (down, on_page, since) else { return };
        if since >= HOLD {
            self.text = Some(TextSelect { held: true, ..Default::default() });
        } else {
            ctx.request_repaint_after(Duration::from_secs_f64(HOLD - since));
        }
    }

    /// The words picked, as text.
    pub fn picked_text(&self) -> Option<String> {
        let t = self.text.as_ref()?;
        let (path, a, b) = t.picked.as_ref()?;
        Some(ocr::span_text(t.words(path)?, *a, *b)).filter(|s| !s.is_empty())
    }

    /// Copies the picked words; false when none are picked.
    pub fn copy_picked_text(&mut self, ctx: &egui::Context) -> bool {
        let Some(text) = self.picked_text() else { return false };
        ctx.copy_text(text);
        self.toast("Text copied");
        true
    }

    /// Ctrl+A: every word on the page under the mouse, or on the first page read.
    pub fn pick_all_text(&mut self, ctx: &egui::Context) {
        let pointer = ctx.input(|i| i.pointer.hover_pos());
        let Some(t) = &mut self.text else { return };
        let under = pointer.and_then(|p| self.drawn.iter().find(|d| d.clip.contains(p)));
        let page = under.into_iter().chain(&self.drawn).find_map(|d| Some((d.path.clone(), t.words(&d.path)?)));
        if let Some((path, words)) = page.filter(|(_, w)| !w.is_empty()) {
            let (first, _) = ocr::whole(words, 0);
            let (_, last) = ocr::whole(words, words.len() - 1);
            t.picked = Some((path, first, last));
        }
    }

    /// Esc: drops what's picked first, then stops selecting.
    pub fn text_escape(&mut self) {
        if let Some(t) = &mut self.text
            && t.picked.take().is_none()
        {
            self.text = None;
        }
    }

    /// Reads the pages just drawn, takes the mouse and paints the words. After the pages.
    pub fn text_layer(&mut self, ctx: &egui::Context, resp: &egui::Response, painter: &egui::Painter) {
        let Some(mut t) = self.text.take() else { return };
        let mut news = None;

        // Pages that left the screen are forgotten; new ones (and a newly paused frame) are read.
        t.pages.retain(|p, _| self.drawn.iter().any(|d| d.path == *p));
        for d in &self.drawn {
            let frame = self.video.as_ref().filter(|v| v.path == d.path && v.paused).and_then(|v| v.frame.clone());
            if crate::video::is_video(&d.path) && frame.is_none() {
                t.pages.remove(&d.path);
                continue;
            }
            let id = frame.as_ref().map_or(0, |f| Arc::as_ptr(f) as usize);
            if t.pages.get(&d.path).is_some_and(|(read, _)| *read == id) {
                continue;
            }
            let source = match frame {
                Some(f) => ocr::Source::Pixels(pixels(&f)),
                None => ocr::Source::File(d.path.clone()),
            };
            t.pages.insert(d.path.clone(), (id, Page::Reading(ocr::Job::start(source))));
            if t.picked.as_ref().is_some_and(|(p, ..)| *p == d.path) {
                t.picked = None;
            }
        }
        let mut reading = false;
        for (_, page) in t.pages.values_mut() {
            if let Page::Reading(job) = page {
                match job.finished() {
                    None => reading = true,
                    Some(Ok(words)) => {
                        if words.is_empty() {
                            news = Some("No text found".to_owned());
                        }
                        *page = Page::Read(words);
                    }
                    Some(Err(e)) => {
                        news = Some(format!("Can't read the text: {e}"));
                        *page = Page::Failed;
                    }
                }
            }
        }
        if reading {
            ctx.request_repaint_after(Duration::from_millis(50));
            for d in &self.drawn {
                if matches!(t.pages.get(&d.path), Some((_, Page::Reading(_)))) {
                    let visible = d.clip.intersect(painter.clip_rect());
                    crate::canvas::draw_loading(painter, visible.center_bottom() - egui::vec2(0.0, 30.0), ctx.input(|i| i.time));
                }
            }
        }

        // The mouse: press and drag across letters, click a word, double-click a line.
        let pages = &t.pages;
        let at = |p: Pos2| self.drawn.iter().find(|d| d.clip.contains(p));
        let near = |d: &Drawn, p: Pos2| words_in(pages, &d.path).and_then(|w| ocr::nearest(w, on_picture(d, p), d.pic.original_size));
        // The spot between letters nearest to a point.
        let spot = |d: &Drawn, p: Pos2| Some(ocr::caret(words_in(pages, &d.path)?, near(d, p)?.0, on_picture(d, p)));
        // The whole word under a click, if it's on one.
        let word_at = |p: Pos2| {
            let d = at(p)?;
            let (i, _) = near(d, p).filter(|(_, dist)| *dist <= ON_A_WORD)?;
            let (a, b) = ocr::whole(words_in(pages, &d.path)?, i);
            Some((d.path.clone(), a, b))
        };
        if resp.drag_started_by(PointerButton::Primary)
            && let Some(p) = ctx.input(|i| i.pointer.press_origin())
            && let Some(d) = at(p)
        {
            t.picked = spot(d, p).map(|c| (d.path.clone(), c, c));
        } else if resp.dragged_by(PointerButton::Primary)
            && let Some(p) = resp.interact_pointer_pos()
        {
            // A drag that began before the words were read (or before a hold started this)
            // starts from where the button went down.
            if t.picked.is_none()
                && let Some(o) = ctx.input(|i| i.pointer.press_origin())
                && let Some(d) = at(o)
            {
                t.picked = spot(d, o).map(|c| (d.path.clone(), c, c));
            }
            if let Some((path, _, end)) = &mut t.picked
                && let Some(d) = self.drawn.iter().find(|d| d.path == *path)
                && let Some(c) = spot(d, p)
            {
                *end = c;
            }
        } else if resp.double_clicked_by(PointerButton::Primary)
            && let Some(p) = resp.interact_pointer_pos()
            && let Some(d) = at(p)
            && let Some((i, dist)) = near(d, p)
            && dist <= ON_A_WORD
            && let Some(words) = words_in(pages, &d.path)
        {
            let line = words[i].line;
            let (first, _) = ocr::whole(words, words.iter().position(|w| w.line == line).unwrap_or(i));
            let (_, last) = ocr::whole(words, words.iter().rposition(|w| w.line == line).unwrap_or(i));
            t.picked = Some((d.path.clone(), first, last));
        } else if resp.clicked_by(PointerButton::Primary)
            && let Some(p) = resp.interact_pointer_pos()
        {
            t.picked = word_at(p);
            // Started by a hold: a click off the words ends it, like it was never on.
            if t.held && t.picked.is_none() {
                self.text = None;
                return;
            }
        }
        // A button held still past egui's click time (0.8 s) is let go as a drag, not a click,
        // with nothing between its two spots: taken as the click it looks like.
        if resp.drag_stopped_by(PointerButton::Primary)
            && let Some(p) = resp.interact_pointer_pos()
            && t.picked.as_ref().is_none_or(|(_, a, b)| a == b)
        {
            t.picked = word_at(p);
            if t.held && t.picked.is_none() {
                self.text = None;
                return;
            }
        }
        if let Some(p) = resp.hover_pos()
            && let Some(d) = at(p)
            && near(d, p).is_some_and(|(_, dist)| dist <= ON_A_WORD)
        {
            ctx.set_cursor_icon(egui::CursorIcon::Text);
        }

        // The words: faint where they are, stronger over the letters picked.
        for d in &self.drawn {
            let Some(words) = t.words(&d.path) else { continue };
            let clipped = painter.with_clip_rect(d.clip.intersect(painter.clip_rect()));
            for w in words {
                clipped.rect_filled(on_screen(d, w.rect), 0.0, hint_color());
            }
            if let Some((_, a, b)) = t.picked.as_ref().filter(|(p, ..)| *p == d.path) {
                for (i, letters) in ocr::span(words, *a, *b) {
                    clipped.rect_filled(on_screen(d, ocr::letters_rect(&words[i], &letters)), 0.0, picked_color());
                }
            }
        }

        self.text = Some(t);
        if let Some(msg) = news {
            self.toast(msg);
        }
    }
}

/// A point on screen to 0..1 of the picture as stored, undoing its turn.
fn on_picture(d: &Drawn, p: Pos2) -> Pos2 {
    let undo = (0..8).find(|&b| compose(d.turns, b) == 0).unwrap_or(0);
    turn(Pos2::ZERO + (p - d.rect.min) / d.rect.size(), undo)
}

/// A box in 0..1 of the picture to where it is on screen, turned like the page.
fn on_screen(d: &Drawn, r: Rect) -> Rect {
    let place = |p: Pos2| d.rect.min + turn(p, d.turns).to_vec2() * d.rect.size();
    Rect::from_two_pos(place(r.min), place(r.max))
}

fn pixels(frame: &egui::ColorImage) -> image::RgbaImage {
    let rgba = frame.pixels.iter().flat_map(|c| c.to_srgba_unmultiplied()).collect();
    image::RgbaImage::from_raw(frame.size[0] as u32, frame.size[1] as u32, rgba).unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::Pic;

    fn drawn(turns: u8, rect: Rect) -> Drawn {
        let pic = Pic { frames: Vec::new(), original_size: [200, 100], format: String::new(), has_alpha: false, shade: Vec::new() };
        Drawn { path: PathBuf::new(), pic: Arc::new(pic), turns, rect, clip: rect }
    }

    #[test]
    fn screen_and_picture_points_round_trip_through_every_turn() {
        let rect = Rect::from_min_size(Pos2::new(100.0, 50.0), egui::vec2(300.0, 600.0));
        let word = Rect::from_min_max(Pos2::new(0.1, 0.2), Pos2::new(0.3, 0.25));
        for turns in 0..8 {
            let d = drawn(turns, rect);
            let shown = on_screen(&d, word);
            assert!(rect.contains_rect(shown), "{turns}: {shown:?}");
            // The middle of the box on screen is the middle of the word.
            let back = on_picture(&d, shown.center());
            assert!((back - word.center()).length() < 1e-4, "{turns}: {back:?}");
        }
        // Turned a quarter clockwise: the top-left of the picture is at the top right on screen.
        let d = drawn(1, rect);
        assert!((on_picture(&d, rect.right_top()) - Pos2::ZERO).length() < 1e-4);
    }
}
