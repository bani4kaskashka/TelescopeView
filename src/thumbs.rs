//! The thumbnail panel: every picture in the folder as a small square, docked to any side
//! (always there, or only while the mouse is at that edge) or floating where it was dragged.

use crate::app::{App, Dock};
use crate::files;
use crate::theme;
use eframe::egui::{self, Pos2, Rect, Sense, Stroke, Vec2};
use std::path::PathBuf;

/// Space around each thumbnail.
const GAP: f32 = 6.0;
/// How close to the edge the mouse has to be to bring out a hover-only panel.
const EDGE: f32 = 10.0;
/// How long a hover-only panel lingers after the mouse leaves it, in seconds.
const GRACE: f64 = 0.4;
/// How long the panel shows itself after being switched on, in seconds.
const PEEK: f64 = 1.5;
/// How long the hover-only panel takes to slide in or out, in seconds.
const SLIDE: f32 = 0.15;

/// The side of `area` closest to `p`.
fn nearest_side(area: Rect, p: Pos2) -> Dock {
    let d = [
        (p.x - area.left(), Dock::Left),
        (area.right() - p.x, Dock::Right),
        (p.y - area.top(), Dock::Top),
        (area.bottom() - p.y, Dock::Bottom),
    ];
    d.into_iter().min_by(|a, b| a.0.total_cmp(&b.0)).map_or(Dock::Left, |(_, side)| side)
}

/// Where a panel `t` thick sits on `side` of `area`.
fn strip_on(side: Dock, area: Rect, t: f32) -> Rect {
    match side {
        Dock::Left => Rect::from_min_size(area.min, Vec2::new(t, area.height())),
        Dock::Right => Rect::from_min_size(Pos2::new(area.right() - t, area.top()), Vec2::new(t, area.height())),
        Dock::Top => Rect::from_min_size(area.min, Vec2::new(area.width(), t)),
        _ => Rect::from_min_size(Pos2::new(area.left(), area.bottom() - t), Vec2::new(area.width(), t)),
    }
}

/// How far a hover-only panel travels: its thickness across the edge it hangs from.
fn travel(dock: Dock, strip: Rect) -> f32 {
    if matches!(dock, Dock::Top | Dock::Bottom) { strip.height() } else { strip.width() }
}

impl App {
    /// Thumbnails on or off. Switched on, the panel slides out for a moment so it's clear
    /// where it lives.
    pub fn toggle_thumbs(&mut self, ctx: &egui::Context) {
        self.settings.thumbs_on = !self.settings.thumbs_on;
        if self.settings.thumbs_on {
            self.thumbs_until = ctx.input(|i| i.time) + PEEK;
        }
    }

    fn thumbs_thickness(&self) -> f32 {
        self.settings.thumb_size + 2.0 * GAP + 8.0
    }

    /// Lays out the panel for this frame. `area` is where the picture goes (between the bars):
    /// a docked panel takes its edge, a hover panel and a floating one sit over it.
    pub fn thumbs(&mut self, ui: &mut egui::Ui, ctx: &egui::Context, area: Rect) {
        if self.files.is_empty() || !self.settings.thumbs_on {
            self.thumbs_until = 0.0;
            return;
        }
        self.thumbs_area = area;
        let dock = self.settings.thumbs_dock;
        let t = self.thumbs_thickness();
        let vertical = !matches!(dock, Dock::Top | Dock::Bottom);
        match dock {
            Dock::Floating => {
                let height = (area.height() * 0.7).max(200.0);
                egui::Area::new(egui::Id::new("thumbs-floating"))
                    .default_pos(area.left_top() + Vec2::new(16.0, 16.0))
                    .movable(true)
                    .constrain(true)
                    .order(egui::Order::Foreground)
                    .show(ctx, |ui| {
                        let frame = egui::Frame::NONE.fill(theme::bar()).stroke(Stroke::new(1.0, theme::line()));
                        frame.show(ui, |ui| {
                            ui.set_width(t);
                            // The grip: drag here to move it.
                            let (grip, resp) = ui.allocate_exact_size(Vec2::new(t, 14.0), Sense::drag());
                            for k in 0..3 {
                                let c = grip.center() + Vec2::new((k as f32 - 1.0) * 6.0, 0.0);
                                ui.painter().rect_filled(
                                    Rect::from_center_size(c, Vec2::splat(2.0)),
                                    0.0,
                                    theme::text_dim(),
                                );
                            }
                            resp.on_hover_cursor(egui::CursorIcon::Grab);
                            ui.allocate_ui(Vec2::new(t, height), |ui| {
                                ui.set_height(height);
                                self.thumbs_list(ui, true, false);
                            });
                        });
                    });
            }
            _ if self.settings.thumbs_pinned => {
                let frame = egui::Frame::NONE.fill(theme::bar());
                let panel = match dock {
                    Dock::Left => egui::Panel::left("thumbs"),
                    Dock::Right => egui::Panel::right("thumbs"),
                    Dock::Top => egui::Panel::top("thumbs"),
                    _ => egui::Panel::bottom("thumbs"),
                };
                panel.exact_size(t).resizable(false).frame(frame).show(ui, |ui| self.thumbs_list(ui, vertical, true));
            }
            _ => {
                let (strip, edge) = (strip_on(dock, area, t), strip_on(dock, area, EDGE));
                let (pointer, now) = ctx.input(|i| (i.pointer.hover_pos(), i.time));
                let out = now < self.thumbs_until;
                let keep = match pointer {
                    Some(p) if edge.contains(p) => true,
                    Some(p) => out && strip.contains(p),
                    None => false,
                } || (out && (egui::Popup::is_any_open(ctx) || self.thumbs_drag));
                if keep {
                    self.thumbs_until = now + GRACE;
                }
                let shown = now < self.thumbs_until;
                if shown {
                    ctx.request_repaint_after_secs((self.thumbs_until - now) as f32);
                }
                // Slides in from its edge and back out again.
                let t = ctx.animate_bool_with_time(egui::Id::new("thumbs-slide"), shown, SLIDE);
                if t > 0.0 {
                    let hidden = (1.0 - t) * travel(dock, strip);
                    let shift = match dock {
                        Dock::Left => Vec2::new(-hidden, 0.0),
                        Dock::Right => Vec2::new(hidden, 0.0),
                        Dock::Top => Vec2::new(0.0, -hidden),
                        _ => Vec2::new(0.0, hidden),
                    };
                    egui::Area::new(egui::Id::new("thumbs-hover"))
                        .fixed_pos(strip.min + shift)
                        .constrain(false)
                        .fade_in(false)
                        .order(egui::Order::Foreground)
                        .show(ctx, |ui| {
                            ui.set_clip_rect(strip);
                            egui::Frame::NONE.fill(theme::bar_overlay()).show(ui, |ui| {
                                ui.set_min_size(strip.size());
                                ui.set_max_size(strip.size());
                                self.thumbs_list(ui, vertical, true);
                            });
                        });
                }
            }
        }
        // Being dragged: show the side it will go to.
        if self.thumbs_drag
            && let Some(p) = ctx.input(|i| i.pointer.latest_pos())
        {
            let target = nearest_side(area, p);
            let r = strip_on(target, area, t);
            let painter = ctx.layer_painter(egui::LayerId::new(egui::Order::Tooltip, egui::Id::new("thumbs-target")));
            painter.rect(
                r,
                0.0,
                theme::selected().gamma_multiply(0.6),
                Stroke::new(2.0, theme::text()),
                egui::StrokeKind::Inside,
            );
            ctx.set_cursor_icon(egui::CursorIcon::Grabbing);
        }
    }

    /// The thumbnails themselves, in a scrolling column (or row). Only the visible ones are
    /// drawn and asked for.
    /// `snap`: dragging the panel moves it to another side (a floating one moves freely instead).
    fn thumbs_list(&mut self, ui: &mut egui::Ui, vertical: bool, snap: bool) {
        let n = self.files.len();
        let panel = ui.max_rect();
        let s = self.settings.thumb_size;
        let cell = s + GAP;
        let length = if vertical { ui.available_height() } else { ui.available_width() };
        let mut scroll = if vertical { egui::ScrollArea::vertical() } else { egui::ScrollArea::horizontal() };
        scroll = scroll.id_salt(("thumbs", vertical)).auto_shrink([false, false]).scroll_source(
            // Wheel and scroll bar only: a drag on the floating panel moves the panel.
            egui::scroll_area::ScrollSource {
                scroll_bar: true,
                mouse_wheel: true,
                ..egui::scroll_area::ScrollSource::NONE
            },
        );
        // The picture on screen moved out of view (or the panel just opened): bring it to the middle.
        let (lo, hi) = (self.index as f32 * cell, (self.index + 1) as f32 * cell);
        let place = self.settings.thumbs_dock as u8;
        if self.thumbs_view.is_none_or(|(a, b, i, d)| d != place || (i != self.index && (lo < a || hi > b))) {
            let at = (lo + cell / 2.0 - length / 2.0).max(0.0);
            scroll = scroll.scroll_offset(if vertical { Vec2::new(0.0, at) } else { Vec2::new(at, 0.0) });
        }
        let ctx = ui.ctx().clone();
        scroll.show_viewport(ui, |ui, view| {
            let origin = ui.max_rect().min;
            let cross = if vertical { ui.available_width() } else { ui.available_height() };
            let total = n as f32 * cell + GAP;
            let content = if vertical { Vec2::new(cross, total) } else { Vec2::new(total, cross) };
            let sense = if snap { Sense::click_and_drag() } else { Sense::click() };
            let bg = ui.allocate_rect(Rect::from_min_size(origin, content), sense);
            if bg.drag_started() {
                self.thumbs_drag = true;
            }
            if bg.drag_stopped() && self.thumbs_drag {
                self.thumbs_drag = false;
                if let Some(p) = bg.interact_pointer_pos().or(ctx.input(|i| i.pointer.latest_pos())) {
                    self.settings.thumbs_dock = nearest_side(self.thumbs_area, p);
                }
            }
            egui::Popup::context_menu(&bg).show(|ui| self.thumbs_menu(ui));
            let (a, b) = if vertical { (view.min.y, view.max.y) } else { (view.min.x, view.max.x) };
            self.thumbs_view = Some((a, b, self.index, place));
            let first = ((a / cell).floor().max(0.0) as usize).min(n);
            let last = ((b / cell).ceil().max(0.0) as usize).min(n);
            let on_screen = self.index..=self.last_on_screen();
            for i in first..last {
                let along = GAP + i as f32 * cell;
                let off = (cross - s) / 2.0;
                let min = if vertical { origin + Vec2::new(off, along) } else { origin + Vec2::new(along, off) };
                let r = Rect::from_min_size(min, Vec2::splat(s));
                let path = self.files[i].clone();
                let resp = ui.interact(r, ui.id().with(("thumb", i)), Sense::click());
                let painter = ui.painter();
                match self.thumbs.get(&path) {
                    Some(Some(tex)) => {
                        let size = tex.size_vec2();
                        let fit = (s / size.x.max(1.0)).min(s / size.y.max(1.0));
                        let img = Rect::from_center_size(r.center(), size * fit);
                        painter.image(
                            tex.id(),
                            img,
                            Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0)),
                            egui::Color32::WHITE,
                        );
                    }
                    Some(None) => {
                        painter.rect_filled(r.shrink(s * 0.3), 0.0, theme::placeholder());
                    }
                    None => {
                        painter.rect_filled(r, 0.0, theme::placeholder());
                        self.thumb_want.push(path.clone());
                    }
                }
                if on_screen.contains(&i) {
                    painter.rect_stroke(r.expand(2.0), 0.0, Stroke::new(2.0, theme::text()), egui::StrokeKind::Outside);
                } else if resp.hovered() {
                    painter.rect_stroke(
                        r.expand(1.0),
                        0.0,
                        Stroke::new(1.0, theme::text_dim()),
                        egui::StrokeKind::Outside,
                    );
                }
                if resp.clicked() && !on_screen.contains(&i) {
                    self.go_to(&ctx, i, i > self.index, false);
                }
                egui::Popup::context_menu(&resp).show(|ui| self.thumbs_menu(ui));
                resp.on_hover_text(format!("{}  ·  {}", i + 1, files::file_name(&path)));
            }
            // A little past both ends, so scrolling finds them ready.
            let ahead: Vec<PathBuf> = (last..(last + 12).min(n))
                .chain(first.saturating_sub(6)..first)
                .map(|i| self.files[i].clone())
                .collect();
            self.thumb_want.extend(ahead);
        });
        if !snap {
            return;
        }
        // The pin, in the corner: keeps the panel on screen.
        let pinned = self.settings.thumbs_pinned;
        let r = Rect::from_min_size(Pos2::new(panel.right() - 28.0, panel.top() + 4.0), Vec2::splat(24.0));
        let resp = ui.interact(r, ui.id().with("thumbs-pin"), Sense::click());
        let painter = ui.painter();
        let fill = if resp.hovered() { theme::hover() } else { theme::bar() };
        painter.rect_filled(r, 0.0, fill);
        let icon = if pinned { &self.icons.pin_on } else { &self.icons.pin };
        let tint = if pinned || resp.hovered() { theme::text() } else { theme::icon() };
        painter.image(icon.id(), r.shrink(4.0), Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0)), tint);
        let tip = if pinned { "Unpin: slide out only at the edge" } else { "Pin: keep it on screen" };
        if resp.on_hover_text(format!("{tip}\nDrag the panel to move it to another side")).clicked() {
            self.settings.thumbs_pinned = !pinned;
        }
    }

    /// Right-click on the panel: where it goes and how big.
    pub fn thumbs_menu(&mut self, ui: &mut egui::Ui) {
        ui.set_min_width(200.0);
        for (dock, name) in [
            (Dock::Left, "Left"),
            (Dock::Right, "Right"),
            (Dock::Top, "Top"),
            (Dock::Bottom, "Bottom"),
            (Dock::Floating, "Floating"),
        ] {
            if ui.add(egui::Button::selectable(self.settings.thumbs_dock == dock, name)).clicked() {
                self.settings.thumbs_dock = dock;
            }
        }
        ui.separator();
        let floating = self.settings.thumbs_dock == Dock::Floating;
        let pin = egui::Button::selectable(self.settings.thumbs_pinned, "Pinned (always on screen)");
        if ui.add_enabled(!floating, pin).clicked() {
            self.settings.thumbs_pinned = !self.settings.thumbs_pinned;
        }
        ui.separator();
        for (size, name) in [(64.0, "Small"), (96.0, "Medium"), (144.0, "Large")] {
            if ui.add(egui::Button::selectable(self.settings.thumb_size == size, name)).clicked() {
                self.settings.thumb_size = size;
            }
        }
        ui.separator();
        if ui.add(egui::Button::new("Hide thumbnails").frame_when_inactive(false)).clicked() {
            self.settings.thumbs_on = false;
            ui.close();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn drops_on_the_nearest_side() {
        let area = Rect::from_min_size(Pos2::new(0.0, 40.0), Vec2::new(1000.0, 600.0));
        assert_eq!(nearest_side(area, Pos2::new(30.0, 300.0)), Dock::Left);
        assert_eq!(nearest_side(area, Pos2::new(960.0, 300.0)), Dock::Right);
        assert_eq!(nearest_side(area, Pos2::new(500.0, 60.0)), Dock::Top);
        assert_eq!(nearest_side(area, Pos2::new(500.0, 620.0)), Dock::Bottom);
        // Dragged past the window still counts.
        assert_eq!(nearest_side(area, Pos2::new(1200.0, 300.0)), Dock::Right);
    }
}
