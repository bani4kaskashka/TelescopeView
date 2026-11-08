//! Reads the words in a picture for Select text: Windows' own OCR, Apple's Vision on macOS,
//! and ocrs (pure Rust, Latin letters only, models shipped beside the app) everywhere else.
//! Each engine only turns pixels into lines of words; the slicing of very long pictures and
//! the joining of words into text happen here, the same for all of them.

#[cfg_attr(windows, path = "windows.rs")]
#[cfg_attr(target_os = "macos", path = "macos.rs")]
#[cfg_attr(not(any(windows, target_os = "macos")), path = "ocrs.rs")]
mod engine;

use eframe::egui::{Pos2, Rect};
use image::RgbaImage;
use std::ops::Range;
use std::path::PathBuf;
use std::sync::mpsc::{Receiver, TryRecvError, channel};

/// One word and where it is, in 0..1 of the picture as stored (before any turn on screen).
#[derive(Clone, Debug, PartialEq)]
pub struct Word {
    pub text: String,
    pub rect: Rect,
    /// Words with the same number are on one line.
    pub line: usize,
}

/// What an engine gives back: lines of words in reading order, boxes in pixels of the
/// picture it was handed as `[left, top, right, bottom]`.
type Lines = Vec<Vec<(String, [f32; 4])>>;

pub enum Source {
    File(PathBuf),
    Pixels(RgbaImage),
}

/// One picture being read on a thread of its own.
pub struct Job {
    result: Receiver<Result<Vec<Word>, String>>,
}

impl Job {
    pub fn start(source: Source) -> Self {
        let (tx, result) = channel();
        std::thread::spawn(move || {
            let img = match source {
                Source::File(path) => crate::edit::read_pixels(&path).map(|(img, _)| img),
                Source::Pixels(img) => Ok(img),
            };
            let _ = tx.send(img.and_then(|img| read(&img)));
        });
        Job { result }
    }

    pub fn finished(&self) -> Option<Result<Vec<Word>, String>> {
        match self.result.try_recv() {
            Ok(r) => Some(r),
            Err(TryRecvError::Disconnected) => Some(Err("it crashed".to_owned())),
            Err(TryRecvError::Empty) => None,
        }
    }
}

pub fn read(img: &RgbaImage) -> Result<Vec<Word>, String> {
    read_with(img, engine::max_side(), engine::read)
}

/// See-through parts go on white, a picture bigger than `limit` on both sides is shrunk until
/// one side fits, and the long side is then cut into overlapping slices of `limit` (webtoon
/// pages can be 30 times taller than wide, which no engine reads in one go). A word belongs to
/// the slice that has its middle furthest from an edge.
fn read_with(img: &RgbaImage, limit: u32, engine: impl Fn(&RgbaImage) -> Result<Lines, String>) -> Result<Vec<Word>, String> {
    let mut flat = img.clone();
    for p in flat.pixels_mut() {
        let a = p[3] as u32;
        for c in &mut p.0[..3] {
            *c = ((*c as u32 * a + 255 * (255 - a)) / 255) as u8;
        }
        p[3] = 255;
    }
    let (w, h) = flat.dimensions();
    if w == 0 || h == 0 {
        return Ok(Vec::new());
    }
    let shrink = (limit as f32 / w.min(h) as f32).min(1.0);
    if shrink < 1.0 {
        let size = |v: u32| ((v as f32 * shrink).round() as u32).max(1);
        flat = image::imageops::resize(&flat, size(w), size(h), image::imageops::FilterType::Triangle);
    }
    let (w, h) = flat.dimensions();
    let tall = h >= w;
    let long = if tall { h } else { w };
    let starts = slice_starts(long, limit);

    let mut words = Vec::new();
    let mut line = 0;
    for (k, &start) in starts.iter().enumerate() {
        let len = limit.min(long - start);
        let piece = if starts.len() == 1 {
            flat.clone()
        } else if tall {
            image::imageops::crop_imm(&flat, 0, start, w, len).to_image()
        } else {
            image::imageops::crop_imm(&flat, start, 0, len, h).to_image()
        };
        // Where this slice's share ends and the next one's begins: halfway through the overlap.
        let from = if k == 0 { 0.0 } else { (start + starts[k - 1] + limit) as f32 / 2.0 };
        let until = starts.get(k + 1).map_or(f32::INFINITY, |&next| (start + limit + next) as f32 / 2.0);
        for found in engine(&piece)? {
            let mut any = false;
            for (text, [l, t, r, b]) in found {
                let (l, t, r, b) = if tall { (l, t + start as f32, r, b + start as f32) } else { (l + start as f32, t, r + start as f32, b) };
                let middle = if tall { (t + b) / 2.0 } else { (l + r) / 2.0 };
                let text = text.trim();
                if text.is_empty() || middle < from || middle >= until {
                    continue;
                }
                let rect = Rect::from_min_max(Pos2::new(l / w as f32, t / h as f32), Pos2::new(r / w as f32, b / h as f32));
                words.push(Word { text: text.to_owned(), rect, line });
                any = true;
            }
            line += any as usize;
        }
    }
    Ok(words)
}

/// Where each slice of a side `long` long starts, `limit` per slice, overlapping by an eighth.
fn slice_starts(long: u32, limit: u32) -> Vec<u32> {
    if long <= limit {
        return vec![0];
    }
    let step = (limit - limit / 8).max(1);
    let mut starts: Vec<u32> = (0..).map(|k| k * step).take_while(|&s| s + limit < long).collect();
    starts.push(long - limit);
    starts
}

/// The words as text: spaces between words, a new line between lines, nothing between two
/// Chinese or Japanese characters (they're written without spaces).
pub fn text_of(words: &[Word]) -> String {
    let mut out = String::new();
    for (i, w) in words.iter().enumerate() {
        if i > 0 {
            let before = &words[i - 1];
            if before.line != w.line {
                out.push('\n');
            } else if !(before.text.chars().last().is_some_and(cjk) && w.text.chars().next().is_some_and(cjk)) {
                out.push(' ');
            }
        }
        out += &w.text;
    }
    out
}

fn cjk(c: char) -> bool {
    matches!(c, '\u{3000}'..='\u{30ff}' | '\u{3400}'..='\u{4dbf}' | '\u{4e00}'..='\u{9fff}' | '\u{ff00}'..='\u{ffef}')
}

/// The word nearest to `p` (0..1 of the picture), measured in pixels of a `size` picture, and
/// how far it is in word heights. `None` when there are no words.
pub fn nearest(words: &[Word], p: Pos2, size: [u32; 2]) -> Option<(usize, f32)> {
    let s = eframe::egui::vec2(size[0] as f32, size[1] as f32);
    words
        .iter()
        .enumerate()
        .map(|(i, w)| {
            let r = Rect::from_min_max((w.rect.min.to_vec2() * s).to_pos2(), (w.rect.max.to_vec2() * s).to_pos2());
            (i, r.distance_to_pos((p.to_vec2() * s).to_pos2()) / r.height().max(1.0))
        })
        .min_by(|a, b| a.1.total_cmp(&b.1))
}

/// A spot between two letters: the word, and how many of its letters come before it.
pub type Caret = (usize, usize);

fn letters(w: &Word) -> usize {
    w.text.chars().count()
}

/// The spot in word `i` nearest to `p` (0..1 of the picture). Engines only give word boxes, so
/// the letters share the box's width evenly.
pub fn caret(words: &[Word], i: usize, p: Pos2) -> Caret {
    let w = &words[i];
    let along = ((p.x - w.rect.min.x) / w.rect.width().max(f32::EPSILON)).clamp(0.0, 1.0);
    (i, (along * letters(w) as f32).round() as usize)
}

/// Before the first letter of word `i` and after its last.
pub fn whole(words: &[Word], i: usize) -> (Caret, Caret) {
    ((i, 0), (i, letters(&words[i])))
}

/// The letters between two spots (either way round), word by word, as ranges of letters.
/// Words with none of their letters between are left out.
pub fn span(words: &[Word], a: Caret, b: Caret) -> Vec<(usize, Range<usize>)> {
    let (from, to) = if a <= b { (a, b) } else { (b, a) };
    (from.0..=to.0)
        .filter_map(|i| {
            let n = letters(words.get(i)?);
            let start = if i == from.0 { from.1.min(n) } else { 0 };
            let end = if i == to.0 { to.1.min(n) } else { n };
            (start < end).then_some((i, start..end))
        })
        .collect()
}

/// The letters between two spots as text, joined like `text_of`.
pub fn span_text(words: &[Word], a: Caret, b: Caret) -> String {
    let parts: Vec<Word> = span(words, a, b)
        .into_iter()
        .map(|(i, r)| Word { text: words[i].text.chars().skip(r.start).take(r.len()).collect(), ..words[i].clone() })
        .collect();
    text_of(&parts)
}

/// The part of a word's box that letters `r` take up.
pub fn letters_rect(w: &Word, r: &Range<usize>) -> Rect {
    let n = letters(w).max(1) as f32;
    let x = |k: usize| w.rect.min.x + w.rect.width() * k as f32 / n;
    Rect::from_x_y_ranges(x(r.start)..=x(r.end), w.rect.y_range())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn word(text: &str, line: usize) -> Word {
        Word { text: text.to_owned(), rect: Rect::NOTHING, line }
    }

    #[test]
    fn words_join_into_lines() {
        let words = [word("Hello", 0), word("there,", 0), word("you", 1)];
        assert_eq!(text_of(&words), "Hello there,\nyou");
        // Japanese words come out one or a few characters each, without spaces between.
        assert_eq!(text_of(&[word("日本", 0), word("語", 0), word("OK", 0)]), "日本語 OK");
        assert_eq!(text_of(&[]), "");
    }

    #[test]
    fn slices_cover_the_side_and_overlap() {
        assert_eq!(slice_starts(500, 1000), [0]);
        assert_eq!(slice_starts(1000, 1000), [0]);
        let s = slice_starts(3000, 1000);
        assert_eq!(s, [0, 875, 1750, 2000]);
        assert!(s.windows(2).all(|p| p[1] < p[0] + 1000), "no gaps");
    }

    /// A pretend engine that sees a one-word line wherever a black box is (even one cut off
    /// by the slice's edge).
    fn squares(img: &RgbaImage) -> Result<Lines, String> {
        let black = |x: u32, y: u32| img.get_pixel(x, y)[0] == 0;
        let mut lines = Vec::new();
        for y in 0..img.height() {
            for x in 0..img.width() {
                if black(x, y) && (y == 0 || !black(x, y - 1)) && (x == 0 || !black(x - 1, y)) {
                    let bottom = (y..img.height()).find(|&b| !black(x, b)).unwrap_or(img.height());
                    let right = (x..img.width()).find(|&r| !black(r, y)).unwrap_or(img.width());
                    lines.push(vec![(format!("{x},{y}"), [x as f32, y as f32, right as f32, bottom as f32])]);
                }
            }
        }
        Ok(lines)
    }

    #[test]
    fn a_long_strip_is_read_in_slices_without_doubles() {
        // 40 wide, 400 tall: a 10 px "word" every 50 px down the left edge, from y = 45, so the
        // ones at 95 and 295 are cut by a slice's edge (slices start at 0, 88, 176, 264, 300).
        let mut img = RgbaImage::from_pixel(40, 400, image::Rgba([255, 255, 255, 255]));
        let top = |k: u32| 45 + k * 50;
        for k in 0..7 {
            for y in top(k)..top(k) + 10 {
                for x in 0..10 {
                    img.put_pixel(x, y, image::Rgba([0, 0, 0, 255]));
                }
            }
        }
        let words = read_with(&img, 100, squares).unwrap();
        // The short side (40) fits, so nothing is shrunk. Every box once, whole, in order.
        assert_eq!(words.len(), 7, "{words:?}");
        for (k, w) in words.iter().enumerate() {
            assert!((w.rect.min.y - top(k as u32) as f32 / 400.0).abs() < 1e-4, "{k}: {:?}", w.rect);
            assert!((w.rect.height() - 10.0 / 400.0).abs() < 1e-4, "{k}: {:?}", w.rect);
            assert_eq!(w.line, k);
        }
    }

    #[test]
    fn see_through_text_lands_on_white() {
        let img = RgbaImage::from_pixel(20, 20, image::Rgba([0, 0, 0, 0]));
        let seen = read_with(&img, 100, |p| {
            assert_eq!(p.get_pixel(5, 5).0, [255, 255, 255, 255]);
            Ok(Vec::new())
        });
        assert_eq!(seen, Ok(Vec::new()));
    }

    #[test]
    fn letters_are_picked_between_two_spots() {
        let mut hello = word("Hello", 0);
        hello.rect = Rect::from_min_max(Pos2::new(0.0, 0.0), Pos2::new(0.5, 0.1));
        let words = [hello, word("big", 0), word("world", 1)];
        // Letters share the box: a fifth of the way in is after "H"; before and past the ends clamp.
        assert_eq!(caret(&words, 0, Pos2::new(0.1, 0.05)), (0, 1));
        assert_eq!(caret(&words, 0, Pos2::new(-1.0, 0.05)), (0, 0));
        assert_eq!(caret(&words, 0, Pos2::new(9.0, 0.05)), (0, 5));
        // Part of one word, either way round.
        assert_eq!(span_text(&words, (0, 1), (0, 4)), "ell");
        assert_eq!(span_text(&words, (0, 4), (0, 1)), "ell");
        // Across words and lines.
        assert_eq!(span_text(&words, (0, 3), (2, 2)), "lo big
wo");
        // Ending right before a word's first letter leaves that word out.
        assert_eq!(span(&words, (0, 3), (1, 0)), [(0, 3..5)]);
        assert_eq!(span_text(&words, (1, 2), (1, 2)), "");
        let (a, b) = whole(&words, 2);
        assert_eq!(span_text(&words, a, b), "world");
        // A box for the letters picked.
        let r = letters_rect(&words[0], &(1..3));
        assert!((r.min.x - 0.1).abs() < 1e-6 && (r.max.x - 0.3).abs() < 1e-6, "{r:?}");
    }

    #[test]
    fn nearest_word_is_measured_in_word_heights() {
        let mut a = word("a", 0);
        a.rect = Rect::from_min_max(Pos2::new(0.1, 0.1), Pos2::new(0.2, 0.15));
        let mut b = word("b", 0);
        b.rect = Rect::from_min_max(Pos2::new(0.6, 0.1), Pos2::new(0.8, 0.15));
        let words = [a, b];
        // Inside b.
        assert_eq!(nearest(&words, Pos2::new(0.7, 0.12), [100, 100]), Some((1, 0.0)));
        // Five pixels left of a, which is five pixels tall: one word height away.
        let (i, d) = nearest(&words, Pos2::new(0.05, 0.12), [100, 100]).unwrap();
        assert_eq!(i, 0);
        assert!((d - 1.0).abs() < 1e-4, "{d}");
        assert_eq!(nearest(&[], Pos2::ZERO, [1, 1]), None);
    }
}
