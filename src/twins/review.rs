//! Walks through the look-alike pairs and remembers the choices. Nothing touches the disk here:
//! files are only marked, so every choice can be undone until the host deletes the marked ones.
//!
//! Marking a file settles every other pair it was in, so three copies of a picture take two
//! choices and not three.

use super::scan::{FileInfo, Pair, Report};

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Choice {
    KeepLeft,
    KeepRight,
    KeepBoth,
    DeleteBoth,
    /// Keep everything left in this set of look-alikes and move on.
    KeepSet,
}

struct Step {
    marked: Vec<u32>,
    kept: Vec<usize>,
}

/// The pair being looked at.
pub struct Current<'a> {
    pub pair: Pair,
    pub left: &'a FileInfo,
    pub right: &'a FileInfo,
    /// 1-based position among the look-alike sets, and how many there are.
    pub set: usize,
    pub sets: usize,
    /// Pairs still open in this set, this one included.
    pub open_in_set: usize,
}

pub struct Review {
    report: Report,
    threshold: f32,
    /// Pair indices at this threshold, grouped by set, best match first.
    order: Vec<usize>,
    /// Set number of each entry in `order`.
    set_of: Vec<usize>,
    sets: usize,
    marked: Vec<bool>,
    kept: Vec<bool>,
    history: Vec<Step>,
}

impl Review {
    pub fn new(report: Report, threshold: f32) -> Self {
        let (files, pairs) = (report.files.len(), report.pairs.len());
        let mut review = Self {
            report,
            threshold,
            order: Vec::new(),
            set_of: Vec::new(),
            sets: 0,
            marked: vec![false; files],
            kept: vec![false; pairs],
            history: Vec::new(),
        };
        review.rebuild();
        review
    }

    pub fn report(&self) -> &Report {
        &self.report
    }

    pub fn threshold(&self) -> f32 {
        self.threshold
    }

    /// Instant: nothing is scanned again and earlier choices stay.
    pub fn set_threshold(&mut self, threshold: f32) {
        if self.threshold != threshold {
            self.threshold = threshold;
            self.rebuild();
        }
    }

    fn visible(&self, pair: &Pair) -> bool {
        pair.identical || pair.score >= self.threshold
    }

    fn rebuild(&mut self) {
        // Look-alikes chain into sets: A like B and B like C is one set of three.
        let mut parent: Vec<u32> = (0..self.report.files.len() as u32).collect();
        fn root(parent: &mut [u32], mut i: u32) -> u32 {
            while parent[i as usize] != i {
                parent[i as usize] = parent[parent[i as usize] as usize];
                i = parent[i as usize];
            }
            i
        }
        let mut shown = Vec::new();
        for (i, p) in self.report.pairs.iter().enumerate() {
            if self.visible(p) {
                let (a, b) = (root(&mut parent, p.left), root(&mut parent, p.right));
                parent[a as usize] = b;
                shown.push(i);
            }
        }
        // `pairs` comes sorted best first, so a set's place is that of its best pair.
        let mut set_number = std::collections::HashMap::new();
        let mut members: Vec<Vec<usize>> = Vec::new();
        for i in shown {
            let r = root(&mut parent, self.report.pairs[i].left);
            let set = *set_number.entry(r).or_insert_with(|| {
                members.push(Vec::new());
                members.len() - 1
            });
            members[set].push(i);
        }
        self.sets = members.len();
        self.order.clear();
        self.set_of.clear();
        for (set, list) in members.into_iter().enumerate() {
            self.set_of.extend(std::iter::repeat_n(set, list.len()));
            self.order.extend(list);
        }
    }

    fn open(&self, pair_index: usize) -> bool {
        let p = &self.report.pairs[pair_index];
        !self.kept[pair_index] && !self.marked[p.left as usize] && !self.marked[p.right as usize]
    }

    fn position(&self) -> Option<usize> {
        self.order.iter().position(|&i| self.open(i))
    }

    pub fn current(&self) -> Option<Current<'_>> {
        let at = self.position()?;
        let pair = self.report.pairs[self.order[at]];
        let set = self.set_of[at];
        let open_in_set =
            self.order.iter().zip(&self.set_of).filter(|&(&i, &s)| s == set && self.open(i)).count();
        Some(Current {
            pair,
            left: &self.report.files[pair.left as usize],
            right: &self.report.files[pair.right as usize],
            set: set + 1,
            sets: self.sets,
            open_in_set,
        })
    }

    /// The pairs after the current one, to load their pictures ahead of time.
    pub fn upcoming(&self, count: usize) -> Vec<Pair> {
        self.order.iter().filter(|&&i| self.open(i)).skip(1).take(count).map(|&i| self.report.pairs[i]).collect()
    }

    pub fn choose(&mut self, choice: Choice) {
        let Some(at) = self.position() else { return };
        let index = self.order[at];
        let pair = self.report.pairs[index];
        let mut step = Step { marked: Vec::new(), kept: Vec::new() };
        match choice {
            Choice::KeepLeft => step.marked.push(pair.right),
            Choice::KeepRight => step.marked.push(pair.left),
            Choice::DeleteBoth => step.marked.extend([pair.left, pair.right]),
            Choice::KeepBoth => step.kept.push(index),
            Choice::KeepSet => {
                let set = self.set_of[at];
                step.kept.extend(
                    self.order.iter().zip(&self.set_of).filter(|&(&i, &s)| s == set && self.open(i)).map(|(&i, _)| i),
                );
            }
        }
        for &f in &step.marked {
            self.marked[f as usize] = true;
        }
        for &p in &step.kept {
            self.kept[p] = true;
        }
        self.history.push(step);
    }

    pub fn can_undo(&self) -> bool {
        !self.history.is_empty()
    }

    pub fn undo(&mut self) {
        let Some(step) = self.history.pop() else { return };
        for f in step.marked {
            self.marked[f as usize] = false;
        }
        for p in step.kept {
            self.kept[p] = false;
        }
    }

    pub fn marked(&self) -> Vec<&FileInfo> {
        self.report.files.iter().zip(&self.marked).filter(|&(_, &m)| m).map(|(f, _)| f).collect()
    }

    #[cfg(test)]
    pub fn marked_paths(&self) -> Vec<std::path::PathBuf> {
        self.marked().into_iter().map(|f| f.path.clone()).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn report(files: usize, pairs: &[(u32, u32, f32)]) -> Report {
        Report {
            files: (0..files)
                .map(|i| FileInfo { path: format!("{i}.png").into(), bytes: 10, modified: None, size: [1, 1] })
                .collect(),
            pairs: pairs.iter().map(|&(left, right, score)| Pair { left, right, score, identical: false }).collect(),
            ..Default::default()
        }
    }

    fn names(r: &Review) -> Vec<String> {
        r.marked_paths().iter().map(|p| p.display().to_string()).collect()
    }

    #[test]
    fn keep_left_marks_right_and_settles_its_other_pairs() {
        let mut r = Review::new(report(3, &[(0, 1, 0.99), (0, 2, 0.98), (1, 2, 0.97)]), 0.9);
        assert_eq!(r.current().unwrap().sets, 1);
        r.choose(Choice::KeepLeft); // Marks 1, which also settles 1-2.
        assert_eq!(names(&r), ["1.png"]);
        let now = r.current().unwrap();
        assert_eq!((now.pair.left, now.pair.right), (0, 2));
        r.choose(Choice::KeepRight); // Marks 0.
        assert_eq!(names(&r), ["0.png", "1.png"]);
        assert!(r.current().is_none());
    }

    #[test]
    fn keep_both_moves_on_without_marking() {
        let mut r = Review::new(report(3, &[(0, 1, 0.99), (0, 2, 0.98)]), 0.9);
        r.choose(Choice::KeepBoth);
        assert!(r.marked().is_empty());
        assert_eq!(r.current().unwrap().pair.right, 2);
    }

    #[test]
    fn delete_both_marks_both() {
        let mut r = Review::new(report(2, &[(0, 1, 0.99)]), 0.9);
        r.choose(Choice::DeleteBoth);
        assert_eq!(names(&r), ["0.png", "1.png"]);
        assert!(r.current().is_none());
    }

    #[test]
    fn undo_puts_everything_back() {
        let mut r = Review::new(report(3, &[(0, 1, 0.99), (0, 2, 0.98), (1, 2, 0.97)]), 0.9);
        r.choose(Choice::KeepLeft);
        r.choose(Choice::KeepSet);
        assert!(r.current().is_none());
        r.undo();
        assert_eq!(r.current().unwrap().pair.right, 2);
        r.undo();
        assert!(r.marked().is_empty());
        assert_eq!(r.current().unwrap().pair.right, 1);
        assert!(!r.can_undo());
    }

    #[test]
    fn threshold_filters_and_keeps_choices() {
        let mut r = Review::new(report(4, &[(0, 1, 0.99), (2, 3, 0.8)]), 0.9);
        assert_eq!(r.current().unwrap().sets, 1);
        r.choose(Choice::KeepLeft);
        assert!(r.current().is_none());
        r.set_threshold(0.7);
        assert_eq!(names(&r), ["1.png"]);
        let now = r.current().unwrap();
        assert_eq!((now.pair.left, now.set, now.sets), (2, 2, 2));
    }

    #[test]
    fn identical_files_show_at_any_threshold() {
        let mut rep = report(2, &[(0, 1, 1.0)]);
        rep.pairs[0].identical = true;
        let r = Review::new(rep, 1.0);
        assert!(r.current().is_some());
    }

    #[test]
    fn sets_stay_together_and_keep_set_clears_one() {
        // Sorted best first, but 0-1-2 chain into one set around the unrelated 3-4.
        let mut r = Review::new(report(5, &[(0, 1, 0.99), (3, 4, 0.98), (1, 2, 0.97)]), 0.9);
        assert_eq!(r.current().unwrap().open_in_set, 2);
        r.choose(Choice::KeepSet);
        let now = r.current().unwrap();
        assert_eq!((now.pair.left, now.set, now.sets), (3, 2, 2));
    }
}
