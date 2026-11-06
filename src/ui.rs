//! Everything around the image: the top bar, the page bar, menus and overlays.

use crate::app::{App, Fit, SLIDESHOW_STEPS, Slot, Start, is_fullscreen};
use crate::files::{self, SortBy};
use crate::keys::{self, Action};
use crate::theme;
use eframe::egui::{
    self, Align, Align2, Color32, FontId, Layout, Pos2, Rect, RichText, Sense, Stroke, TextureHandle, Vec2,
};
use std::time::Duration;

const TOP_BAR_H: f32 = 34.0;
const SHOW_IN_FOLDER: &str = if cfg!(windows) {
    "Show in Explorer"
} else if cfg!(target_os = "macos") {
    "Show in Finder"
} else {
    "Show in folder"
};
const PAGE_BAR_H: f32 = 22.0;
const TOAST_FOR: Duration = Duration::from_millis(2200);

impl App {
    pub fn draw(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        let fullscreen = is_fullscreen(ctx);
        let has_pages = !self.files.is_empty();
        let editing = self.edit.is_some();

        if editing {
            egui::Panel::bottom("edit")
                .exact_size(crate::edit::PANEL_H)
                .show_separator_line(false)
                .frame(egui::Frame::NONE.fill(self.backdrop(ctx)))
                .show(ui, |ui| self.edit_panel(ui));
        }
        if !fullscreen && !editing {
            egui::Panel::top("top")
                .exact_size(TOP_BAR_H)
                .frame(egui::Frame::NONE.fill(theme::bar()).inner_margin(egui::Margin::symmetric(4, 0)))
                .show(ui, |ui| self.top_bar(ui));
            if has_pages && self.settings.show_page_bar {
                egui::Panel::bottom("pages")
                    .exact_size(PAGE_BAR_H)
                    .frame(egui::Frame::NONE.fill(theme::bar()))
                    .show(ui, |ui| self.page_bar(ui));
            }
        }
        // Where the picture goes, between the bars: thumbnails dock to its edges.
        let mut area = ctx.content_rect();
        if !fullscreen && !editing {
            area.min.y += TOP_BAR_H;
            if has_pages && self.settings.show_page_bar {
                area.max.y -= PAGE_BAR_H;
            }
        }
        if !editing {
            self.thumbs(ui, ctx, area);
        }
        egui::CentralPanel::default().frame(egui::Frame::NONE).show(ui, |ui| self.canvas(ui));

        // Fullscreen: the same bars float over the image and get out of the way when the
        // mouse rests, so they never resize the page under you.
        if fullscreen && !editing && self.bars_visible(ctx) {
            let screen = ctx.content_rect();
            egui::Area::new(egui::Id::new("top-overlay"))
                .fixed_pos(screen.left_top())
                .order(egui::Order::Foreground)
                .show(ctx, |ui| {
                    egui::Frame::NONE.fill(theme::bar_overlay()).inner_margin(egui::Margin::symmetric(4, 0)).show(
                        ui,
                        |ui| {
                            ui.set_width(screen.width() - 8.0);
                            ui.set_height(TOP_BAR_H);
                            self.top_bar(ui);
                        },
                    );
                });
            if has_pages && self.settings.show_page_bar {
                egui::Area::new(egui::Id::new("pages-overlay"))
                    .fixed_pos(Pos2::new(screen.left(), screen.bottom() - PAGE_BAR_H))
                    .order(egui::Order::Foreground)
                    .show(ctx, |ui| {
                        egui::Frame::NONE.fill(theme::bar_overlay()).show(ui, |ui| {
                            ui.set_width(screen.width());
                            ui.set_height(PAGE_BAR_H);
                            self.page_bar(ui);
                        });
                    });
            }
        }
        if fullscreen {
            // Wake up again to hide the bars and cursor.
            ctx.request_repaint_after(Duration::from_millis(250));
        }

        if !has_pages {
            self.start_screen(ctx);
        }
        if self.settings.show_info && has_pages {
            self.info_panel(ctx);
        }
        if self.show_help {
            self.help_panel(ctx);
        }
        if self.show_settings {
            self.settings_panel(ctx);
        }
        if self.renaming.is_some() {
            self.rename_box(ctx);
        }
        if self.goto.is_some() {
            self.goto_box(ctx);
        }
        if self.confirm_delete.is_some() {
            self.confirm_delete_box(ctx);
        }
        if self.twins_ask.is_some() {
            self.twins_box(ctx);
        }
        if self.edit.as_ref().is_some_and(|e| e.confirm_leave) {
            self.confirm_leave_box(ctx);
        }
        if fullscreen && has_pages && self.settings.page_number_fullscreen && !self.bars_visible(ctx) {
            let text = format!("{} / {}", self.index + 1, self.files.len());
            let r = ctx.content_rect();
            let painter = ctx.layer_painter(egui::LayerId::new(egui::Order::Foreground, egui::Id::new("page-no")));
            let galley = painter.layout_no_wrap(text, FontId::monospace(13.0), theme::text());
            let at = r.right_bottom() - galley.size() - Vec2::new(16.0, 12.0);
            painter.rect_filled(
                Rect::from_min_size(at, galley.size()).expand2(Vec2::new(8.0, 4.0)),
                0.0,
                theme::bar_overlay(),
            );
            painter.galley(at, galley, theme::text());
        }
        if self.offer_default
            && (self.default_menu || !fullscreen && self.started.elapsed() > std::time::Duration::from_secs(4))
        {
            self.default_offer(ctx);
        }
        self.draw_toast(ctx);

        if ctx.input(|i| !i.raw.hovered_files.is_empty()) {
            let painter = ctx.layer_painter(egui::LayerId::new(egui::Order::Foreground, egui::Id::new("drop")));
            let r = ctx.content_rect();
            painter.rect_filled(r, 0.0, Color32::from_black_alpha(190));
            painter.rect_stroke(r.shrink(12.0), 0.0, Stroke::new(1.0, theme::text_dim()), egui::StrokeKind::Inside);
            painter.text(r.center(), Align2::CENTER_CENTER, "Drop to open", FontId::proportional(22.0), theme::text());
        }
    }

    /// Whether a toolbar button is switched on in Settings.
    fn button_on(&self, key: &str) -> bool {
        self.settings.toolbar.iter().any(|k| k == key)
    }

    fn top_bar(&mut self, ui: &mut egui::Ui) {
        let ctx = ui.ctx().clone();
        let has_pages = !self.files.is_empty();
        let on = |key: &str| self.button_on(key);
        let (nav, rotate, view, thumbs) = (on("nav"), on("rotate"), on("view"), on("thumbs"));
        let (fit_btns, strip, book, enlarge, slideshow) =
            (on("fit"), on("strip"), on("book"), on("enlarge"), on("slideshow"));
        ui.spacing_mut().item_spacing.x = 0.0;
        ui.horizontal_centered(|ui| {
            let open = icon_button(ui, &self.icons.open.clone(), &self.keymap.tip("Open", Action::Open), false, true);
            egui::Popup::menu(&open).show(|ui| self.open_menu(ui));

            if nav {
                separator(ui);
                if icon_button(ui, &self.icons.prev.clone(), "Previous  (←)", false, has_pages).clicked() {
                    self.step(&ctx, -1);
                }
                if icon_button(ui, &self.icons.next.clone(), "Next  (→)", false, has_pages).clicked() {
                    self.step(&ctx, 1);
                }
            }

            if rotate {
                separator(ui);
                let tip = format!("{}\nRight-click: flip", self.keymap.tip("Rotate left", Action::RotateLeft));
                let left = icon_button(ui, &self.icons.rotate_ccw.clone(), &tip, false, has_pages);
                if left.clicked() {
                    self.rotate(false);
                }
                egui::Popup::context_menu(&left).show(|ui| self.turn_menu(ui));
                let tip = format!("{}\nRight-click: flip", self.keymap.tip("Rotate right", Action::RotateRight));
                let right = icon_button(ui, &self.icons.rotate_cw.clone(), &tip, false, has_pages);
                if right.clicked() {
                    self.rotate(true);
                }
                egui::Popup::context_menu(&right).show(|ui| self.turn_menu(ui));
            }

            if view || thumbs || fit_btns || strip || book || enlarge || slideshow {
                separator(ui);
            }
            if view {
                // Shows the layout in use; the menu has every way of showing the pages.
                let icon = if self.strip {
                    self.icons.strip.clone()
                } else if self.spread_on() {
                    self.icons.book.clone()
                } else if self.settings.fit == Fit::Width {
                    self.icons.fit_width.clone()
                } else {
                    self.icons.fit_page.clone()
                };
                let tip = "View: fit, webtoon strip, two pages, thumbnails, slideshow";
                let resp = icon_button(ui, &icon, tip, false, true);
                egui::Popup::menu(&resp).show(|ui| self.view_menu(ui));
            }
            if thumbs {
                let tip = format!("{}\nRight-click: where and how big", self.keymap.tip("Thumbnails", Action::Thumbs));
                let on = self.settings.thumbs_on;
                let resp = icon_button(ui, &self.icons.thumbs.clone(), &tip, on, has_pages);
                if resp.clicked() {
                    self.toggle_thumbs(&ctx);
                }
                egui::Popup::context_menu(&resp).show(|ui| self.thumbs_menu(ui));
            }
            if fit_btns {
                let fit = self.settings.fit;
                let page = !self.strip && fit == Fit::Page && self.zoom.is_none();
                let width = !self.strip && fit == Fit::Width && self.zoom.is_none();
                let tip = self.keymap.tip("Fit to window", Action::FitPage);
                if icon_button(ui, &self.icons.fit_page.clone(), &tip, page, has_pages).clicked() {
                    self.set_fit(Fit::Page);
                }
                let tip = self.keymap.tip("Fit width, scroll down", Action::FitWidth);
                if icon_button(ui, &self.icons.fit_width.clone(), &tip, width, has_pages).clicked() {
                    self.set_fit(Fit::Width);
                }
            }
            if strip {
                let strip_tip = self.keymap.tip("Strip: all pages in one long scroll, for webtoons", Action::Strip);
                if icon_button(ui, &self.icons.strip.clone(), &strip_tip, self.strip, has_pages).clicked() {
                    self.toggle_strip();
                }
            }
            if book {
                let book_tip = format!(
                    "{}\nRight-click: reading direction",
                    self.keymap.tip("Two pages side by side, like a book", Action::Spread)
                );
                let book = icon_button(ui, &self.icons.book.clone(), &book_tip, self.spread_on(), has_pages);
                if book.clicked() {
                    self.toggle_spread();
                }
                egui::Popup::context_menu(&book).show(|ui| self.reading_menu(ui));
            }
            if enlarge {
                let on = self.settings.enlarge_small;
                let tip = self.keymap.tip("Enlarge small images", Action::Enlarge);
                if icon_button(ui, &self.icons.enlarge.clone(), &tip, on, true).clicked() {
                    self.toggle_enlarge();
                }
            }
            if slideshow {
                let playing = self.slideshow.is_some();
                let (icon, label) = if playing {
                    (self.icons.pause.clone(), "Stop slideshow")
                } else {
                    (self.icons.play.clone(), "Slideshow")
                };
                let tip = format!("{}\nRight-click: speed", self.keymap.tip(label, Action::Slideshow));
                let play = icon_button(ui, &icon, &tip, playing, has_pages);
                if play.clicked() {
                    self.toggle_slideshow();
                }
                egui::Popup::context_menu(&play).show(|ui| self.slideshow_menu(ui));
            }

            ui.add_space(10.0);
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                let more = icon_button(ui, &self.icons.more.clone(), "More", false, true);
                egui::Popup::menu(&more).show(|ui| self.actions_menu(ui));
                let settings = self.show_settings;
                let tip = self.keymap.tip("Settings", Action::Settings);
                if icon_button(ui, &self.icons.settings.clone(), &tip, settings, true).clicked() {
                    self.show_settings = !settings;
                }
                if self.button_on("fullscreen") {
                    let fs = is_fullscreen(&ctx);
                    let (icon, tip) = if fs {
                        (self.icons.exit_fullscreen.clone(), self.keymap.tip("Exit fullscreen", Action::Fullscreen))
                    } else {
                        (self.icons.fullscreen.clone(), self.keymap.tip("Fullscreen", Action::Fullscreen))
                    };
                    if icon_button(ui, &icon, &tip, false, true).clicked() {
                        self.toggle_fullscreen(&ctx);
                    }
                }
                if self.button_on("twins") && icon_button(ui, &self.icons.twins.clone(), "Find duplicates", false, true).clicked() {
                    self.ask_twins();
                }
                if self.button_on("frames")
                    && icon_button(ui, &self.icons.frames.clone(), "Pick a frame", false, self.frames_video().is_some()).clicked()
                {
                    self.open_frames(&ctx);
                }
                if self.button_on("info") {
                    let info = self.settings.show_info;
                    let tip = self.keymap.tip("Info", Action::Info);
                    if icon_button(ui, &self.icons.info.clone(), &tip, info, has_pages).clicked() {
                        self.settings.show_info = !info;
                    }
                }
                if self.button_on("bookmark") {
                    let marked = self.bookmark_index(self.index).is_some();
                    let label = if marked { "Remove bookmark" } else { "Bookmark this page, saved as a file" };
                    let bm_tip = format!("{}\nRight-click: all bookmarks", self.keymap.tip(label, Action::Bookmark));
                    let icon = if marked { self.icons.bookmark_on.clone() } else { self.icons.bookmark.clone() };
                    let bm = icon_button(ui, &icon, &bm_tip, false, has_pages);
                    if bm.clicked() {
                        self.toggle_bookmark();
                    }
                    egui::Popup::context_menu(&bm).show(|ui| self.bookmarks_menu(ui));
                }
                let tip = self.keymap.tip("Edit the picture", Action::Edit);
                let can_edit = self.can_edit(self.index);
                if icon_button(ui, &self.icons.edit.clone(), &tip, false, can_edit).clicked() {
                    self.start_edit(&ctx, self.index);
                }
                if has_pages {
                    if self.button_on("counter") {
                        ui.add_space(10.0);
                        let last = self.last_on_screen();
                        let pages = if last != self.index {
                            format!("{}-{}", self.index + 1, last + 1)
                        } else {
                            (self.index + 1).to_string()
                        };
                        let counter = format!("{pages} / {}", self.files.len());
                        let resp = ui.add(
                            egui::Label::new(RichText::new(counter).monospace().color(theme::text_dim()))
                                .sense(Sense::click()),
                        );
                        if resp.on_hover_text(self.keymap.tip("Go to page", Action::GoTo)).clicked() {
                            self.goto = Some(String::new());
                        }
                    }
                    ui.add_space(12.0);
                    self.zoom_control(ui);
                    ui.add_space(12.0);
                    if self.button_on("name") {
                        ui.with_layout(Layout::left_to_right(Align::Center), |ui| {
                            if let Some(p) = self.current() {
                                ui.add(
                                    egui::Label::new(RichText::new(files::file_name(p)).color(theme::text()))
                                        .truncate(),
                                );
                            }
                        });
                    }
                }
            });
        });
    }

    /// Zoom slider, the zoom in percent, and the lock that carries it to the next image.
    /// Laid out right to left (the lock ends up rightmost).
    fn zoom_control(&mut self, ui: &mut egui::Ui) {
        if self.button_on("lock") {
            let locked = self.settings.lock_zoom;
            let (icon, label) = if locked {
                (self.icons.lock.clone(), "Zoom locked: the next images keep it")
            } else {
                (self.icons.unlock.clone(), "Lock the zoom for the next images")
            };
            if icon_button(ui, &icon, &self.keymap.tip(label, Action::LockZoom), locked, true).clicked() {
                self.toggle_lock_zoom();
            }
        }
        let Some(z) = self.effective_zoom else {
            return;
        };
        if !self.button_on("zoom") {
            return;
        }
        ui.add_space(4.0);
        // Fixed width so the toolbar doesn't shift as the number changes.
        let pct = RichText::new(format!("{:>4.0}%", z * 100.0)).monospace().color(theme::text_dim());
        let label = ui.add_sized([44.0, 20.0], egui::Label::new(pct).sense(Sense::click()));
        if self.strip {
            label.on_hover_text("Column width  (+ / −)");
            return;
        }
        let actual = (z - 1.0).abs() < 0.001;
        let tip =
            if actual { "Click: fit".to_owned() } else { self.keymap.tip("Click: actual size", Action::ActualSize) };
        if label.on_hover_text(tip).clicked() {
            if actual {
                self.zoom = None;
            } else {
                self.zoom_request = Some(1.0);
            }
            ui.ctx().request_repaint();
        }
        ui.add_space(6.0);
        let mut value = z;
        let slider = egui::Slider::new(&mut value, 0.05..=8.0)
            .logarithmic(true)
            .show_value(false)
            .trailing_fill(true)
            .handle_shape(egui::style::HandleShape::Rect { aspect_ratio: 0.5 });
        ui.spacing_mut().slider_width = 110.0;
        let resp = ui.add(slider).on_hover_text("Zoom  (Ctrl+wheel, + / −)");
        if resp.changed() {
            self.zoom_request = Some(value.clamp(crate::canvas::MIN_ZOOM, crate::canvas::MAX_ZOOM));
            // The page was drawn before the toolbar this frame; the new zoom shows on the next.
            ui.ctx().request_repaint();
        }
    }

    /// A thin bar showing where you are in the folder; click or drag to jump.
    fn page_bar(&mut self, ui: &mut egui::Ui) {
        let ctx = ui.ctx().clone();
        let rect = ui.max_rect();
        let resp = ui.interact(rect, ui.id().with("page-bar"), Sense::click_and_drag());
        let n = self.files.len();
        if n == 0 {
            return;
        }
        let track = rect.shrink2(Vec2::new(10.0, 0.0));
        // Right to left: the bar fills from the right, like the book.
        let rtl = self.settings.rtl && !self.strip;
        let mirror = |x: f32| {
            if rtl { track.left() + track.right() - x } else { x }
        };
        let index_at = |x: f32| {
            (((mirror(x) - track.left()) / track.width()) * n as f32).floor().clamp(0.0, n as f32 - 1.0) as usize
        };
        let x_of = |i: usize| mirror(track.left() + track.width() * (i as f32 + 0.5) / n as f32);

        if (resp.clicked() || resp.dragged())
            && let Some(p) = resp.interact_pointer_pos()
        {
            let i = index_at(p.x);
            if i != self.index {
                self.go_to(&ctx, i, i > self.index, false);
            }
        }

        let painter = ui.painter_at(rect);
        let y = rect.center().y;
        let hovered = resp.hovered() || resp.dragged();
        let thickness = if hovered { 6.0 } else { 3.0 };
        painter.rect_filled(
            Rect::from_x_y_ranges(track.x_range(), y - thickness / 2.0..=y + thickness / 2.0),
            0.0,
            theme::line(),
        );
        let (a, b) = if rtl { (x_of(self.index), track.right()) } else { (track.left(), x_of(self.index)) };
        let done = Rect::from_x_y_ranges(a..=b, y - thickness / 2.0..=y + thickness / 2.0);
        painter.rect_filled(done, 0.0, theme::progress());
        // Bookmarked pages: a tiny bookmark above the line.
        let mut marked = Vec::new();
        if let Some(dir) = &self.dir {
            for bm in self.settings.bookmarks.iter().filter(|b| b.dir == *dir) {
                if let Some(i) = self.files.iter().position(|f| files::relative_name(dir, f) == bm.page) {
                    let icon = Rect::from_center_size(Pos2::new(x_of(i), rect.top() + 4.5), Vec2::splat(9.0));
                    let uv = Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0));
                    painter.image(self.icons.bookmark_on.id(), icon, uv, theme::text());
                    marked.push(i);
                }
            }
        }
        painter.rect_filled(
            Rect::from_center_size(Pos2::new(x_of(self.index), y), Vec2::new(4.0, 12.0)),
            0.0,
            theme::text(),
        );

        if let Some(p) = resp.hover_pos() {
            let i = index_at(p.x);
            painter.rect_filled(
                Rect::from_center_size(Pos2::new(x_of(i), y), Vec2::new(2.0, 12.0)),
                0.0,
                theme::text_dim(),
            );
            let mut label = format!("{}  ·  {}", i + 1, files::file_name(&self.files[i]));
            if marked.contains(&i) {
                label += "  ·  bookmarked";
            }
            let mut thumb = None;
            if self.settings.bar_preview {
                // The page under the mouse, and its neighbours so sliding along finds them ready.
                let near = (i.saturating_sub(3)..(i + 4).min(n)).map(|k| self.files[k].clone());
                self.thumb_want.insert(0, self.files[i].clone());
                self.thumb_want.extend(near);
                thumb = self.thumbs.get(&self.files[i]).cloned().flatten();
            }
            // At once and while moving, not the usual tooltip that waits for the mouse to rest.
            egui::Tooltip::for_widget(&resp).at_pointer().gap(12.0).show(|ui| {
                if let Some(t) = &thumb {
                    let size = t.size_vec2();
                    let fit = (160.0 / size.x.max(1.0)).min(160.0 / size.y.max(1.0));
                    ui.image((t.id(), size * fit));
                }
                ui.label(label);
            });
        }
    }

    fn open_menu(&mut self, ui: &mut egui::Ui) {
        let ctx = ui.ctx().clone();
        ui.set_min_width(240.0);
        if menu_item(ui, "Open file…", &self.keymap.text(Action::Open)) {
            self.open_file_dialog(&ctx);
        }
        if menu_item(ui, "Open folder…", &self.keymap.text(Action::OpenFolder)) {
            self.open_folder_dialog(&ctx);
        }
        let recent: Vec<_> = self.settings.history.iter().take(10).cloned().collect();
        if !recent.is_empty() {
            ui.separator();
            ui.label(RichText::new("Recent").small().color(theme::text_dim()));
            for (dir, file) in recent {
                let label = files::file_name(&dir);
                let tip = format!("{}\nresumes at {file}", dir.display());
                if ui.add(egui::Button::new(label).frame_when_inactive(false)).on_hover_text(tip).clicked() {
                    self.open_saved(&ctx, dir, Start::Resume);
                }
            }
        }
    }

    /// What an empty window shows: the name, the two ways in, and where you left off.
    fn start_screen(&mut self, ctx: &egui::Context) {
        let screen = ctx.content_rect();
        let top = if is_fullscreen(ctx) { 0.0 } else { TOP_BAR_H };
        let area = Rect::from_min_max(Pos2::new(screen.left(), screen.top() + top), screen.max);
        egui::Area::new(egui::Id::new("start"))
            .fade_in(false)
            .fixed_pos(area.center())
            .pivot(Align2::CENTER_CENTER)
            .order(egui::Order::Middle)
            .show(ctx, |ui| {
                ui.set_width(630.0);
                ui.vertical_centered(|ui| {
                    ui.add(egui::Image::new((self.icons.logo.id(), Vec2::splat(84.0))));
                    ui.add_space(18.0);
                    ui.label(RichText::new(crate::app::APP_NAME).size(45.0).color(theme::text()));
                    ui.add_space(6.0);
                    ui.label(RichText::new("The fastest gallery viewer").size(22.0).color(theme::text()));
                    ui.add_space(4.0);
                    ui.label(RichText::new("Pictures, videos, comics, manga and webtoons").size(16.0).color(theme::text_dim()));
                    ui.add_space(30.0);
                    let ctx = ui.ctx().clone();
                    let button = |text: &str| {
                        egui::Button::new(RichText::new(text).size(15.0))
                            .min_size(Vec2::new(150.0, 36.0))
                            .stroke(Stroke::new(1.0, theme::line()))
                            .fill(theme::bar())
                    };
                    ui.horizontal(|ui| {
                        // Centres the pair of buttons.
                        ui.add_space((ui.available_width() - 2.0 * 150.0 - 10.0).max(0.0) / 2.0);
                        ui.spacing_mut().item_spacing.x = 10.0;
                        if ui
                            .add(button("Open file"))
                            .on_hover_text("Ctrl+O  ·  JPEG, PNG, WebP, GIF, HEIC, AVIF, RAW and more\nVideos: MP4, MKV, WebM, MOV, HEVC, AVI and more\nComics: CBZ, CBR, CB7, PDF, EPUB")
                            .clicked()
                        {
                            self.open_file_dialog(&ctx);
                        }
                        if ui.add(button("Open folder")).on_hover_text("Ctrl+Shift+O").clicked() {
                            self.open_folder_dialog(&ctx);
                        }
                    });
                    ui.add_space(14.0);
                    let hint = "or drop files here  ·  H for shortcuts  ·  Ctrl+, for settings";
                    ui.label(RichText::new(hint).size(15.0).color(theme::text_dim()));
                    if let Some(err) = &self.error {
                        ui.add_space(10.0);
                        ui.label(RichText::new(err).color(theme::error()));
                    }

                    let recent: Vec<_> = self.settings.history.iter().take(6).cloned().collect();
                    if !recent.is_empty() {
                        ui.add_space(40.0);
                        ui.label(RichText::new("Continue").size(21.0).color(theme::text_dim()));
                        ui.add_space(6.0);
                        for (dir, page) in recent {
                            let parent = dir.parent().map(files::file_name).unwrap_or_default();
                            let label = format!("{}   ·   {}", files::file_name(&dir), page);
                            let resp = ui.add(
                                egui::Button::new(RichText::new(label).size(20.0).color(theme::text()))
                                    .frame_when_inactive(false)
                                    .min_size(Vec2::new(630.0, 38.0))
                                    .truncate(),
                            );
                            if resp.on_hover_text(format!("{}\nin {parent}", dir.display())).clicked() {
                                self.open_saved(&ctx, dir, Start::Resume);
                            }
                        }
                    }

                    let marks: Vec<_> = self.settings.bookmarks.iter().take(5).cloned().collect();
                    if !marks.is_empty() {
                        ui.add_space(26.0);
                        ui.label(RichText::new("Bookmarks").size(21.0).color(theme::text_dim()));
                        ui.add_space(6.0);
                        for bm in marks {
                            let label = format!("{}   ·   page {}", files::file_name(&bm.dir), bm.number);
                            let resp = ui.add(
                                egui::Button::new(RichText::new(label).size(20.0).color(theme::text()))
                                    .frame_when_inactive(false)
                                    .min_size(Vec2::new(630.0, 38.0))
                                    .truncate(),
                            );
                            if resp.on_hover_text(format!("{}\n{}", bm.dir.display(), bm.page)).clicked() {
                                self.open_bookmark(&ctx, bm);
                            }
                        }
                    }
                });
            });
    }

    /// The view button: how the pages are shown, and the modes that go with it.
    fn view_menu(&mut self, ui: &mut egui::Ui) {
        ui.set_min_width(270.0);
        let k = |a: Action| self.keymap.text(a);
        let keys = [
            k(Action::FitPage),
            k(Action::FitWidth),
            k(Action::Strip),
            k(Action::Spread),
            k(Action::Rtl),
            k(Action::SplitWide),
            k(Action::Enlarge),
            k(Action::LockZoom),
            k(Action::Thumbs),
            k(Action::Slideshow),
        ];
        let (strip, fit) = (self.strip, self.settings.fit);
        if toggle_item(ui, !strip && fit == Fit::Page, "Fit to window", &keys[0]) {
            self.set_fit(Fit::Page);
        }
        if toggle_item(ui, !strip && fit == Fit::Width, "Fit the width, scroll down", &keys[1]) {
            self.set_fit(Fit::Width);
        }
        if toggle_item(ui, strip, "Webtoon strip", &keys[2]) {
            self.toggle_strip();
        }
        ui.separator();
        if toggle_item(ui, self.spread_on(), "Two pages side by side", &keys[3]) {
            self.toggle_spread();
        }
        if toggle_item(ui, self.settings.rtl, "Right to left (manga)", &keys[4]) {
            self.toggle_rtl();
        }
        if toggle_item(ui, self.settings.split_wide, "Split wide pages in two", &keys[5]) {
            self.settings.split_wide = !self.settings.split_wide;
            self.half = 0;
        }
        ui.separator();
        if toggle_item(ui, self.settings.enlarge_small, "Enlarge small images", &keys[6]) {
            self.toggle_enlarge();
        }
        if toggle_item(ui, self.settings.lock_zoom, "Keep the zoom for the next images", &keys[7]) {
            self.toggle_lock_zoom();
        }
        ui.separator();
        if toggle_item(ui, self.settings.thumbs_on, "Thumbnails", &keys[8]) {
            self.toggle_thumbs(&ui.ctx().clone());
        }
        if toggle_item(ui, self.slideshow.is_some(), "Slideshow", &keys[9]) {
            self.toggle_slideshow();
        }
        ui.menu_button("Slideshow speed", |ui| self.slideshow_menu(ui));
    }

    /// Book layout and reading direction.
    fn reading_menu(&mut self, ui: &mut egui::Ui) {
        ui.set_min_width(250.0);
        let spread = self.spread_on();
        let key = dim(&self.keymap.text(Action::Spread));
        if ui.add(egui::Button::selectable(spread, "Two pages side by side").shortcut_text(key)).clicked() {
            self.toggle_spread();
        }
        if ui
            .add(
                egui::Button::selectable(self.settings.rtl, "Right to left (manga)")
                    .shortcut_text(dim(&self.keymap.text(Action::Rtl))),
            )
            .clicked()
        {
            self.toggle_rtl();
        }
        ui.add_enabled_ui(spread, |ui| {
            let ctx = ui.ctx().clone();
            if menu_item(ui, "Shift pairs by one page", &self.keymap.text(Action::StepOn)) && !self.files.is_empty() {
                let i = (self.index + 1).min(self.files.len() - 1);
                self.go_to(&ctx, i, true, false);
            }
        });
        let split = self.settings.split_wide;
        let key = dim(&self.keymap.text(Action::SplitWide));
        if ui.add(egui::Button::selectable(split, "Split wide pages in two").shortcut_text(key)).clicked() {
            self.settings.split_wide = !split;
            self.half = 0;
        }
        ui.separator();
        let zones = self.settings.click_zones;
        let tip = "Clicking the left or right third of the page turns it";
        if ui.add(egui::Button::selectable(zones, "Click the sides to turn pages")).on_hover_text(tip).clicked() {
            self.settings.click_zones = !zones;
        }
    }

    /// Rotate and flip, on screen until saved.
    fn turn_menu(&mut self, ui: &mut egui::Ui) {
        let ctx = ui.ctx().clone();
        ui.set_min_width(250.0);
        if menu_item(ui, "Rotate right", &self.keymap.text(Action::RotateRight)) {
            self.rotate(true);
        }
        if menu_item(ui, "Rotate left", &self.keymap.text(Action::RotateLeft)) {
            self.rotate(false);
        }
        if menu_item(ui, "Flip left to right", &self.keymap.text(Action::FlipH)) {
            self.flip(false);
        }
        if menu_item(ui, "Flip upside down", &self.keymap.text(Action::FlipV)) {
            self.flip(true);
        }
        let turned = self.current_rotation() != 0;
        let real = self.current_file().is_some();
        ui.separator();
        if ui.add_enabled(turned, egui::Button::new("Undo").frame_when_inactive(false)).clicked()
            && let Some(p) = self.current().cloned()
        {
            self.rotation.remove(&p);
        }
        let key = self.keymap.text(Action::SaveTurn);
        if ui.add_enabled(turned && real, menu_button("Save to the file", &key)).clicked() {
            self.save_rotation(&ctx);
        }
    }

    /// The folders and archives next to the open one, to jump between chapters.
    fn chapters_menu(&mut self, ui: &mut egui::Ui) {
        let ctx = ui.ctx().clone();
        ui.set_min_width(280.0);
        let list = self.chapter_list();
        if list.is_empty() {
            ui.label(dim("No other folders here"));
            return;
        }
        egui::ScrollArea::vertical().max_height(420.0).show(ui, |ui| {
            for chapter in list {
                let here = self.dir.as_ref() == Some(&chapter);
                let resp = ui.add(egui::Button::selectable(here, files::file_name(&chapter)).truncate());
                if resp.clicked() && !here {
                    self.open_folder(&ctx, chapter, Start::Resume);
                    ui.close();
                }
            }
        });
    }

    fn bookmarks_menu(&mut self, ui: &mut egui::Ui) {
        let ctx = ui.ctx().clone();
        ui.set_min_width(300.0);
        let marked = self.bookmark_index(self.index).is_some();
        let label = if marked { "Remove this bookmark" } else { "Bookmark this page" };
        let key = self.keymap.text(Action::Bookmark);
        if ui.add_enabled(!self.files.is_empty(), menu_button(label, &key)).clicked() {
            self.toggle_bookmark();
        }
        let key = self.keymap.text(Action::SaveBookmark);
        let save = menu_button("Save as a file…", &key);
        let tip = "A small file you can keep anywhere: open it to continue from this page";
        if ui.add_enabled(!self.files.is_empty(), save).on_hover_text(tip).clicked() {
            self.save_bookmark_file();
            ui.close();
        }
        if self.settings.bookmarks.is_empty() {
            ui.label(RichText::new("No bookmarks yet").color(theme::text_dim()));
            return;
        }
        ui.separator();
        let mut remove = None;
        egui::ScrollArea::vertical().max_height(360.0).show(ui, |ui| {
            for (i, bm) in self.settings.bookmarks.clone().into_iter().enumerate() {
                ui.horizontal(|ui| {
                    let label = format!("{}  ·  page {}", files::file_name(&bm.dir), bm.number);
                    let open = ui.add(
                        egui::Button::new(label).frame_when_inactive(false).min_size(Vec2::new(250.0, 0.0)).truncate(),
                    );
                    if open.on_hover_text(format!("{}\n{}", bm.dir.display(), bm.page)).clicked() {
                        self.open_bookmark(&ctx, bm.clone());
                        ui.close();
                    }
                    if ui.add(egui::Button::new("×").frame_when_inactive(false)).on_hover_text("Remove").clicked() {
                        remove = Some(i);
                    }
                });
            }
        });
        if let Some(i) = remove {
            self.settings.bookmarks.remove(i);
        }
    }

    fn slideshow_menu(&mut self, ui: &mut egui::Ui) {
        ui.label(RichText::new("Seconds per image").small().color(theme::text_dim()));
        for &s in SLIDESHOW_STEPS {
            let selected = self.settings.slideshow_secs == s;
            if ui.add(egui::Button::selectable(selected, format!("{s} s"))).clicked() {
                self.set_slideshow_secs(s);
            }
        }
    }

    /// The "more" menu, also on right-click over the image.
    pub fn actions_menu(&mut self, ui: &mut egui::Ui) {
        let ctx = ui.ctx().clone();
        ui.set_min_width(250.0);
        let has_pages = !self.files.is_empty();
        ui.add_enabled_ui(has_pages, |ui| {
            if menu_item(ui, "Copy image", &self.keymap.text(Action::CopyImage)) {
                self.copy_image(&ctx);
            }
            if menu_item(ui, "Copy path", &self.keymap.text(Action::CopyPath)) {
                self.copy_path(&ctx);
            }
            if menu_item(ui, "Copy the file", &self.keymap.text(Action::CopyFile)) {
                self.copy_file();
            }
            if menu_item(ui, SHOW_IN_FOLDER, &self.keymap.text(Action::ShowInFolder)) {
                self.show_in_explorer();
            }
            ui.menu_button("Set as", |ui| {
                ui.set_min_width(200.0);
                if menu_item(ui, "Desktop background", "") {
                    self.set_wallpaper(&ctx);
                    ui.close();
                }
                if cfg!(windows) && menu_item(ui, "Lock screen", "") {
                    self.set_lock_screen(&ctx);
                    ui.close();
                }
            });
            ui.separator();
            // Changing the file itself: only for real files, not pages inside archives.
            let real = self.current_file().is_some();
            ui.add_enabled_ui(real, |ui| {
                if menu_item(ui, "Rename…", &self.keymap.text(Action::Rename)) {
                    self.start_rename();
                }
                let turned = self.current_rotation() != 0;
                let key = self.keymap.text(Action::SaveTurn);
                if ui.add_enabled(turned, menu_button("Save rotation / flip to file", &key)).clicked() {
                    self.save_rotation(&ctx);
                }
                if menu_item(ui, "Delete", &self.keymap.text(Action::Delete)) {
                    self.delete_current(&ctx);
                }
            });
            ui.menu_button("More file actions", |ui| {
                if menu_item(ui, "Copy to folder…", "") {
                    self.copy_or_move_to(&ctx, false);
                }
                ui.add_enabled_ui(real, |ui| {
                    if menu_item(ui, "Move to folder…", "") {
                        self.copy_or_move_to(&ctx, true);
                    }
                    // Windows' own shell verbs.
                    if cfg!(windows) {
                        ui.separator();
                        if menu_item(ui, "Open with…", "") {
                            self.open_with();
                        }
                        if menu_item(ui, "Edit", "") {
                            self.shell_verb("edit");
                        }
                        if menu_item(ui, "Print…", "") {
                            self.shell_verb("print");
                        }
                    }
                });
                if cfg!(windows) {
                    ui.add_enabled_ui(real, |ui| {
                        if menu_item(ui, "Properties", "") {
                            self.shell_verb("properties");
                        }
                    });
                }
            });
            ui.separator();
            ui.menu_button("Rotate and flip", |ui| self.turn_menu(ui));
            let can_edit = self.can_edit(self.index);
            if ui.add_enabled(can_edit, menu_button("Edit the picture…", &self.keymap.text(Action::Edit))).clicked() {
                self.start_edit(&ctx, self.index);
            }
            if self.anim_state().is_some() {
                let paused = self.anim_pause.is_some();
                let key = self.keymap.text(Action::Pause);
                if menu_item(ui, if paused { "Play animation" } else { "Pause animation" }, &key) {
                    self.toggle_pause();
                }
            }
            if menu_item(ui, "Go to page…", &self.keymap.text(Action::GoTo)) {
                self.goto = Some(String::new());
            }
            if menu_item(ui, "Reload", &self.keymap.text(Action::Reload)) {
                self.reload(&ctx);
            }
        });
        ui.add_enabled_ui(has_pages, |ui| {
            ui.menu_button("Chapters", |ui| self.chapters_menu(ui));
        });
        ui.menu_button("Bookmarks", |ui| self.bookmarks_menu(ui));
        if menu_item(ui, "Find duplicates…", "") {
            self.ask_twins();
        }
        if self.frames_video().is_some() && menu_item(ui, "Pick a frame…", "") {
            self.open_frames(&ctx);
        }
        ui.menu_button("Sort by", |ui| {
            let mut changed = false;
            let sorts: &[(SortBy, &str)] = &[
                (SortBy::Name, "Name"),
                (SortBy::Modified, "Date modified"),
                (SortBy::Created, "Date created"),
                (SortBy::Size, "Size"),
                (SortBy::Type, "Type"),
                (SortBy::Random, "Random"),
            ];
            for &(sort, label) in sorts {
                if ui.add(egui::Button::selectable(self.settings.sort == sort, label)).clicked() {
                    self.settings.sort = sort;
                    changed = true;
                }
            }
            ui.separator();
            if ui.add(egui::Button::selectable(self.settings.sort_desc, "Reverse order")).clicked() {
                self.settings.sort_desc = !self.settings.sort_desc;
                changed = true;
            }
            if changed {
                self.resort(&ctx);
            }
        });
        ui.separator();
        if menu_item(ui, "Paste", &self.keymap.text(Action::Paste)) {
            self.paste(&ctx);
        }
        if menu_item(ui, "Settings…", &self.keymap.text(Action::Settings)) {
            self.show_settings = true;
        }
        if menu_item(ui, "Keyboard shortcuts", &self.keymap.text(Action::Help)) {
            self.show_help = true;
        }
        // macOS takes that from the app bundle instead.
        if !cfg!(target_os = "macos") && menu_item(ui, "Make default…", "") {
            self.offer_default = true;
            self.default_menu = true;
        }
    }

    fn info_panel(&mut self, ctx: &egui::Context) {
        let Some(path) = self.current().cloned() else {
            return;
        };
        // Read from disk once per page, not every frame (it can be a slow external drive).
        if self.info_meta.as_ref().is_none_or(|m| m.path != path) {
            self.info_meta = Some(crate::app::InfoMeta {
                path: path.clone(),
                meta: std::fs::metadata(&path).ok(),
                exif: files::read_exif(&path),
            });
        }
        let path = &path;
        let screen = ctx.content_rect();
        let top = if is_fullscreen(ctx) { 8.0 } else { TOP_BAR_H + 8.0 };
        egui::Area::new(egui::Id::new("info"))
            .fixed_pos(Pos2::new(screen.right() - 8.0, screen.top() + top))
            .pivot(Align2::RIGHT_TOP)
            .order(egui::Order::Foreground)
            .interactable(false)
            .show(ctx, |ui| {
                panel_frame().show(ui, |ui| {
                    ui.set_max_width(340.0);
                    let meta = self.info_meta.as_ref().and_then(|m| m.meta.clone());
                    let exif = self.info_meta.as_ref().map(|m| m.exif.clone()).unwrap_or_default();
                    let mut row = |k: &str, v: String| {
                        ui.horizontal(|ui| {
                            ui.add_sized([72.0, 16.0], egui::Label::new(RichText::new(k).color(theme::text_dim())));
                            ui.add(egui::Label::new(v).wrap());
                        });
                    };
                    row("File", files::file_name(path));
                    row("Page", format!("{} of {}", self.index + 1, self.files.len()));
                    if let Some(Slot::Ready(pic)) = self.cache.get(path) {
                        let [w, h] = pic.original_size;
                        let mp = (w as f64 * h as f64) / 1e6;
                        row("Size", format!("{w} × {h}  ({mp:.1} MP)"));
                        let frames = pic.frames.len();
                        let kind =
                            if frames > 1 { format!("{}, {frames} frames", pic.format) } else { pic.format.clone() };
                        row("Format", kind);
                    }
                    if let Some((frame, n, paused)) = self.anim_state() {
                        let state = if paused { "  ·  paused" } else { "" };
                        row("Frame", format!("{} / {n}{state}", frame + 1));
                    }
                    if let Some(m) = &meta {
                        row("On disk", files::format_size(m.len()));
                        if let Ok(t) = m.modified() {
                            row("Modified", files::format_time(t));
                        }
                    }
                    if let Some(z) = self.effective_zoom {
                        row("Zoom", format!("{:.0}%", z * 100.0));
                    }
                    let turns = self.current_rotation();
                    if turns != 0 {
                        let deg = (turns & 3) as u32 * 90;
                        let text = match (deg, turns & 4 != 0) {
                            (0, true) => "flipped".to_owned(),
                            (d, true) => format!("flipped, then {d}°"),
                            (d, false) => format!("{d}°"),
                        };
                        row("Turned", format!("{text}  ·  not saved"));
                    }
                    for (k, v) in exif {
                        row(k, v);
                    }
                    if let Some(dir) = &self.dir {
                        row("Folder", dir.display().to_string());
                    }
                    let ready = self.window().iter().filter(|&&i| self.ready(i).is_some()).count();
                    row("Preloaded", format!("{ready} of {}", self.window().len()));
                });
            });
    }

    fn help_panel(&mut self, ctx: &egui::Context) {
        // The keys that can't be changed, then every shortcut from the table (as set now).
        const FIXED: &[(&str, &str)] = &[
            ("→  ←", "Next / previous image (look around when zoomed in)"),
            ("↓  ↑", "Scroll a little, then next"),
            ("Space  Shift+Space", "Scroll down / up, then next"),
            ("Wheel", "Scroll, then next image"),
            ("Ctrl+wheel", "Zoom (strip: column width)"),
            ("Drag", "Pan"),
            ("Click left / right", "Previous / next"),
            ("Double-click", "Fullscreen"),
            ("Mouse back / forward", "Previous / next image"),
            ("Bottom bar", "Click or drag to jump"),
            ("Esc", "Close / leave fullscreen / stop slideshow"),
            ("Ctrl +  Ctrl −", "Interface size"),
        ];
        let mut rows: Vec<(String, &str)> = FIXED.iter().map(|(k, v)| ((*k).to_owned(), *v)).collect();
        for d in &keys::DEFS {
            let k = self.keymap.text(d.action);
            if !k.is_empty() {
                rows.push((k, d.label));
            }
        }
        let half = rows.len().div_ceil(2);
        let screen = ctx.content_rect();
        let mut open = true;
        let mut change = false;
        // Hung from near the top: a centred Area only gets the space below its centre.
        let top = if is_fullscreen(ctx) { 20.0 } else { TOP_BAR_H + 20.0 };
        egui::Area::new(egui::Id::new("help"))
            .fixed_pos(Pos2::new(screen.center().x, screen.top() + top))
            .pivot(Align2::CENTER_TOP)
            .order(egui::Order::Foreground)
            .show(ctx, |ui| {
                // Opaque: it sits over the picture and must stay readable.
                let frame = egui::Frame::NONE
                    .fill(theme::bar())
                    .stroke(Stroke::new(1.0, theme::line()))
                    .inner_margin(egui::Margin::same(18));
                frame.show(ui, |ui| {
                    ui.horizontal(|ui| {
                        ui.label(RichText::new("Shortcuts").strong());
                        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                            if ui.add(egui::Button::new("Close").frame_when_inactive(false)).clicked() {
                                open = false;
                            }
                            if ui.add(egui::Button::new(dim("Change…")).frame_when_inactive(false)).clicked() {
                                change = true;
                            }
                        });
                    });
                    ui.add_space(6.0);
                    let h = screen.height() - top - 110.0;
                    egui::ScrollArea::vertical().max_height(h).min_scrolled_height(h).show(ui, |ui| {
                        ui.horizontal_top(|ui| {
                            for (n, part) in rows.chunks(half.max(1)).enumerate() {
                                if n > 0 {
                                    ui.add_space(28.0);
                                }
                                egui::Grid::new(("keys", n)).num_columns(2).spacing([18.0, 5.0]).show(ui, |ui| {
                                    for (k, what) in part {
                                        ui.label(RichText::new(k).monospace().color(theme::text()));
                                        ui.label(RichText::new(*what).color(theme::text_dim()));
                                        ui.end_row();
                                    }
                                });
                            }
                        });
                    });
                });
            });
        if !open {
            self.show_help = false;
        }
        if change {
            self.show_help = false;
            self.show_settings = true;
        }
    }

    /// Everything that can be switched: the categories down the left, one at a time on the right.
    fn settings_panel(&mut self, ctx: &egui::Context) {
        use crate::app::SettingsPage as P;
        const SIDEBAR: f32 = 200.0;
        const PAGES: &[(P, &str)] = &[
            (P::Viewing, "Viewing"),
            (P::Browsing, "Folders and order"),
            (P::Comics, "Comics"),
            (P::Thumbnails, "Thumbnails"),
            (P::Slideshow, "Slideshow"),
            (P::Look, "Look"),
            (P::Window, "Window"),
            (P::Files, "Files"),
            (P::Performance, "Performance"),
            (P::Toolbar, "Toolbar"),
            (P::Shortcuts, "Shortcuts"),
        ];
        let screen = ctx.content_rect();
        let mut open = true;
        let mut resort = false;
        // Fixed size and opaque: it sits over whatever is open and must stay readable.
        let size = Vec2::new(860.0f32.min(screen.width() - 40.0), (screen.height() - 90.0).clamp(240.0, 640.0));
        egui::Area::new(egui::Id::new("settings"))
            .fixed_pos(screen.center() - size / 2.0 + Vec2::new(0.0, 12.0))
            .order(egui::Order::Foreground)
            .show(ctx, |ui| {
                let frame = egui::Frame::NONE.fill(theme::bar()).stroke(Stroke::new(1.0, theme::line()));
                frame.show(ui, |ui| {
                    ui.set_width(size.x);
                    ui.set_height(size.y);
                    ui.spacing_mut().item_spacing = Vec2::new(8.0, 6.0);
                    ui.horizontal_top(|ui| {
                        // Categories.
                        // Each column its own top-down layout (the row around them is sideways).
                        let column = Layout::top_down_justified(Align::Min);
                        let list = egui::Frame::NONE.fill(theme::bg()).inner_margin(egui::Margin::symmetric(10, 18));
                        ui.allocate_ui_with_layout(Vec2::new(SIDEBAR, size.y), column, |ui| {
                            list.show(ui, |ui| {
                                ui.set_width(SIDEBAR - 20.0);
                                ui.set_height(size.y - 36.0);
                                ui.label(RichText::new("Settings").size(18.0).strong());
                                ui.add_space(10.0);
                                ui.spacing_mut().item_spacing.y = 2.0;
                                for &(page, name) in PAGES {
                                    let on = self.settings_page == page;
                                    let text = if on { RichText::new(name).color(theme::text()) } else { dim(name) };
                                    let button = egui::Button::selectable(on, text).min_size(Vec2::new(0.0, 30.0));
                                    if ui.add(button).clicked() {
                                        self.settings_page = page;
                                        self.capturing = None;
                                    }
                                }
                                ui.with_layout(Layout::bottom_up(Align::Min), |ui| {
                                    let reset = egui::Button::new(dim("Reset all")).frame_when_inactive(false);
                                    if ui
                                        .add(reset)
                                        .on_hover_text("Back to the defaults (keeps history and bookmarks)")
                                        .clicked()
                                    {
                                        self.reset_settings(ui.ctx());
                                        resort = true;
                                    }
                                });
                            })
                        });
                        // The open category.
                        let pane =
                            egui::Frame::NONE.inner_margin(egui::Margin { left: 18, right: 18, top: 18, bottom: 18 });
                        let column = Layout::top_down(Align::Min);
                        ui.allocate_ui_with_layout(Vec2::new(size.x - SIDEBAR, size.y), column, |ui| {
                            pane.show(ui, |ui| {
                                ui.set_width(size.x - SIDEBAR - 36.0);
                                ui.set_height(size.y - 36.0);
                                ui.horizontal(|ui| {
                                    let name = PAGES.iter().find(|p| p.0 == self.settings_page).map_or("", |p| p.1);
                                    ui.label(RichText::new(name).size(18.0));
                                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                                        if ui.add(egui::Button::new("Close").frame_when_inactive(false)).clicked() {
                                            open = false;
                                        }
                                    });
                                });
                                ui.add_space(8.0);
                                egui::ScrollArea::vertical()
                                    .id_salt(("settings-page", self.settings_page as u8))
                                    .auto_shrink([false, false])
                                    .show(ui, |ui| {
                                        ui.spacing_mut().item_spacing.y = 8.0;
                                        resort |= self.settings_body(ui);
                                    });
                            })
                        });
                    });
                });
            });
        if resort {
            self.resort(ctx);
        }
        if !open {
            self.show_settings = false;
            self.capturing = None;
        }
    }

    /// The open category's settings. Returns true when the sort order changed.
    fn settings_body(&mut self, ui: &mut egui::Ui) -> bool {
        use crate::app::{AtEnd, Backdrop, Dock, SettingsPage as P};
        let ctx = ui.ctx().clone();
        let mut resort = false;
        match self.settings_page {
            P::Viewing => {
                let mut fit = self.settings.fit;
                choice(ui, "Fit", &mut fit, &[(Fit::Page, "Whole image"), (Fit::Width, "Fill the width")]);
                if fit != self.settings.fit {
                    self.set_fit(fit);
                }
                choice(
                    ui,
                    "Zoom step",
                    &mut self.settings.zoom_step,
                    &[(1.1, "Fine (10%)"), (1.25, "Normal (25%)"), (1.5, "Big (50%)")],
                );
                let k = |a| self.keymap.text(a);
                let (enlarge, lock) = (k(Action::Enlarge), k(Action::LockZoom));
                check(ui, &mut self.settings.enlarge_small, "Enlarge small images to fit", &enlarge);
                check(ui, &mut self.settings.lock_zoom, "Keep the zoom for the next images", &lock);
                check(ui, &mut self.settings.wheel_turns, "Mouse wheel turns pages (instead of zooming)", "");
                check(ui, &mut self.settings.glide, "Glide from one image to the next", "");
                ui.add_enabled_ui(self.settings.glide, |ui| {
                    ui.horizontal(|ui| {
                        row_label(ui, "Glide time");
                        ui.add(
                            egui::Slider::new(&mut self.settings.glide_ms, 100..=2000)
                                .logarithmic(true)
                                .suffix(" ms")
                                .trailing_fill(true)
                                .handle_shape(egui::style::HandleShape::Rect { aspect_ratio: 0.5 }),
                        );
                    });
                });
                check(ui, &mut self.settings.click_zones, "Click the left / right side to go back / on", "");
                check(ui, &mut self.settings.arrows_pan, "Arrow keys look around a zoomed-in image first", "");
                let sharp = self.settings.sharp_pixels;
                check(ui, &mut self.settings.sharp_pixels, "Sharp pixels when zoomed in (pixel art, screenshots)", "");
                if sharp != self.settings.sharp_pixels {
                    self.reupload();
                }
            }
            P::Browsing => {
                let mut at_end = self.settings.at_end;
                choice(
                    ui,
                    "At the last image",
                    &mut at_end,
                    &[
                        (AtEnd::NextFolderOnSecondPush, "Next folder on a 2nd push"),
                        (AtEnd::NextFolder, "Next folder straight away"),
                        (AtEnd::Loop, "Start over"),
                        (AtEnd::Stop, "Stop"),
                    ],
                );
                if at_end != self.settings.at_end {
                    self.settings.at_end = at_end;
                    self.hop = None;
                }
                check(ui, &mut self.settings.resume, "Reopen folders where I left off", "");
                ui.add_space(6.0);
                let mut sort = self.settings.sort;
                choice(
                    ui,
                    "Sort by",
                    &mut sort,
                    &[
                        (SortBy::Name, "Name"),
                        (SortBy::Modified, "Date modified"),
                        (SortBy::Created, "Date created"),
                        (SortBy::Size, "Size"),
                        (SortBy::Type, "Type"),
                        (SortBy::Random, "Random"),
                    ],
                );
                let mut desc = self.settings.sort_desc;
                check(ui, &mut desc, "Reverse order", "");
                if sort != self.settings.sort || desc != self.settings.sort_desc {
                    self.settings.sort = sort;
                    self.settings.sort_desc = desc;
                    resort = true;
                }
            }
            P::Comics => {
                let (mut spread, mut rtl) = (self.settings.spread, self.settings.rtl);
                check(ui, &mut spread, "Two pages side by side", &self.keymap.text(Action::Spread));
                check(ui, &mut rtl, "Right to left (manga)", &self.keymap.text(Action::Rtl));
                if spread != self.settings.spread {
                    self.toggle_spread();
                }
                if rtl != self.settings.rtl {
                    self.toggle_rtl();
                }
                let split = self.keymap.text(Action::SplitWide);
                check(
                    ui,
                    &mut self.settings.split_wide,
                    "Split wide pages: read a double-page scan a half at a time",
                    &split,
                );
            }
            P::Thumbnails => {
                let key = self.keymap.text(Action::Thumbs);
                check(ui, &mut self.settings.thumbs_on, "Show thumbnails of the folder", &key);
                ui.add_enabled_ui(self.settings.thumbs_on, |ui| {
                    ui.add_enabled_ui(self.settings.thumbs_dock != Dock::Floating, |ui| {
                        check(
                            ui,
                            &mut self.settings.thumbs_pinned,
                            "Keep it on screen (pinned); otherwise it slides out at its edge",
                            "",
                        );
                    });
                    choice(
                        ui,
                        "Place",
                        &mut self.settings.thumbs_dock,
                        &[
                            (Dock::Left, "Left"),
                            (Dock::Right, "Right"),
                            (Dock::Top, "Top"),
                            (Dock::Bottom, "Bottom"),
                            (Dock::Floating, "Floating (drag it anywhere)"),
                        ],
                    );
                    choice(
                        ui,
                        "Size",
                        &mut self.settings.thumb_size,
                        &[(64.0, "Small"), (96.0, "Medium"), (144.0, "Large")],
                    );
                });
                check(ui, &mut self.settings.bar_preview, "Preview the page under the mouse on the position bar", "");
            }
            P::Slideshow => {
                ui.horizontal_wrapped(|ui| {
                    row_label(ui, "Seconds per image");
                    for &secs in SLIDESHOW_STEPS {
                        let selected = self.settings.slideshow_secs == secs;
                        if ui.add(egui::Button::selectable(selected, format!("{secs}"))).clicked() {
                            self.set_slideshow_secs(secs);
                        }
                    }
                });
                check(ui, &mut self.settings.slideshow_stops, "Stop at the last image (instead of starting over)", "");
            }
            P::Look => {
                let mut pref = self.settings.theme;
                let system = if cfg!(windows) { "Like Windows" } else { "Like the system" };
                use crate::theme::ThemePref;
                choice(
                    ui,
                    "Theme",
                    &mut pref,
                    &[(ThemePref::System, system), (ThemePref::Dark, "Dark"), (ThemePref::Light, "Light")],
                );
                if pref != self.settings.theme {
                    self.set_theme(&ctx, pref);
                }
                ui.horizontal_wrapped(|ui| {
                    row_label(ui, "Interface size");
                    for scale in [1.0, 1.25, 1.5, 1.75, 2.0] {
                        let selected = (ctx.zoom_factor() - scale).abs() < 0.01;
                        if ui.add(egui::Button::selectable(selected, format!("{:.0}%", scale * 100.0))).clicked() {
                            ctx.set_zoom_factor(scale);
                        }
                    }
                });
                choice(
                    ui,
                    "Background",
                    &mut self.settings.backdrop,
                    &[
                        (Backdrop::Theme, "Theme"),
                        (Backdrop::Black, "Black"),
                        (Backdrop::Gray, "Grey"),
                        (Backdrop::White, "White"),
                    ],
                );
                check(ui, &mut self.settings.checker, "Checkerboard behind see-through images", "");
                check(ui, &mut self.settings.high_contrast, "High contrast", "");
                check(ui, &mut self.settings.reduce_motion, "Reduce motion", "");
                check(ui, &mut self.settings.show_page_bar, "Show the position bar at the bottom", "");
                check(ui, &mut self.settings.show_info, "Show the info panel", &self.keymap.text(Action::Info));
            }
            P::Window => {
                check(ui, &mut self.settings.single_instance, "Open files in this window when it's already open", "");
                if !cfg!(target_os = "macos") {
                    ui.horizontal(|ui| {
                        check(ui, &mut self.settings.ask_default, "Now and then, offer to be the default viewer", "");
                        if ui.button("Make default…").clicked() {
                            self.offer_default = true;
                            self.default_menu = true;
                        }
                    });
                }
                check(ui, &mut self.settings.start_fullscreen, "Start in fullscreen", "");
                check(ui, &mut self.settings.esc_closes, "Esc closes the window when nothing else is open", "");
                check(ui, &mut self.settings.keep_bars, "Keep the bars in fullscreen", "");
                ui.add_enabled_ui(!self.settings.keep_bars, |ui| {
                    choice(
                        ui,
                        "Hide the bars after",
                        &mut self.settings.bars_hide_secs,
                        &[(1.0, "1 s"), (1.8, "2 s"), (3.0, "3 s"), (5.0, "5 s")],
                    );
                });
                check(ui, &mut self.settings.page_number_fullscreen, "Page number in the corner in fullscreen", "");
            }
            P::Files => {
                check(ui, &mut self.settings.confirm_delete, "Ask before deleting", &self.keymap.text(Action::Delete));
                ui.horizontal(|ui| {
                    let n = self.settings.history.len();
                    if ui.add_enabled(n > 0, egui::Button::new("Clear the Continue list")).clicked() {
                        self.settings.history.clear();
                        self.toast("Continue list cleared");
                    }
                    ui.label(dim(&format!("{n} remembered")));
                });
            }
            P::Performance => {
                choice(
                    ui,
                    "Images kept ready ahead",
                    &mut self.settings.preload,
                    &[(1, "1"), (2, "2"), (4, "4"), (8, "8"), (16, "16")],
                );
                ui.label(dim("More: smoother fast flipping, more memory. Half as many are kept behind."));
                let threads = (self.settings.decode_threads, self.settings.thumb_threads);
                choice(
                    ui,
                    "Decoding threads",
                    &mut self.settings.decode_threads,
                    &[(0, "Auto"), (1, "1"), (2, "2"), (4, "4"), (6, "6"), (8, "8")],
                );
                choice(ui, "Thumbnail threads", &mut self.settings.thumb_threads, &[(1, "1"), (2, "2"), (4, "4")]);
                ui.label(dim("Fewer threads leave more of the computer to other programs."));
                if threads != (self.settings.decode_threads, self.settings.thumb_threads) {
                    self.restart_loaders(&ctx);
                }
                ui.add_space(6.0);
                let ready = self.window().iter().filter(|&&i| self.ready(i).is_some()).count();
                let thumbs = self.thumbs.values().filter(|t| t.is_some()).count();
                ui.label(dim(&format!("Now: {ready} images ready, {thumbs} thumbnails in memory")));
            }
            P::Toolbar => {
                ui.columns(2, |cols| {
                    for (i, (key, label)) in TOOLBAR_BUTTONS.iter().enumerate() {
                        let ui = &mut cols[i % 2];
                        let mut shown = self.button_on(key);
                        if ui.checkbox(&mut shown, *label).changed() {
                            self.settings.toolbar.retain(|k| k != key);
                            if shown {
                                self.settings.toolbar.push((*key).to_owned());
                            }
                        }
                    }
                });
            }
            P::Shortcuts => self.shortcut_settings(ui),
        }
        resort
    }

    /// Every rebindable shortcut: Set replaces the keys, + adds one more.
    fn shortcut_settings(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            ui.label(dim("Arrows, Space, Esc and the mouse stay as they are."));
            let changed = !self.settings.keys.is_empty();
            if ui.add_enabled(changed, egui::Button::new("Reset all shortcuts")).clicked() {
                self.settings.keys.clear();
                self.capturing = None;
                self.keymap = keys::Keymap::new(&self.settings.keys);
            }
        });
        egui::Grid::new("shortcuts").num_columns(3).spacing([14.0, 4.0]).striped(false).show(ui, |ui| {
            for d in &keys::DEFS {
                ui.label(d.label);
                let waiting = self.capturing.is_some_and(|(a, _)| a == d.action);
                let text = self.keymap.text(d.action);
                let shown = if waiting {
                    RichText::new("press a key…  (Esc cancels)").color(theme::text())
                } else if text.is_empty() {
                    dim("none")
                } else {
                    RichText::new(text).monospace()
                };
                ui.label(shown);
                ui.horizontal(|ui| {
                    if ui.small_button("Set").on_hover_text("Replace with the next key you press").clicked() {
                        self.capturing = Some((d.action, false));
                    }
                    if ui.small_button("+").on_hover_text("Add another key").clicked() {
                        self.capturing = Some((d.action, true));
                    }
                    let changed = self.settings.keys.contains_key(d.id);
                    if changed && ui.small_button("Default").clicked() {
                        self.settings.keys.remove(d.id);
                        self.keymap = keys::Keymap::new(&self.settings.keys);
                    }
                    if !self.keymap.get(d.action).is_empty() && ui.small_button("×").on_hover_text("No key").clicked()
                    {
                        keys::set(&mut self.settings.keys, d.action, &[]);
                        self.keymap = keys::Keymap::new(&self.settings.keys);
                    }
                });
                ui.end_row();
            }
        });
    }

    fn ask_twins(&mut self) {
        self.twins_ask = Some(self.dialog_dir().map(|d| d.display().to_string()).unwrap_or_default());
    }

    /// Which folder and how alike, before the duplicate finder starts.
    fn twins_box(&mut self, ctx: &egui::Context) {
        let (mut browse, mut scan, mut cancel) = (false, false, false);
        egui::Area::new(egui::Id::new("twins-ask"))
            .fixed_pos(ctx.content_rect().center())
            .pivot(Align2::CENTER_CENTER)
            .order(egui::Order::Foreground)
            .show(ctx, |ui| {
                panel_frame().fill(theme::bar()).inner_margin(egui::Margin::same(16)).show(ui, |ui| {
                    ui.label(RichText::new("Find duplicates").strong());
                    ui.add_space(6.0);
                    let Some(folder) = self.twins_ask.as_mut() else {
                        return;
                    };
                    ui.horizontal(|ui| {
                        ui.add(egui::TextEdit::singleline(folder).hint_text("Folder").desired_width(380.0));
                        browse = ui.button("Browse…").clicked();
                    });
                    ui.add_space(4.0);
                    ui.checkbox(&mut self.settings.twins_subfolders, "Include subfolders");
                    ui.add_space(10.0);
                    let mut percent = self.settings.twins_threshold * 100.0;
                    ui.horizontal(|ui| {
                        row_label(ui, "Alike at least");
                        ui.add(egui::Slider::new(&mut percent, 50.0..=100.0).step_by(1.0).fixed_decimals(0).suffix("%"));
                    });
                    self.settings.twins_threshold = percent / 100.0;
                    ui.add_space(10.0);
                    let exists = std::path::Path::new(folder.trim()).is_dir();
                    ui.horizontal(|ui| {
                        scan = ui.add_enabled(exists, egui::Button::new("Scan  (Enter)")).clicked()
                            || exists && ui.input(|i| i.key_pressed(egui::Key::Enter));
                        cancel = ui.button("Cancel  (Esc)").clicked() || ui.input(|i| i.key_pressed(egui::Key::Escape));
                    });
                });
            });
        if browse {
            let mut dialog = rfd::FileDialog::new();
            if let Some(dir) = self.dialog_dir() {
                dialog = dialog.set_directory(dir);
            }
            if let Some(p) = dialog.pick_folder() {
                self.twins_ask = Some(p.display().to_string());
            }
        } else if cancel {
            self.twins_ask = None;
        } else if scan && let Some(folder) = self.twins_ask.take() {
            let mut options = crate::twins::Options::new(folder.trim());
            options.subfolders = self.settings.twins_subfolders;
            options.is_image = files::is_image;
            options.decode = crate::loader::decode_for_twins;
            self.twins = Some(crate::twins::Panel::new(options, self.settings.twins_threshold, crate::actions::recycle));
            // Nothing carries on unseen behind it.
            self.slideshow = None;
            if let Some(v) = &mut self.video {
                v.pause();
            }
        }
    }

    /// The video on screen, when it's a real file rather than a page inside an archive.
    fn frames_video(&self) -> Option<std::path::PathBuf> {
        self.current_file().filter(|p| crate::video::is_video(p))
    }

    fn open_frames(&mut self, ctx: &egui::Context) {
        let Some(path) = self.frames_video() else { return };
        self.slideshow = None;
        let mut at = None;
        if let Some(v) = self.video.as_mut().filter(|v| v.path == path) {
            v.pause();
            at = Some(v.position());
        }
        self.frames = Some(crate::frames::Picker::new(ctx, path, at));
    }

    /// The frame picker, over the whole window.
    pub fn draw_frames(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        let Some(picker) = &mut self.frames else { return };
        if picker.show(ui) {
            let saved = picker.saved_any();
            self.frames = None;
            // The new picture may be in the open folder.
            if saved {
                self.resort(ctx);
            }
        }
    }

    /// The duplicate finder, over the whole window.
    pub fn draw_twins(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        let Some(panel) = &mut self.twins else { return };
        let frame = egui::Frame::NONE.fill(ui.visuals().panel_fill);
        match egui::CentralPanel::default().frame(frame).show(ui, |ui| panel.show(ui)).inner {
            Some(crate::twins::Event::Deleted(paths)) => self.forget_files(ctx, &paths),
            Some(crate::twins::Event::Closed) => {
                self.settings.twins_threshold = panel.threshold();
                self.twins = None;
            }
            None => {}
        }
        self.draw_toast(ctx);
    }

    fn confirm_delete_box(&mut self, ctx: &egui::Context) {
        let Some(path) = self.confirm_delete.clone() else { return };
        let screen = ctx.content_rect();
        let mut answer = None;
        egui::Area::new(egui::Id::new("confirm-delete"))
            .fixed_pos(screen.center())
            .pivot(Align2::CENTER_CENTER)
            .order(egui::Order::Foreground)
            .show(ctx, |ui| {
                let frame = egui::Frame::NONE
                    .fill(theme::bar())
                    .stroke(Stroke::new(1.0, theme::line()))
                    .inner_margin(egui::Margin::same(18));
                frame.show(ui, |ui| {
                    ui.label(RichText::new(format!("Delete {}?", files::file_name(&path))).size(16.0));
                    ui.label(dim(if cfg!(windows) { "It goes to the Recycle Bin." } else { "It goes to the Trash." }));
                    ui.add_space(10.0);
                    ui.horizontal(|ui| {
                        if ui.button("Delete  (Enter)").clicked() {
                            answer = Some(true);
                        }
                        if ui.button("Keep  (Esc)").clicked() {
                            answer = Some(false);
                        }
                    });
                });
            });
        match answer {
            Some(true) => {
                self.confirm_delete = None;
                self.delete_now(ctx, path);
            }
            Some(false) => self.confirm_delete = None,
            None => {}
        }
    }

    fn confirm_leave_box(&mut self, ctx: &egui::Context) {
        let Some(e) = &mut self.edit else { return };
        let mut throw_away = false;
        egui::Area::new(egui::Id::new("confirm-leave"))
            .fixed_pos(ctx.content_rect().center())
            .pivot(Align2::CENTER_CENTER)
            .order(egui::Order::Foreground)
            .show(ctx, |ui| {
                let frame = egui::Frame::NONE
                    .fill(theme::bar())
                    .stroke(Stroke::new(1.0, theme::line()))
                    .inner_margin(egui::Margin::same(18));
                frame.show(ui, |ui| {
                    ui.label(RichText::new("Throw away your changes?").size(16.0));
                    ui.label(dim(&format!("{} stays as it was.", files::file_name(&e.path))));
                    ui.add_space(10.0);
                    ui.horizontal(|ui| {
                        throw_away = ui.button("Throw away  (Enter)").clicked();
                        if ui.button("Keep editing  (Esc)").clicked() {
                            e.keep_editing();
                        }
                    });
                });
            });
        if throw_away {
            self.discard_edit(ctx);
        }
    }

    fn rename_box(&mut self, ctx: &egui::Context) {
        let screen = ctx.content_rect();
        let mut done: Option<bool> = None;
        egui::Area::new(egui::Id::new("rename"))
            .fixed_pos(screen.center())
            .pivot(Align2::CENTER_CENTER)
            .order(egui::Order::Foreground)
            .show(ctx, |ui| {
                panel_frame().inner_margin(egui::Margin::same(16)).show(ui, |ui| {
                    ui.label(RichText::new("Rename").strong());
                    ui.add_space(6.0);
                    let Some(text) = self.renaming.as_mut() else {
                        return;
                    };
                    let stem = text[..text.rfind('.').unwrap_or(text.len())].chars().count();
                    let output = egui::TextEdit::singleline(text).desired_width(380.0).show(ui);
                    if !output.response.has_focus() && !output.response.lost_focus() {
                        // First frame: focus, and select the name without its extension, like Explorer.
                        output.response.request_focus();
                        let mut state = output.state;
                        let range =
                            egui::text::CCursorRange::two(egui::text::CCursor::new(0), egui::text::CCursor::new(stem));
                        state.cursor.set_char_range(Some(range));
                        state.store(ui.ctx(), output.response.id);
                    }
                    if ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                        done = Some(true);
                    }
                    if ui.input(|i| i.key_pressed(egui::Key::Escape)) {
                        done = Some(false);
                    }
                    ui.add_space(8.0);
                    ui.horizontal(|ui| {
                        if ui.button("Rename").clicked() {
                            done = Some(true);
                        }
                        if ui.button("Cancel").clicked() {
                            done = Some(false);
                        }
                    });
                });
            });
        match done {
            Some(true) => {
                let name = self.renaming.clone().unwrap_or_default();
                self.finish_rename(ctx, &name);
            }
            Some(false) => self.renaming = None,
            None => {}
        }
    }

    fn goto_box(&mut self, ctx: &egui::Context) {
        let screen = ctx.content_rect();
        let total = self.files.len();
        let mut done: Option<bool> = None;
        egui::Area::new(egui::Id::new("goto"))
            .fixed_pos(screen.center())
            .pivot(Align2::CENTER_CENTER)
            .order(egui::Order::Foreground)
            .show(ctx, |ui| {
                panel_frame().inner_margin(egui::Margin::same(16)).show(ui, |ui| {
                    ui.label(RichText::new(format!("Go to page  (1 to {total})")).strong());
                    ui.add_space(6.0);
                    let Some(text) = self.goto.as_mut() else {
                        return;
                    };
                    let resp = ui.add(egui::TextEdit::singleline(text).desired_width(160.0));
                    if !resp.has_focus() && !resp.lost_focus() {
                        resp.request_focus();
                    }
                    text.retain(|c| c.is_ascii_digit());
                    if ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                        done = Some(true);
                    }
                    if ui.input(|i| i.key_pressed(egui::Key::Escape)) {
                        done = Some(false);
                    }
                    ui.add_space(8.0);
                    ui.horizontal(|ui| {
                        if ui.button("Go").clicked() {
                            done = Some(true);
                        }
                        if ui.button("Cancel").clicked() {
                            done = Some(false);
                        }
                    });
                });
            });
        match done {
            Some(true) => self.finish_goto(ctx),
            Some(false) => self.goto = None,
            None => {}
        }
    }

    /// A small card in the corner: which kinds of files TelescopeView should open. Comes up
    /// from the menu, or by itself once a week at most while it isn't the default.
    fn default_offer(&mut self, ctx: &egui::Context) {
        let screen = ctx.content_rect();
        let bottom = if self.files.is_empty() || !self.settings.show_page_bar { 16.0 } else { PAGE_BAR_H + 16.0 };
        let menu = self.default_menu;
        let mut answer = None;
        egui::Area::new(egui::Id::new("default-offer"))
            .fixed_pos(Pos2::new(screen.right() - 16.0, screen.bottom() - bottom))
            .pivot(Align2::RIGHT_BOTTOM)
            // Above the video controls, which share that corner.
            .order(egui::Order::Tooltip)
            .show(ctx, |ui| {
                egui::Frame::NONE
                    .fill(theme::bar())
                    .stroke(Stroke::new(1.0, theme::line()))
                    .inner_margin(egui::Margin::same(16))
                    .show(ui, |ui| {
                        ui.set_width(360.0);
                        let title = if menu { "Open with TelescopeView" } else { "Open your pictures and videos with TelescopeView?" };
                        ui.label(RichText::new(title).size(15.0).color(theme::text()));
                        ui.add_space(8.0);
                        let off = &mut self.settings.default_off;
                        for (name, exts) in crate::system::groups() {
                            let out = exts.iter().filter(|e| off.iter().any(|o| o == *e)).count();
                            let mut on = out < exts.len();
                            let sample = exts.iter().take(4).map(|e| e.to_uppercase()).collect::<Vec<_>>().join(", ");
                            let check = egui::Checkbox::new(&mut on, name).indeterminate(out > 0 && out < exts.len());
                            if ui.add(check).on_hover_text(format!("{sample}…  ({} formats)", exts.len())).changed() {
                                off.retain(|o| !exts.contains(&o.as_str()));
                                if !on {
                                    off.extend(exts.iter().map(|e| e.to_string()));
                                }
                            }
                        }
                        // One format at a time, for keeping, say, PNGs in another app.
                        let label = if self.default_formats { "Hide formats" } else { "Pick formats…" };
                        if ui.add(egui::Button::new(dim(label)).frame_when_inactive(false)).clicked() {
                            self.default_formats = !self.default_formats;
                        }
                        if self.default_formats {
                            egui::ScrollArea::vertical().max_height(180.0).show(ui, |ui| {
                                ui.horizontal_wrapped(|ui| {
                                    ui.spacing_mut().item_spacing = Vec2::new(4.0, 4.0);
                                    for (_, exts) in crate::system::groups() {
                                        for e in exts {
                                            let on = !off.iter().any(|o| o == e);
                                            if ui.selectable_label(on, e).clicked() {
                                                if on {
                                                    off.push(e.to_owned());
                                                } else {
                                                    off.retain(|o| o != e);
                                                }
                                            }
                                        }
                                    }
                                });
                            });
                        }
                        ui.add_space(6.0);
                        let how = if cfg!(windows) {
                            "Unticked types stay with the app that opens them now. Windows then shows its Default apps page: pick TelescopeView for each type listed (apps aren't allowed to do it themselves)."
                        } else {
                            "Unticked types stay with the app that opens them now."
                        };
                        ui.label(dim(how));
                        ui.add_space(12.0);
                        ui.horizontal(|ui| {
                            ui.spacing_mut().item_spacing.x = 8.0;
                            let main = egui::Button::new(RichText::new("Make default").color(theme::text()))
                                .fill(theme::selected())
                                .min_size(Vec2::new(0.0, 28.0));
                            if ui.add(main).clicked() {
                                answer = Some(1);
                            }
                            let later = if menu { "Cancel" } else { "Not now" };
                            if ui.add(egui::Button::new(later).min_size(Vec2::new(0.0, 28.0))).clicked() {
                                answer = Some(0);
                            }
                            if !menu {
                                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                                    let never = egui::Button::new(dim("Don't ask again")).frame_when_inactive(false);
                                    if ui.add(never).clicked() {
                                        answer = Some(2);
                                    }
                                });
                            }
                        });
                    });
            });
        if ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
            answer = answer.or(Some(0));
        }
        if let Some(a) = answer {
            self.offer_default = false;
            self.default_menu = false;
            if !menu {
                self.settings.default_asked_at = crate::app::now_secs();
            }
            match a {
                1 => self.make_default(),
                2 => self.settings.ask_default = false,
                _ => {}
            }
        }
    }

    fn draw_toast(&mut self, ctx: &egui::Context) {
        let Some((msg, at)) = &self.toast else { return };
        let age = at.elapsed();
        if age > TOAST_FOR {
            self.toast = None;
            return;
        }
        ctx.request_repaint_after(TOAST_FOR - age);
        let screen = ctx.content_rect();
        let bottom = if is_fullscreen(ctx) { 40.0 } else { PAGE_BAR_H + 24.0 };
        let painter = ctx.layer_painter(egui::LayerId::new(egui::Order::Tooltip, egui::Id::new("toast")));
        let galley = painter.layout_no_wrap(msg.clone(), FontId::proportional(14.0), theme::text());
        let rect = Rect::from_center_size(
            Pos2::new(screen.center().x, screen.bottom() - bottom - galley.size().y / 2.0),
            galley.size() + Vec2::new(28.0, 14.0),
        );
        painter.rect(rect, 0.0, theme::bar_overlay(), Stroke::new(1.0, theme::line()), egui::StrokeKind::Inside);
        painter.galley(rect.center() - galley.size() / 2.0, galley, theme::text());
    }
}

/// Toolbar buttons that Settings can show or hide: key stored in the settings, label.
/// Open, Settings and the ... menu always stay.
pub const TOOLBAR_BUTTONS: &[(&str, &str)] = &[
    ("nav", "Previous / next"),
    ("rotate", "Rotate"),
    ("view", "View menu (fit, strip, book...)"),
    ("thumbs", "Thumbnails"),
    ("fit", "Fit to window / width"),
    ("strip", "Strip (webtoons)"),
    ("book", "Two pages (book)"),
    ("enlarge", "Enlarge small images"),
    ("slideshow", "Slideshow"),
    ("name", "File name"),
    ("zoom", "Zoom slider"),
    ("lock", "Zoom lock"),
    ("counter", "Page number"),
    ("bookmark", "Bookmark"),
    ("info", "Info"),
    ("twins", "Find duplicates"),
    ("frames", "Pick a frame"),
    ("fullscreen", "Fullscreen"),
];

/// What a new toolbar shows: the everyday buttons. The rest sit in the view and ... menus.
pub const DEFAULT_TOOLBAR: &[&str] = &["nav", "rotate", "view", "name", "zoom", "counter", "bookmark", "fullscreen"];

/// A menu entry that is on or off, with its shortcut.
fn toggle_item(ui: &mut egui::Ui, on: bool, label: &str, shortcut: &str) -> bool {
    ui.add(egui::Button::selectable(on, label).shortcut_text(dim(shortcut))).clicked()
}

/// A setting switch with its key, if it has one.
fn check(ui: &mut egui::Ui, value: &mut bool, label: &str, key: &str) {
    ui.horizontal(|ui| {
        ui.checkbox(value, label);
        if !key.is_empty() {
            ui.label(RichText::new(key).monospace().color(theme::text_dim()));
        }
    });
}

/// A row's name, the same height as the buttons after it so they line up.
fn row_label(ui: &mut egui::Ui, text: &str) {
    // A label sits at the row's top; the buttons have padding above their text.
    let pad = ui.spacing().button_padding.y;
    ui.vertical(|ui| {
        ui.add_space(pad);
        ui.label(dim(text));
    });
}

/// One of a few options, as a row of square buttons.
fn choice<T: PartialEq + Copy>(ui: &mut egui::Ui, label: &str, value: &mut T, options: &[(T, &str)]) {
    ui.horizontal_wrapped(|ui| {
        if !label.is_empty() {
            row_label(ui, label);
        }
        for (option, text) in options {
            if ui.add(egui::Button::selectable(*value == *option, *text)).clicked() {
                *value = *option;
            }
        }
    });
}

fn panel_frame() -> egui::Frame {
    egui::Frame::NONE
        .fill(theme::bar_overlay())
        .stroke(Stroke::new(1.0, theme::line()))
        .inner_margin(egui::Margin::same(12))
}

/// A square icon button: no frame until hovered, a filled square when selected.
pub fn icon_button(
    ui: &mut egui::Ui,
    icon: &TextureHandle,
    tooltip: &str,
    selected: bool,
    enabled: bool,
) -> egui::Response {
    let size = Vec2::new(34.0, TOP_BAR_H);
    let sense = if enabled { Sense::click() } else { Sense::hover() };
    let (rect, resp) = ui.allocate_exact_size(size, sense);
    let painter = ui.painter();
    let hovered = enabled && resp.hovered();
    if selected && enabled {
        painter.rect_filled(rect, 0.0, theme::selected());
    } else if hovered {
        painter.rect_filled(rect, 0.0, theme::hover());
    }
    let tint = match (enabled, selected || hovered) {
        (false, _) => theme::icon_disabled(),
        (true, true) => theme::text(),
        (true, false) => theme::icon(),
    };
    let icon_rect = Rect::from_center_size(rect.center(), Vec2::splat(18.0));
    painter.image(icon.id(), icon_rect, Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0)), tint);
    // Screen readers get the tooltip's first line as the button's name.
    let name = tooltip.lines().next().unwrap_or(tooltip).to_owned();
    resp.widget_info(|| {
        let mut info = egui::WidgetInfo::labeled(egui::WidgetType::Button, enabled, &name);
        info.selected = Some(selected);
        info
    });
    if enabled { resp.on_hover_text(tooltip) } else { resp }
}

pub fn separator(ui: &mut egui::Ui) {
    let (rect, _) = ui.allocate_exact_size(Vec2::new(13.0, TOP_BAR_H), Sense::hover());
    let x = rect.center().x.round() + 0.5;
    ui.painter().vline(x, rect.center().y - 9.0..=rect.center().y + 9.0, Stroke::new(1.0, theme::line()));
}

fn dim(text: &str) -> RichText {
    RichText::new(text).color(theme::text_dim())
}

fn menu_button<'a>(label: &'a str, shortcut: &'a str) -> egui::Button<'a> {
    egui::Button::new(label).shortcut_text(RichText::new(shortcut).color(theme::text_dim())).frame_when_inactive(false)
}

fn menu_item(ui: &mut egui::Ui, label: &str, shortcut: &str) -> bool {
    ui.add(menu_button(label, shortcut)).clicked()
}
