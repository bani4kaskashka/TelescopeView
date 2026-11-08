//! The image itself: fit modes, zoom, pan, rotation, animation, the mouse wheel, and the
//! continuous vertical strip for webtoons.

use crate::app::{App, Drawn, Fit, Pic, Slot, compose, contrasting, is_fullscreen, shade_at};
use crate::files;
use crate::keys::Action;
use crate::theme;
use eframe::egui::{self, Align2, Color32, FontId, PointerButton, Pos2, Rect, Sense, Vec2};
use std::sync::Arc;
use std::time::Duration;

pub const MIN_ZOOM: f32 = 0.02;
pub const MAX_ZOOM: f32 = 40.0;
/// Points scrolled per wheel notch on a page taller than the window.
const WHEEL_STEP: f32 = 120.0;
/// Strip mode scrolls further per notch: webtoon panels are tall.
const STRIP_WHEEL_STEP: f32 = 180.0;
/// Touchpad scrolling turns at most one page per this long.
const TOUCHPAD_TURN_GAP: Duration = Duration::from_millis(350);

impl App {
    pub fn canvas(&mut self, ui: &mut egui::Ui) {
        let rect = ui.max_rect();
        let ctx = ui.ctx().clone();
        let resp = ui.interact(rect, ui.id().with("canvas"), Sense::click_and_drag());
        let painter = ui.painter_at(rect);
        self.view_size = rect.size();
        self.ppp = ctx.pixels_per_point();

        // Click zones: the outer thirds turn the page (mirrored for right to left), the middle
        // double-clicks to fullscreen. A double-click on a side is just two page turns.
        let zone = resp.interact_pointer_pos().map(|p| ((p.x - rect.left()) / rect.width() * 3.0).floor() as i32);
        let editing = self.edit.is_some();
        // Selecting text: the left button picks words instead (the middle button still pans).
        // Set a frame before the button goes up, so the release of a hold that started it
        // doesn't also turn the page.
        let selecting = self.text.is_some();
        let side = !editing && !selecting && self.settings.click_zones && !self.files.is_empty() && zone.is_some_and(|z| z != 1);
        if side && resp.clicked_by(PointerButton::Primary) {
            let towards_right = zone == Some(2);
            let forward = towards_right != (self.settings.rtl && !self.strip);
            if self.strip {
                self.scroll_or_turn(&ctx, if forward { 1.0 } else { -1.0 } * self.view_size.y * 0.85);
            } else {
                self.step(&ctx, if forward { 1 } else { -1 });
            }
        } else if !editing && !selecting && resp.double_clicked_by(PointerButton::Primary) {
            self.toggle_fullscreen(&ctx);
        }
        if !editing {
            resp.context_menu(|ui| self.actions_menu(ui));
        }
        // A click on the middle of a video plays / pauses it.
        if self.video.is_some() && !selecting && zone == Some(1) && resp.clicked_by(PointerButton::Primary) {
            self.toggle_pause();
        }
        if self.video.is_some() {
            self.video_controls(&ctx, rect);
        }

        painter.rect_filled(rect, 0.0, self.backdrop(&ctx));
        if is_fullscreen(&ctx) && !self.bars_visible(&ctx) {
            ctx.set_cursor_icon(egui::CursorIcon::None);
        }
        if self.files.is_empty() {
            self.effective_zoom = None;
            return; // The start screen is drawn by `ui.rs`.
        }

        // Wheel without Ctrl: lines and points, both counted downwards-positive in points.
        let (wheel_lines, wheel_points) = ctx.input(|i| {
            let (mut lines, mut points) = (0.0, 0.0);
            for e in &i.raw.events {
                if let egui::Event::MouseWheel { unit, delta, modifiers, .. } = e
                    && !modifiers.command
                {
                    match unit {
                        egui::MouseWheelUnit::Line => lines += delta.y,
                        egui::MouseWheelUnit::Point => points += delta.y,
                        egui::MouseWheelUnit::Page => lines += delta.y * 3.0,
                    }
                }
            }
            (lines, points)
        });
        let wheel = resp.hovered() && (wheel_lines != 0.0 || wheel_points != 0.0);

        if self.strip && !editing {
            if wheel {
                self.strip_pending -= wheel_lines * STRIP_WHEEL_STEP + wheel_points;
            }
            self.strip_view(&ctx, &resp, rect, &painter);
            self.text_layer(&ctx, &resp, &painter);
            return;
        }

        // Wheel: zooms around the cursor. Set in Settings, it scrolls a page taller than the
        // window and turns the page at its ends instead (a video always zooms);
        // a page that fits just turns. Ctrl+wheel (or a pinch) always zooms.
        let playing = self.current().is_some_and(|p| crate::video::is_video(p));
        if wheel && (!self.settings.wheel_turns || playing || editing) {
            let step = self.settings.zoom_step.max(1.01);
            self.wheel_zoom *= step.powf(wheel_lines) * (wheel_points / 200.0 * step.ln() / 0.2).exp();
        } else if wheel {
            if self.slack.y > 0.5 {
                // One notch at a time, so hitting the bottom and turning the page are two separate notches.
                let notches = wheel_lines.abs().round().max(1.0) as usize;
                let per = if wheel_lines != 0.0 { -wheel_lines.signum() * WHEEL_STEP } else { -wheel_points };
                for _ in 0..notches {
                    self.scroll_or_turn(&ctx, per);
                }
            } else {
                // Smooth-scroll deltas (touchpads) keep coming during a flick's momentum: one
                // page per flick, not dozens.
                let cooling = wheel_lines == 0.0 && self.last_turn.is_some_and(|t| t.elapsed() < TOUCHPAD_TURN_GAP);
                self.wheel_acc = if cooling { 0.0 } else { self.wheel_acc + wheel_lines + wheel_points / 50.0 };
                let steps = self.wheel_acc.trunc();
                if steps != 0.0 {
                    self.wheel_acc = 0.0;
                    let steps = if wheel_lines == 0.0 { steps.signum() } else { steps };
                    self.step(&ctx, -steps as isize);
                    self.last_turn = Some(std::time::Instant::now());
                }
            }
        }
        self.page_view(&ctx, &resp, rect, &painter);
        if !editing {
            self.hold_to_select(&ctx, &resp);
            self.text_layer(&ctx, &resp, &painter);
        }
    }

    /// What's behind the picture.
    pub fn backdrop(&self, ctx: &egui::Context) -> Color32 {
        match self.settings.backdrop {
            crate::app::Backdrop::Theme if is_fullscreen(ctx) => theme::bg_fullscreen(),
            crate::app::Backdrop::Theme => theme::bg(),
            crate::app::Backdrop::Black => Color32::BLACK,
            crate::app::Backdrop::Gray => Color32::from_gray(128),
            crate::app::Backdrop::White => Color32::WHITE,
        }
    }

    /// A small triangle on the side the mouse is on, when clicking there turns the page. It's
    /// black or white, whichever shows up on what's under it.
    fn side_arrow(&self, ctx: &egui::Context, resp: &egui::Response, rect: Rect, painter: &egui::Painter) {
        let idle = self.last_activity.elapsed().as_secs_f32();
        let wait = self.settings.bars_hide_secs;
        let Some(p) = resp.hover_pos() else { return };
        if !self.settings.click_zones || self.edit.is_some() || self.text.is_some() || idle >= wait {
            return;
        }
        ctx.request_repaint_after_secs(wait - idle);
        let third = rect.width() / 3.0;
        let right = p.x > rect.right() - third;
        if !right && p.x > rect.left() + third {
            return;
        }
        let forward = right != self.settings.rtl;
        if !(if forward { self.index + 1 < self.files.len() } else { self.index > 0 }) {
            return;
        }
        let side = if right { 1.0 } else { -1.0 };
        let center = Pos2::new(if right { rect.right() } else { rect.left() } - side * 24.0, rect.center().y);
        let tip = center + Vec2::new(side * 7.0, 0.0);
        let back = center - Vec2::new(side * 7.0, 0.0);
        let points = vec![tip, back + Vec2::new(0.0, 12.0), back - Vec2::new(0.0, 12.0)];
        let color = contrasting(self.lightness_at(ctx, center));
        painter.add(egui::Shape::convex_polygon(points, color, egui::Stroke::NONE));
    }

    /// How light (0..255) what's on screen is at `p`.
    fn lightness_at(&self, ctx: &egui::Context, p: Pos2) -> f32 {
        let back = crate::app::luma(self.backdrop(ctx));
        let Some(d) = self.drawn.iter().find(|d| d.rect.contains(p) && d.clip.contains(p)) else { return back };
        let undo_turn = (0..8).find(|&b| compose(d.turns, b) == 0).unwrap_or(0);
        let at = Pos2::ZERO + (p - d.rect.min) / d.rect.size();
        shade_at(&d.pic.shade, turn(at, undo_turn), back)
    }

    /// One page at a time: fit page / fit width / free zoom.
    fn page_view(&mut self, ctx: &egui::Context, resp: &egui::Response, rect: Rect, painter: &egui::Painter) {
        let current = self.current().cloned();
        let failed = current.as_ref().and_then(|p| match self.cache.get(p) {
            Some(Slot::Failed(e)) => Some(e.clone()),
            _ => None,
        });

        self.effective_zoom = None;
        if failed.is_some() || self.shown.is_none() {
            self.drawn.clear();
        }
        if let Some(err) = failed {
            let name = current.as_deref().map(files::file_name).unwrap_or_default();
            let msg = format!("Can't open {name}\n{err}");
            painter.text(rect.center(), Align2::CENTER_CENTER, msg, FontId::proportional(15.0), theme::error());
            self.slack = Vec2::ZERO;
            return;
        }

        let Some(shown) = &self.shown else {
            draw_loading(painter, rect.center(), ctx.input(|i| i.time));
            ctx.request_repaint();
            self.slack = Vec2::ZERO;
            return;
        };
        let (pic, since, shown_path) = (shown.pic.clone(), shown.since, shown.path.clone());
        // Editing: the preview takes the page's place, already turned and sized like the result.
        let edited = self.edit.as_ref().and_then(|e| e.pic());
        let is_edited = edited.is_some();
        let pic = edited.unwrap_or(pic);
        // Paused: the frame it stopped at (other pages of a spread keep playing).
        let paused = self.anim_pause;

        let ppp = self.ppp;
        let turns_of = |p: &std::path::PathBuf| self.rotation.get(p).copied().unwrap_or(0);
        let quarter_turns = if is_edited { 0 } else { turns_of(&shown_path) };
        // 1.0 zoom = one image pixel per physical screen pixel (of the first page).
        let first = pic.size_turned(quarter_turns) / ppp;
        // A wide page split in two: only one half is laid out (and drawn, see below).
        let split = current.as_ref() == Some(&shown_path) && self.split_now();
        let first = if split { Vec2::new(first.x / 2.0, first.y) } else { first };
        // The pages on screen, left to right: one, or a spread scaled to the same height. A
        // partner page still decoding gets a placeholder the size of the first.
        let mut parts: Vec<(Option<Arc<Pic>>, u8, Vec2)> = vec![(Some(pic.clone()), quarter_turns, first)];
        if current.as_ref() == Some(&shown_path)
            && let Some(j) = self.partner(self.index)
        {
            let other = self.ready(j).cloned();
            let turns = turns_of(&self.files[j]);
            let size = other.as_ref().map_or(first, |p| {
                let s = p.size_turned(turns);
                Vec2::new(s.x * first.y / s.y.max(1.0), first.y)
            });
            parts.push((other, turns, size));
        }
        if self.settings.rtl {
            parts.reverse();
        }
        let natural = Vec2::new(parts.iter().map(|p| p.2.x).sum(), first.y);
        let fit = match self.settings.fit {
            Fit::Page => (rect.width() / natural.x).min(rect.height() / natural.y),
            Fit::Width => rect.width() / natural.x,
        };
        let fit = if self.settings.enlarge_small { fit } else { fit.min(1.0) };
        let floor = min_zoom(rect.size(), natural, fit);
        let mut zoom = self.zoom.unwrap_or(fit).max(floor);

        let (mut factor, to_actual, to_fit) = self.zoom_keys(ctx);
        if to_fit {
            self.zoom = None;
            zoom = fit;
        }
        if let Some(z) = self.zoom_request.take() {
            factor *= z / zoom;
        }
        let mut anchor = rect.center();
        let (pinch, pointer) = ctx.input(|i| (i.zoom_delta(), i.pointer.hover_pos()));
        let pinch = pinch * std::mem::replace(&mut self.wheel_zoom, 1.0);
        if resp.hovered() && pinch != 1.0 {
            factor *= pinch;
            anchor = pointer.unwrap_or(anchor);
        }

        // Clamp first: a freshly shown page carries an "infinite" pan meaning top or bottom.
        let slack_for = |z: f32| ((natural * z - rect.size()) * 0.5).max(Vec2::ZERO);
        self.pan = self.pan.clamp(-slack_for(zoom), slack_for(zoom));

        if to_actual {
            self.zoom = Some(1.0);
            zoom = 1.0;
            self.pan = Vec2::new(0.0, f32::INFINITY);
        } else if factor != 1.0 {
            let new_zoom = (zoom * factor).clamp(floor, MAX_ZOOM);
            let center = rect.center() + self.pan;
            let offset = (center - anchor) * (new_zoom / zoom);
            self.pan = anchor + offset - rect.center();
            self.zoom = Some(new_zoom);
            zoom = new_zoom;
        }

        let cropping = self.edit.as_ref().is_some_and(|e| e.cropping || e.comparing);
        let left_pans = !cropping && self.text.is_none();
        if (resp.dragged_by(PointerButton::Primary) && left_pans) || resp.dragged_by(PointerButton::Middle) {
            self.pan += resp.drag_delta();
        }

        let size = natural * zoom;
        let slack = slack_for(zoom);
        self.pan = self.pan.clamp(-slack, slack);
        self.slack = slack;

        // Snap to whole physical pixels so 100% stays crisp.
        let min = rect.center() + self.pan - size * 0.5;
        let min = Pos2::new((min.x * ppp).round() / ppp, (min.y * ppp).round() / ppp);
        // A glide: the old page leaves on one side as the new one comes in from the other.
        let mut slide = Vec2::ZERO;
        if let Some(g) = &self.glide {
            let t = g.since.elapsed().as_secs_f32() / (self.settings.glide_ms.max(1) as f32 / 1000.0);
            if t >= 1.0 {
                self.glide = None;
            } else {
                let done = 1.0 - (1.0 - t).powi(3);
                let dir = if g.forward != self.settings.rtl { 1.0 } else { -1.0 };
                slide.x = dir * rect.width() * (1.0 - done);
                let out = Vec2::new(-dir * rect.width() * done, 0.0);
                for d in &g.from {
                    let clipped = painter.with_clip_rect(d.clip.translate(out).intersect(painter.clip_rect()));
                    draw_pic(&clipped, ctx, &d.pic, Duration::ZERO, d.rect.translate(out), d.turns, false);
                }
                ctx.request_repaint();
            }
        }
        let mut drawn = Vec::new();
        let mut x = min.x;
        // Which file each part is, in the same order as `parts`.
        let mut paths = vec![shown_path.clone()];
        if parts.len() > 1
            && let Some(j) = self.partner(self.index)
        {
            paths.push(self.files[j].clone());
        }
        if self.settings.rtl {
            paths.reverse();
        }
        for ((part, turns, part_size), path) in parts.iter().zip(paths) {
            let w = size.x * part_size.x / natural.x;
            let rest = Rect::from_min_size(Pos2::new(x, min.y), Vec2::new(w, size.y));
            let dest = rest.translate(slide);
            match part {
                Some(p) => {
                    if p.has_alpha && self.settings.checker {
                        // One repeating texture: 8 px squares on screen whatever the zoom.
                        let uv = Rect::from_min_size(Pos2::ZERO, dest.size() / 16.0);
                        painter.image(self.checker_tex.id(), dest, uv, Color32::WHITE);
                    }
                    let t = if Arc::ptr_eq(p, &pic) { paused } else { None };
                    let elapsed = t.unwrap_or_else(|| since.elapsed());
                    if split {
                        // The whole page, twice the width, slid so the half being read is in
                        // the frame, and clipped to it. Right to left starts with the right half.
                        let right = (self.half == 1) != self.settings.rtl;
                        let shift = if right { dest.width() } else { 0.0 };
                        let whole = Rect::from_min_size(
                            dest.min - Vec2::new(shift, 0.0),
                            Vec2::new(dest.width() * 2.0, dest.height()),
                        );
                        let clipped = painter.with_clip_rect(dest.intersect(painter.clip_rect()));
                        draw_pic(&clipped, ctx, p, elapsed, whole, *turns, t.is_none());
                        drawn.push(Drawn { path, pic: p.clone(), turns: *turns, rect: whole, clip: dest });
                    } else if is_edited {
                        // A crop is the whole picture slid and enlarged, clipped to the page.
                        let clipped = painter.with_clip_rect(dest.intersect(painter.clip_rect()));
                        draw_pic(&clipped, ctx, p, elapsed, dest, 0, false);
                        // Comparing: the picture without the dials up to the line.
                        if let Some(e) = &self.edit
                            && let Some(before) = e.before_pic()
                        {
                            let left = Rect::from_min_max(dest.min, Pos2::new(e.line_x(dest), dest.bottom()));
                            let clipped = painter.with_clip_rect(left.intersect(painter.clip_rect()));
                            clipped.rect_filled(left, 0.0, self.backdrop(ctx));
                            if p.has_alpha && self.settings.checker {
                                let uv = Rect::from_min_size(Pos2::ZERO, dest.size() / 16.0);
                                clipped.image(self.checker_tex.id(), dest, uv, Color32::WHITE);
                            }
                            draw_pic(&clipped, ctx, &before, elapsed, dest, 0, false);
                        }
                    } else {
                        draw_pic(painter, ctx, p, elapsed, dest, *turns, t.is_none());
                        // Where it is on screen (mid-glide too), so the next glide starts there.
                        drawn.push(Drawn { path, pic: p.clone(), turns: *turns, rect: dest, clip: dest });
                        // A playing video draws its frames over the poster, turned the same way.
                        if Arc::ptr_eq(p, &pic)
                            && let Some(v) = &self.video
                            && Some(&v.path) == current.as_ref()
                            && v.path == shown_path
                            && let Some(tex) = &v.texture
                        {
                            let all = Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0));
                            painter.add(tile_mesh(tex.id(), all, dest, *turns));
                        }
                    }
                }
                None => {
                    painter.rect_filled(dest, 0.0, theme::placeholder());
                    draw_loading(painter, dest.center(), ctx.input(|i| i.time));
                    ctx.request_repaint();
                }
            }
            x += w;
        }
        self.drawn = drawn;
        self.effective_zoom = Some(zoom);
        if let Some(e) = &mut self.edit {
            if e.cropping {
                e.crop_ui(resp, Rect::from_min_size(min, size), painter, ctx);
            } else if e.comparing {
                e.compare_ui(resp, Rect::from_min_size(min, size), painter, ctx);
            }
        }
        self.side_arrow(ctx, resp, rect, painter);

        if slack != Vec2::ZERO && resp.hovered() && !(is_fullscreen(ctx) && !self.bars_visible(ctx)) {
            ctx.set_cursor_icon(if resp.dragged() { egui::CursorIcon::Grabbing } else { egui::CursorIcon::Grab });
        }

        // A video still starting: dots over its first frame.
        if self.video_starting() {
            draw_loading(painter, rect.center(), ctx.input(|i| i.time));
            ctx.request_repaint();
        }

        // Still decoding the image we moved to: keep the old one up with a small marker.
        if current.as_ref().is_some_and(|c| *c != shown_path) {
            draw_loading(painter, rect.right_bottom() - Vec2::new(30.0, 22.0), ctx.input(|i| i.time));
            ctx.request_repaint();
        }
    }

    /// Every page stacked in one column, scrolled continuously.
    fn strip_view(&mut self, ctx: &egui::Context, resp: &egui::Response, rect: Rect, painter: &egui::Painter) {
        self.drawn.clear();
        // Width: Ctrl+wheel and + / - narrow or widen the column, 0 / 1 back to full width.
        let (factor, actual, fit) = self.zoom_keys(ctx);
        let reset = actual || fit;
        let factor = factor * if resp.hovered() { ctx.input(|i| i.zoom_delta()) } else { 1.0 };
        if reset {
            self.settings.strip_width = 1.0;
        } else if factor != 1.0 {
            // Keep the spot under the view's top line where it is while the column resizes.
            let before = self.strip_page_height(self.index);
            self.settings.strip_width = (self.settings.strip_width * factor).clamp(0.15, 1.0);
            self.strip_offset *= self.strip_page_height(self.index) / before.max(1.0);
        }
        if (resp.dragged_by(PointerButton::Primary) && self.text.is_none()) || resp.dragged_by(PointerButton::Middle) {
            self.strip_scroll_now(ctx, -resp.drag_delta().y, false);
            self.strip_pending = 0.0;
        }
        // Eased scrolling: covers most of the remaining distance within ~100 ms.
        if self.strip_pending != 0.0 {
            // Real time since the last eased step: egui's frame-time estimate runs low here.
            let now = std::time::Instant::now();
            let dt = self.strip_tick.map_or(1.0 / 60.0, |t| (now - t).as_secs_f32()).clamp(0.001, 0.05);
            self.strip_tick = Some(now);
            let step = if self.strip_pending.abs() < 1.0 || self.settings.reduce_motion {
                self.strip_pending
            } else {
                self.strip_pending * (1.0 - (-dt * 22.0).exp())
            };
            self.strip_pending -= step;
            if !self.strip_scroll_now(ctx, step, true) {
                self.strip_pending = 0.0;
            }
            ctx.request_repaint();
        } else if self.slideshow.is_none() {
            self.strip_tick = None;
        }

        let time = ctx.input(|i| i.time);
        let elapsed = self.started.elapsed();
        let mut y = rect.top() - self.strip_offset;
        let mut i = self.index;
        let mut next_frame: Option<Duration> = None;
        let mut loading = false;
        // Pages above the current one, when it starts below the top (only at the very start).
        while i < self.files.len() && y < rect.bottom() {
            let w = self.strip_page_width(i);
            let h = self.strip_page_height(i);
            let page = Rect::from_min_size(Pos2::new(rect.center().x - w / 2.0, y), Vec2::new(w, h));
            let page = Rect::from_min_max(snap(page.min, self.ppp), snap(page.max, self.ppp));
            match self.cache.get(&self.files[i]) {
                Some(Slot::Ready(pic)) => {
                    let turns = self.rotation.get(&self.files[i]).copied().unwrap_or(0);
                    if let Some(wait) = draw_pic(painter, ctx, pic, elapsed, page, turns, true) {
                        next_frame = Some(next_frame.map_or(wait, |n| n.min(wait)));
                    }
                    let path = self.files[i].clone();
                    self.drawn.push(Drawn { path, pic: pic.clone(), turns, rect: page, clip: page });
                    let s = pic.size_turned(turns);
                    self.strip_aspect = s.y / s.x.max(1.0);
                    if i == self.index {
                        self.effective_zoom = Some(w * self.ppp / s.x.max(1.0));
                    }
                }
                Some(Slot::Failed(e)) => {
                    let msg = format!("Can't open {}\n{e}", files::file_name(&self.files[i]));
                    painter.text(page.center(), Align2::CENTER_CENTER, msg, FontId::proportional(14.0), theme::error());
                }
                None => {
                    painter.rect_filled(page, 0.0, theme::placeholder());
                    let c = Pos2::new(
                        page.center().x,
                        page.top().max(rect.top())
                            + (page.bottom().min(rect.bottom()) - page.top().max(rect.top())) / 2.0,
                    );
                    draw_loading(painter, c, time);
                    loading = true;
                }
            }
            y += h;
            i += 1;
        }
        let last = i.saturating_sub(1);
        if last != self.strip_last_visible {
            self.strip_last_visible = last;
            self.refresh(ctx);
        }
        if loading {
            ctx.request_repaint();
        } else if let Some(wait) = next_frame {
            ctx.request_repaint_after(wait);
        }
        self.slack = Vec2::ZERO;
    }
}

/// How far out the zoom goes: half the window, or the size the picture fits at if that's
/// smaller, so a small picture can't shrink to a dot. Never above its own size, so actual
/// size (the 1 key) always works.
fn min_zoom(view: Vec2, natural: Vec2, fit: f32) -> f32 {
    let whole = (view.x / natural.x).min(view.y / natural.y);
    (whole * 0.5).min(fit).min(1.0)
}

fn snap(p: Pos2, ppp: f32) -> Pos2 {
    Pos2::new((p.x * ppp).round() / ppp, (p.y * ppp).round() / ppp)
}

/// Zoom keys: plus and minus, and 1 / 0 for "back to normal". Read outside other `ctx.input` calls:
/// calling back into ctx while its input lock is held deadlocks.
/// Returns (zoom factor, 1 = actual size, 0 = fit).
impl App {
    fn zoom_keys(&self, ctx: &egui::Context) -> (f32, bool, bool) {
        if ctx.egui_wants_keyboard_input() || self.capturing.is_some() {
            return (1.0, false, false);
        }
        let n = ctx.input(|i| self.keymap.presses(i));
        let steps = n[Action::ZoomIn as usize] as i32 - n[Action::ZoomOut as usize] as i32;
        (self.settings.zoom_step.powi(steps), n[Action::ActualSize as usize] > 0, n[Action::ZoomFit as usize] > 0)
    }
}

/// Draws the frame of `pic` that is due `elapsed` into its animation, turned by
/// `quarter_turns` x 90° clockwise to fill `rect`. Returns how long until the next frame.
fn draw_pic(
    painter: &egui::Painter,
    ctx: &egui::Context,
    pic: &Pic,
    elapsed: Duration,
    rect: Rect,
    quarter_turns: u8,
    playing: bool,
) -> Option<Duration> {
    let (frame, wait) = pic.frame_at(elapsed);
    let clip = painter.clip_rect();
    for (tex, tile) in &frame.tiles {
        let shape = tile_mesh(tex.id(), *tile, rect, quarter_turns);
        // Long strips are many tiles; skip the ones far off screen.
        if shape.visual_bounding_rect().intersects(clip) {
            painter.add(shape);
        }
    }
    let wait = wait.filter(|_| playing);
    if let Some(w) = wait {
        ctx.request_repaint_after(w);
    }
    wait
}

/// A point of the image (0..1 coordinates) to where it lands after turning it clockwise
/// (mirrored left to right first when bit 2 is set, see `app::compose`).
pub fn turn(p: Pos2, quarter_turns: u8) -> Pos2 {
    let p = if quarter_turns & 4 != 0 { Pos2::new(1.0 - p.x, p.y) } else { p };
    match quarter_turns & 3 {
        1 => Pos2::new(1.0 - p.y, p.x),
        2 => Pos2::new(1.0 - p.x, 1.0 - p.y),
        3 => Pos2::new(p.y, 1.0 - p.x),
        _ => p,
    }
}

/// One textured quad for the part of the image `tile` covers.
fn tile_mesh(tex: egui::TextureId, tile: Rect, dest: Rect, quarter_turns: u8) -> egui::Shape {
    let corners = [tile.left_top(), tile.right_top(), tile.right_bottom(), tile.left_bottom()];
    let uvs = [Pos2::new(0.0, 0.0), Pos2::new(1.0, 0.0), Pos2::new(1.0, 1.0), Pos2::new(0.0, 1.0)];
    let mut mesh = egui::Mesh::with_texture(tex);
    for (corner, uv) in corners.into_iter().zip(uvs) {
        let t = turn(corner, quarter_turns);
        let pos = dest.min + Vec2::new(t.x * dest.width(), t.y * dest.height());
        mesh.vertices.push(egui::epaint::Vertex { pos, uv, color: Color32::WHITE });
    }
    mesh.indices.extend_from_slice(&[0, 1, 2, 0, 2, 3]);
    egui::Shape::mesh(mesh)
}

/// Three small squares lighting up in turn.
pub fn draw_loading(painter: &egui::Painter, center: Pos2, time: f64) {
    let lit = (time * 6.0) as usize % 3;
    for k in 0..3 {
        let c = center + Vec2::new((k as f32 - 1.0) * 10.0, 0.0);
        let color = if k == lit { theme::text() } else { theme::icon_disabled() };
        painter.rect_filled(Rect::from_center_size(c, Vec2::splat(6.0)), 0.0, color);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn positions(tile: Rect, turns: u8) -> Vec<Pos2> {
        let dest = Rect::from_min_size(Pos2::ZERO, Vec2::splat(1.0));
        match tile_mesh(egui::TextureId::default(), tile, dest, turns) {
            egui::Shape::Mesh(m) => m.vertices.iter().map(|v| v.pos).collect(),
            _ => unreachable!(),
        }
    }

    #[test]
    fn zooming_out_stops_before_a_picture_gets_tiny() {
        let view = Vec2::new(1000.0, 800.0);
        // Big: half the window.
        assert_eq!(min_zoom(view, Vec2::new(4000.0, 2000.0), 0.25), 0.125);
        // Small, shown at its own size: no smaller than that.
        assert_eq!(min_zoom(view, Vec2::new(50.0, 50.0), 1.0), 1.0);
        // A long strip at fill width can still go out to half the window's height.
        assert_eq!(min_zoom(view, Vec2::new(800.0, 16000.0), 1.25), 0.025);
        // Small and enlarged to fit: actual size is still allowed.
        assert_eq!(min_zoom(view, Vec2::new(50.0, 50.0), 16.0), 1.0);
    }

    #[test]
    fn undoing_a_turn_finds_the_same_spot() {
        let p = Pos2::new(0.2, 0.7);
        for q in 0..8 {
            let undo = (0..8).find(|&b| compose(q, b) == 0).unwrap();
            assert!((turn(turn(p, q), undo) - p).length() < 1e-5, "{q}");
        }
    }

    #[test]
    fn rotation_maps_corners_clockwise() {
        let full = Rect::from_min_size(Pos2::ZERO, Vec2::splat(1.0));
        // Turned once clockwise, the image's top-left corner lands top-right on screen.
        assert_eq!(positions(full, 1)[0], Pos2::new(1.0, 0.0));
        assert_eq!(positions(full, 1)[3], Pos2::new(0.0, 0.0), "bottom-left goes top-left");
        assert_eq!(positions(full, 2)[0], Pos2::new(1.0, 1.0));
        assert_eq!(positions(full, 0), positions(full, crate::app::compose(3, 1)));
        // The lower half of a tall image ends up on the left after a clockwise turn.
        let lower = Rect::from_min_max(Pos2::new(0.0, 0.5), Pos2::new(1.0, 1.0));
        assert!(positions(lower, 1).iter().all(|p| p.x <= 0.5));
    }

    #[test]
    fn flips_compose_like_the_exif_tag() {
        use crate::app::{FLIP_H, FLIP_V, compose};
        let full = Rect::from_min_size(Pos2::ZERO, Vec2::splat(1.0));
        assert_eq!(positions(full, FLIP_H)[0], Pos2::new(1.0, 0.0), "top-left goes top-right");
        assert_eq!(positions(full, FLIP_V)[0], Pos2::new(0.0, 1.0), "top-left goes bottom-left");
        assert_eq!(compose(FLIP_H, FLIP_H), 0);
        assert_eq!(compose(FLIP_H, FLIP_V), 2, "both flips = half a turn");
        for a in 0..8 {
            for b in 0..8 {
                let p = Pos2::new(0.2, 0.7);
                let (one, two) = (turn(p, compose(a, b)), turn(turn(p, a), b));
                assert!((one - two).length() < 1e-5, "{a} then {b}: {one:?} vs {two:?}");
            }
        }
    }
}
