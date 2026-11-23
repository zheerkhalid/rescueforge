//! The Results tab: scan progress, filters, folder tree, file list or
//! thumbnail grid, the preview panel and the selection bar.

use super::app::{App, Tab};
use super::state::{scroll_into_view, ChanceFilter, Col, DateFilter, Session, SizeFilter};
use super::theme::{pal, regular, semibold, Icon, Palette};
use super::widgets;
use crate::model::{Category, Chance, Status};
use crate::util::{fmt_count, fmt_duration, fmt_size, fmt_time};
use eframe::egui::{
    self, pos2, vec2, Align, Align2, Color32, CornerRadius, FontFamily, FontId, Frame, Layout,
    Margin, Rect, RichText, Sense, Stroke, StrokeKind, TextureHandle, Ui,
};

pub(super) fn chance_color(p: &Palette, c: Chance) -> Color32 {
    match c {
        Chance::Good => p.good,
        Chance::Partial => p.warn,
        Chance::Poor => p.danger,
        Chance::Unknown => p.muted,
    }
}

pub(super) fn category_look(p: &Palette, c: Category) -> (Icon, Color32) {
    match c {
        Category::Pictures => (Icon::PICTURE, p.info),
        Category::Video => (Icon::VIDEO, Color32::from_rgb(0xa7, 0x8b, 0xfa)),
        Category::Audio => (Icon::AUDIO, p.good),
        Category::Documents => (Icon::DOCUMENT, p.accent),
        Category::Archives => (Icon::ARCHIVE, p.warn),
        Category::Other => (Icon::OTHER, p.muted),
    }
}

fn mono(size: f32) -> FontId {
    FontId::new(size, FontFamily::Monospace)
}

/// Paints a texture inside `rect`, keeping its proportions.
fn paint_fitted(ui: &Ui, rect: Rect, tex: &TextureHandle) {
    let size = tex.size_vec2();
    if size.x <= 0.0 || size.y <= 0.0 {
        return;
    }
    let scale = (rect.width() / size.x).min(rect.height() / size.y);
    let target = Rect::from_center_size(rect.center(), size * scale);
    ui.painter().image(
        tex.id(),
        target,
        Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)),
        Color32::WHITE,
    );
}

/// Height of one line of the file list and of one row of tiles.
pub(super) const LIST_ROW: f32 = 36.0;
pub(super) const TILE_ROW: f32 = 162.0;

/// What was chosen in the right-click menu of a file.
enum RowAction {
    Recover(u32),
    Preview(u32),
    Toggle(u32),
    TickType(String),
    TickFolder(u32),
    Copy(String),
}

/// The right-click menu of a file in the list or the grid.
fn row_menu(ui: &mut Ui, ses: &Session, i: u32) -> Option<RowAction> {
    let f = &ses.files[i as usize];
    let ext = f.ext();
    let mut action = None;
    ui.set_min_width(230.0);
    if ui
        .add_enabled(
            ses.source.is_some(),
            egui::Button::new(Icon::RECOVER.with("Recover this file...")),
        )
        .clicked()
    {
        action = Some(RowAction::Recover(i));
    }
    if ui
        .button(Icon::VIEW.with("Open the large preview"))
        .clicked()
    {
        action = Some(RowAction::Preview(i));
    }
    ui.separator();
    let tick = if ses.checked[i as usize] {
        "Untick this file"
    } else {
        "Tick this file"
    };
    if ui.button(tick).clicked() {
        action = Some(RowAction::Toggle(i));
    }
    if !ext.is_empty() && ui.button(format!("Tick all .{ext} files shown")).clicked() {
        action = Some(RowAction::TickType(ext));
    }
    if ui.button("Tick everything in its folder").clicked() {
        action = Some(RowAction::TickFolder(ses.dir_of[i as usize]));
    }
    ui.separator();
    if ui.button(Icon::COPY.with("Copy the name")).clicked() {
        action = Some(RowAction::Copy(f.name.clone()));
    }
    if ui.button("Copy the original path").clicked() {
        action = Some(RowAction::Copy(crate::fs::join(&f.path, &f.name)));
    }
    if action.is_some() {
        ui.close_menu();
    }
    action
}

struct Columns {
    name: (f32, f32),
    size: (f32, f32),
    modified: Option<(f32, f32)>,
    chance: (f32, f32),
    folder: Option<(f32, f32)>,
}

impl Columns {
    /// Column positions (left edge, width) for a table of this width.
    fn for_width(left: f32, width: f32) -> Columns {
        let check = 40.0;
        let size = 84.0;
        let chance = 150.0;
        let modified = if width >= 600.0 { 132.0 } else { 0.0 };
        let folder = if width >= 860.0 {
            ((width - 860.0) * 0.5 + 190.0).min(320.0)
        } else {
            0.0
        };
        let name = width - check - size - chance - modified - folder - 8.0;
        let mut x = left + check;
        let mut take = |w: f32| {
            let r = (x, w);
            x += w;
            r
        };
        let name = take(name);
        let size = take(size);
        let modified = (modified > 0.0).then(|| take(modified));
        let chance = take(chance);
        let folder = (folder > 0.0).then(|| take(folder));
        Columns {
            name,
            size,
            modified,
            chance,
            folder,
        }
    }
}

impl App {
    pub(super) fn results_ui(&mut self, ui: &mut Ui) {
        let p = pal(ui);
        let Some(mut ses) = self.ses.take() else {
            Self::page_header(ui, "Results", "Nothing has been scanned yet", |_| {});
            widgets::empty_state(
                ui,
                Icon::SCAN,
                p.accent,
                "No scan yet",
                "Choose a drive on the \"Recover files\" tab and start a quick scan. What it finds is listed here.",
            );
            let mut go = false;
            ui.vertical_centered(|ui| {
                if widgets::primary_button(ui, "Choose a drive", true).clicked() {
                    go = true;
                }
            });
            if go {
                self.tab = Tab::Recover;
            }
            return;
        };
        let scanning = ses.scanning() || self.fake_scan.is_some();
        let mode = if ses.options.deep {
            "Deep scan"
        } else {
            "Quick scan"
        };
        let state = if scanning {
            "in progress".to_string()
        } else if ses.stopped {
            "stopped early".to_string()
        } else {
            match ses.elapsed_secs {
                Some(0) => "finished in under a second".to_string(),
                Some(s) => format!("finished in {}", fmt_duration(s)),
                None => "opened from a saved session".to_string(),
            }
        };
        let subtitle = format!(
            "{mode} {state} · {} files found · {} shown",
            fmt_count(ses.files.len() as u64),
            fmt_count(ses.visible.len() as u64)
        );
        let (mut save, mut new_scan, mut export, mut resume) = (false, false, false, false);
        Self::page_header(ui, &ses.label, &subtitle, |ui| {
            if widgets::ghost_button(ui, &Icon::SCAN.with("New scan")).clicked() {
                new_scan = true;
            }
            // With the preview panel open there is only room for two labelled buttons.
            let export_tip = "Save the list as a table (CSV) for a spreadsheet: the ticked files, or everything shown (Ctrl+E)";
            if ui.available_width() < 250.0 {
                if widgets::icon_button(ui, Icon::LIST, export_tip).clicked() {
                    export = !ses.files.is_empty();
                }
            } else if widgets::ghost_button_enabled(
                ui,
                &Icon::LIST.with("Export list"),
                !ses.files.is_empty(),
            )
            .on_hover_text(export_tip)
            .clicked()
            {
                export = true;
            }
            if widgets::ghost_button(ui, &Icon::SAVE.with("Save scan")).on_hover_text("Save the list to a session file and open it later without scanning again (Ctrl+S)").clicked() {
                save = true;
            }
        });
        if scanning {
            self.progress_card(ui, &mut ses);
            ui.add_space(2.0);
        } else if let Some(point) = ses.resume.as_ref().filter(|_| ses.stopped) {
            // A deep scan that was stopped early can go on where it was.
            let real = ses
                .source
                .as_ref()
                .map(|s| s.len())
                .unwrap_or(ses.source_len);
            let done = if real == 0 {
                0.0
            } else {
                (point.pos as f64 / real as f64).clamp(0.0, 1.0)
            };
            let left = ((1.0 - done) * ses.source_len as f64) as u64;
            let ready = ses.source.is_some();
            widgets::card(ui, |ui| {
                ui.allocate_ui_with_layout(
                    vec2(ui.available_width(), 34.0),
                    Layout::left_to_right(Align::Center),
                    |ui| {
                        ui.label(RichText::new(Icon::PAUSE.glyph()).size(16.0).color(p.accent));
                        ui.label(
                            RichText::new(format!("The deep scan was stopped at {:.0}%", done * 100.0))
                                .font(semibold(15.0)),
                        );
                        if ui.available_width() > 420.0 {
                            ui.label(
                                RichText::new(format!("{} still to read", fmt_size(left)))
                                    .color(p.muted),
                            );
                        }
                        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                            if widgets::primary_button(ui, &Icon::PLAY.with("Continue scan"), ready)
                                .on_hover_text("Go on where the scan was stopped. The files listed so far, and your ticks, are kept.")
                                .on_disabled_hover_text("Connect the drive this scan was made of and open the session again.")
                                .clicked()
                            {
                                resume = true;
                            }
                        });
                    },
                );
            });
            ui.add_space(2.0);
        }
        if !ses.source_note.is_empty() {
            widgets::notice(ui, Icon::WARNING, p.warn, &ses.source_note);
            ui.add_space(2.0);
        }
        self.filter_rows(ui, &mut ses);
        ui.add_space(2.0);

        let height = ui.available_height();
        let tree_w = 236.0;
        let mut action: Option<RowAction> = None;
        ui.horizontal_top(|ui| {
            ui.spacing_mut().item_spacing.x = 12.0;
            ui.allocate_ui_with_layout(vec2(tree_w, height), Layout::top_down(Align::Min), |ui| {
                self.tree_card(ui, &mut ses, height);
            });
            let rest = ui.available_width();
            ui.allocate_ui_with_layout(vec2(rest, height), Layout::top_down(Align::Min), |ui| {
                action = if ses.grid {
                    self.grid_card(ui, &mut ses, height)
                } else {
                    self.table_card(ui, &mut ses, height)
                };
            });
        });
        let mut recover_one = None;
        match action {
            Some(RowAction::Recover(i)) => recover_one = Some(i),
            Some(RowAction::Preview(i)) => {
                ses.highlighted = Some(i);
                self.big_preview = true;
            }
            Some(RowAction::Toggle(i)) => ses.toggle(i),
            Some(RowAction::TickType(ext)) => ses.set_ext(&ext, true),
            Some(RowAction::TickFolder(dir)) => ses.set_folder(dir, true),
            Some(RowAction::Copy(text)) => ui.ctx().copy_text(text),
            None => {}
        }

        // Keep the preview in step with the highlighted row.
        if let Some(h) = ses.highlighted {
            if let Some(f) = ses.files.get(h as usize) {
                self.previewer.want_full(h, f, ses.source.as_ref());
            }
        } else {
            self.previewer.full = None;
        }
        self.ses = Some(ses);
        if save {
            self.save_session();
        }
        if export {
            self.export_list();
        }
        if resume {
            self.continue_scan(ui.ctx());
        }
        if let Some(i) = recover_one {
            self.open_recover_dialog(Some(i));
        }
        if new_scan {
            self.tab = Tab::Recover;
        }
    }

    fn progress_card(&mut self, ui: &mut Ui, ses: &mut Session) {
        let p = pal(ui);
        let (fraction, phase, detail, paused) = match (&self.fake_scan, &ses.scan) {
            (Some(f), _) => (f.fraction, f.phase.clone(), f.detail.clone(), false),
            (None, Some(h)) => {
                let paused = h.ctl.is_paused();
                (
                    h.progress.fraction(),
                    if paused {
                        "Paused".to_string()
                    } else {
                        h.progress.phase()
                    },
                    Self::progress_text(&h.progress, &ses.meter, paused),
                    paused,
                )
            }
            _ => return,
        };
        let bad = ses
            .source
            .as_ref()
            .map(|s| s.log().bad_bytes())
            .unwrap_or(0);
        let mut detail = format!(
            "{detail} · {} files found",
            fmt_count(ses.files.len() as u64)
        );
        if bad > 0 {
            detail.push_str(&format!(" · {} unreadable", fmt_size(bad)));
        }
        let (mut toggle_pause, mut stop) = (false, false);
        widgets::card(ui, |ui| {
            ui.spacing_mut().item_spacing.y = 8.0;
            ui.allocate_ui_with_layout(
                vec2(ui.available_width(), 32.0),
                Layout::left_to_right(Align::Center),
                |ui| {
                    if !paused {
                        ui.add(egui::Spinner::new().size(18.0).color(p.accent));
                    }
                    ui.label(RichText::new(phase).font(semibold(15.0)));
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        if widgets::ghost_button(ui, &Icon::STOP.with("Stop"))
                            .on_hover_text("End the scan and keep what was found")
                            .clicked()
                        {
                            stop = true;
                        }
                        let label = if paused {
                            Icon::PLAY.with("Resume")
                        } else {
                            Icon::PAUSE.with("Pause")
                        };
                        if widgets::ghost_button(ui, &label)
                            .on_hover_text("Ctrl+P")
                            .clicked()
                        {
                            toggle_pause = true;
                        }
                    });
                },
            );
            widgets::progress_bar(ui, fraction, if paused { p.muted } else { p.accent });
            ui.add(egui::Label::new(RichText::new(detail).size(12.5).color(p.muted)).truncate());
        });
        if let Some(h) = &ses.scan {
            if toggle_pause {
                h.ctl.set_paused(!h.ctl.is_paused());
            }
            if stop {
                h.ctl.stop();
            }
        }
    }

    fn filter_rows(&mut self, ui: &mut Ui, ses: &mut Session) {
        let mut changed = false;
        ui.allocate_ui_with_layout(
            vec2(ui.available_width(), 32.0),
            Layout::left_to_right(Align::Center),
            |ui| {
                ui.spacing_mut().item_spacing.x = 6.0;
                let all: usize = ses.cat_counts.iter().sum();
                if widgets::chip(ui, "All", Some(all), ses.cat.is_none()).clicked() {
                    ses.cat = None;
                    changed = true;
                }
                for c in Category::ALL {
                    let n = ses.cat_counts[c.index()];
                    if widgets::chip(ui, c.label(), Some(n), ses.cat == Some(c)).clicked() {
                        ses.cat = if ses.cat == Some(c) { None } else { Some(c) };
                        changed = true;
                    }
                }
            },
        );
        ui.allocate_ui_with_layout(
            vec2(ui.available_width(), 32.0),
            Layout::left_to_right(Align::Center),
            |ui| {
                ui.spacing_mut().item_spacing.x = 8.0;
                macro_rules! combo {
                    ($id:literal, $field:expr, $all:expr, $width:expr) => {
                        egui::ComboBox::from_id_salt($id)
                            .selected_text($field.label())
                            .width($width)
                            .show_ui(ui, |ui| {
                                for v in $all {
                                    if ui.selectable_label($field == v, v.label()).clicked()
                                        && $field != v
                                    {
                                        $field = v;
                                        changed = true;
                                    }
                                }
                            });
                    };
                }
                combo!("size-filter", ses.size_filter, SizeFilter::ALL, 118.0);
                combo!("date-filter", ses.date_filter, DateFilter::ALL, 190.0);
                combo!("chance-filter", ses.chance_filter, ChanceFilter::ALL, 186.0);
                if ui.available_width() > 330.0 {
                    let mut existing = ses.show_existing;
                    if widgets::checkbox(ui, &mut existing, "Existing files")
                        .on_hover_text(
                            "Also show files that were never deleted (when the scan listed them)",
                        )
                        .changed()
                    {
                        ses.show_existing = existing;
                        changed = true;
                    }
                }
                if ses.filters_active() {
                    let tip = "Show all types, sizes, dates and ratings again";
                    let room = ui.available_width();
                    // Beside an open preview panel only the small button fits.
                    let clicked = if room > 290.0 {
                        widgets::ghost_button(ui, &Icon::CLOSE.with("Clear filters"))
                            .on_hover_text(tip)
                            .clicked()
                    } else {
                        room > 200.0
                            && widgets::icon_button(ui, Icon::CLOSE, "Clear filters").clicked()
                    };
                    if clicked {
                        ses.clear_filters();
                    }
                }
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    let current = if ses.grid { 1 } else { 0 };
                    if let Some(i) = widgets::segmented(ui, &["List", "Thumbnails"], current) {
                        ses.grid = i == 1;
                    }
                });
            },
        );
        if changed {
            ses.rebuild_now();
        }
    }

    fn tree_card(&mut self, ui: &mut Ui, ses: &mut Session, height: f32) {
        let p = pal(ui);
        const ROW: f32 = 30.0;
        let mut select: Option<u32> = None;
        let mut expand: Option<u32> = None;
        let mut check: Option<(u32, bool)> = None;
        Frame::new()
            .fill(p.card)
            .stroke(Stroke::new(1.0_f32, p.border))
            .corner_radius(CornerRadius::same(12))
            .inner_margin(Margin::symmetric(6, 6))
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.set_height(height - 14.0);
                let width = ui.available_width();
                let (hrect, _) = ui.allocate_exact_size(vec2(width, 28.0), Sense::hover());
                widgets::text_in(
                    ui,
                    Rect::from_min_max(pos2(hrect.left() + 10.0, hrect.top()), hrect.max),
                    "FOLDERS",
                    semibold(12.0),
                    p.faint,
                );
                ui.painter().hline(
                    hrect.x_range().shrink(6.0),
                    hrect.bottom() - 1.0,
                    Stroke::new(1.0_f32, p.border),
                );
                ui.spacing_mut().item_spacing.y = 0.0;
                egui::ScrollArea::vertical()
                    .id_salt("tree")
                    .auto_shrink([false, false])
                    .show_rows(ui, ROW, ses.tree_rows.len() + 1, |ui, range| {
                        for row in range {
                            let (dir, depth) = if row == 0 {
                                (0u32, 0u16)
                            } else {
                                ses.tree_rows[row - 1]
                            };
                            let (rect, resp) =
                                ui.allocate_exact_size(vec2(width, ROW), Sense::click());
                            let active = ses.folder == dir;
                            if active {
                                ui.painter().rect_filled(
                                    rect.shrink2(vec2(0.0, 1.0)),
                                    CornerRadius::same(7),
                                    p.selected,
                                );
                            } else if resp.hovered() {
                                ui.painter().rect_filled(
                                    rect.shrink2(vec2(0.0, 1.0)),
                                    CornerRadius::same(7),
                                    p.hover,
                                );
                            }
                            let mut x = rect.left() + 4.0 + depth as f32 * 14.0;
                            // Chevron.
                            if dir != 0 && ses.has_visible_children(dir) {
                                let r = Rect::from_min_size(pos2(x, rect.top()), vec2(16.0, ROW));
                                let open = ses.expanded.contains(&dir);
                                ui.painter().text(
                                    r.center(),
                                    Align2::CENTER_CENTER,
                                    if open {
                                        Icon::CHEVRON_DOWN.glyph()
                                    } else {
                                        Icon::CHEVRON_RIGHT.glyph()
                                    },
                                    regular(10.0),
                                    p.muted,
                                );
                                if ui
                                    .interact(r, ui.id().with(("chev", dir)), Sense::click())
                                    .clicked()
                                {
                                    expand = Some(dir);
                                }
                            }
                            x += 16.0;
                            // Checkbox.
                            let r = Rect::from_min_size(pos2(x, rect.top()), vec2(22.0, ROW));
                            let state = ses.folder_state(dir);
                            let cresp =
                                ui.interact(r, ui.id().with(("check", dir)), Sense::click());
                            widgets::paint_checkbox(ui, r, state, cresp.hovered());
                            if cresp.clicked() {
                                check = Some((dir, state != Some(true)));
                            }
                            x += 26.0;
                            let tint = if active { p.accent } else { p.muted };
                            if dir == 0 {
                                ui.painter().text(
                                    pos2(x + 8.0, rect.center().y),
                                    Align2::CENTER_CENTER,
                                    Icon::DRIVE.glyph(),
                                    regular(14.0),
                                    tint,
                                );
                            } else {
                                widgets::paint_folder(ui, pos2(x + 8.0, rect.center().y), tint);
                            }
                            x += 22.0;
                            let count = fmt_count(ses.dir_total[dir as usize] as u64);
                            let count_w = ui
                                .painter()
                                .layout_no_wrap(count.clone(), regular(12.0), p.faint)
                                .size()
                                .x;
                            let name = if dir == 0 {
                                "Everything"
                            } else {
                                ses.dirs.nodes[dir as usize].name.as_str()
                            };
                            let name_rect = Rect::from_min_max(
                                pos2(x, rect.top()),
                                pos2(rect.right() - count_w - 14.0, rect.bottom()),
                            );
                            let cut = widgets::text_in(
                                ui,
                                name_rect,
                                name,
                                if active {
                                    semibold(13.5)
                                } else {
                                    regular(13.5)
                                },
                                p.text,
                            );
                            ui.painter().text(
                                pos2(rect.right() - 8.0, rect.center().y),
                                Align2::RIGHT_CENTER,
                                count,
                                regular(12.0),
                                p.faint,
                            );
                            if cut {
                                resp.clone().on_hover_text(name);
                            }
                            if resp.clicked() {
                                select = Some(dir);
                            }
                            if resp.double_clicked() && dir != 0 {
                                expand = Some(dir);
                            }
                        }
                    });
            });
        if let Some(dir) = expand {
            if !ses.expanded.remove(&dir) {
                ses.expanded.insert(dir);
            }
            ses.rebuild_tree();
        }
        if let Some((dir, on)) = check {
            ses.set_folder(dir, on);
        }
        if let Some(dir) = select {
            if ses.folder != dir {
                ses.folder = dir;
                ses.rebuild_now();
            }
        }
    }

    /// Returns `true` when "Show everything" was pressed.
    fn empty_list(&self, ui: &mut Ui, ses: &Session) -> bool {
        let p = pal(ui);
        let mut show_all = false;
        if ses.scanning() && ses.files.is_empty() {
            widgets::busy_state(
                ui,
                "Looking for files",
                "Results appear here while the scan runs.",
            );
        } else if ses.files.is_empty() {
            widgets::empty_state(
                ui,
                Icon::SEARCH,
                p.muted,
                "Nothing found",
                if ses.options.deep {
                    "The scan found no recoverable files on this source. The scan log has the details."
                } else {
                    "The quick scan found no deleted files. A deep scan reads the whole drive and finds files by their content."
                },
            );
        } else {
            widgets::empty_state(
                ui,
                Icon::FILTER,
                p.muted,
                "No files match",
                "Nothing passes the current search and filters. Try another folder, or show everything again.",
            );
            ui.vertical_centered(|ui| {
                if widgets::primary_button(ui, "Show everything", true)
                    .on_hover_text("Clear the search and all filters and go back to the top folder")
                    .clicked()
                {
                    show_all = true;
                }
            });
        }
        show_all
    }

    fn table_card(&mut self, ui: &mut Ui, ses: &mut Session, height: f32) -> Option<RowAction> {
        let p = pal(ui);
        const ROW: f32 = LIST_ROW;
        let tz = self.tz;
        let mut action: Option<RowAction> = None;
        let mut show_all = false;
        let mut view: Option<(f32, f32)> = None;
        let mut toggle: Option<u32> = None;
        let mut highlight: Option<u32> = None;
        let mut open_big = false;
        let mut sort: Option<Col> = None;
        let mut all: Option<bool> = None;
        Frame::new()
            .fill(p.card)
            .stroke(Stroke::new(1.0_f32, p.border))
            .corner_radius(CornerRadius::same(12))
            .inner_margin(Margin::symmetric(6, 6))
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.set_height(height - 14.0);
                let width = ui.available_width();
                let (hrect, _) = ui.allocate_exact_size(vec2(width, 32.0), Sense::hover());
                ui.painter().hline(
                    hrect.x_range().shrink(6.0),
                    hrect.bottom() - 1.0,
                    Stroke::new(1.0_f32, p.border),
                );
                let cols = Columns::for_width(hrect.left(), width);
                // Header: tick-all box and sortable titles.
                let r =
                    Rect::from_min_size(pos2(hrect.left() + 8.0, hrect.top()), vec2(24.0, 32.0));
                let state = ses.visible_state();
                let cresp = ui.interact(r, ui.id().with("check-all"), Sense::click());
                if !ses.visible.is_empty() {
                    widgets::paint_checkbox(ui, r, state, cresp.hovered());
                    if cresp
                        .on_hover_text("Tick or untick every file shown (Ctrl+A)")
                        .clicked()
                    {
                        all = Some(state != Some(true));
                    }
                }
                let mut head = |col: Col, (x, w): (f32, f32), text: &str, right: bool| {
                    let r = Rect::from_min_max(
                        pos2(x, hrect.top()),
                        pos2(x + w - 12.0, hrect.bottom()),
                    );
                    let active = ses.sort.0 == col;
                    let label = if active {
                        format!(
                            "{text} {}",
                            if ses.sort.1 {
                                Icon::SORT_UP.glyph()
                            } else {
                                Icon::SORT_DOWN.glyph()
                            }
                        )
                    } else {
                        text.to_string()
                    };
                    let color = if active { p.text } else { p.faint };
                    if right {
                        widgets::text_in_right(ui, r, &label, semibold(12.0), color);
                    } else {
                        widgets::text_in(ui, r, &label, semibold(12.0), color);
                    }
                    if ui
                        .interact(r, ui.id().with(("head", text)), Sense::click())
                        .clicked()
                    {
                        sort = Some(col);
                    }
                };
                head(Col::Name, cols.name, "NAME", false);
                head(Col::Size, cols.size, "SIZE", true);
                if let Some(c) = cols.modified {
                    head(Col::Modified, c, "CHANGED", false);
                }
                head(Col::Chance, cols.chance, "RECOVERABILITY", false);
                if let Some(c) = cols.folder {
                    head(Col::Folder, c, "FOLDER", false);
                }

                if ses.visible.is_empty() {
                    show_all = self.empty_list(ui, ses);
                    return;
                }
                ui.spacing_mut().item_spacing.y = 0.0;
                let mut area = egui::ScrollArea::vertical()
                    .id_salt("table")
                    .auto_shrink([false, false]);
                // The keyboard moved the highlight: keep it in sight.
                if std::mem::take(&mut ses.scroll_to_highlight) {
                    if let Some(to) = ses.highlighted_row().and_then(|row| {
                        scroll_into_view(row as f32 * ROW, ROW, ses.list_view.0, ses.list_view.1)
                    }) {
                        area = area.vertical_scroll_offset(to);
                    }
                }
                let out = area.show_rows(ui, ROW, ses.visible.len(), |ui, range| {
                    for row in range {
                        let i = ses.visible[row];
                        let f = &ses.files[i as usize];
                        let (rect, resp) = ui.allocate_exact_size(vec2(width, ROW), Sense::click());
                        let active = ses.highlighted == Some(i);
                        if active {
                            ui.painter().rect_filled(
                                rect.shrink2(vec2(0.0, 1.0)),
                                CornerRadius::same(8),
                                p.selected,
                            );
                        } else if resp.hovered() {
                            ui.painter().rect_filled(
                                rect.shrink2(vec2(0.0, 1.0)),
                                CornerRadius::same(8),
                                p.hover,
                            );
                        }
                        let r = Rect::from_min_size(
                            pos2(rect.left() + 8.0, rect.top()),
                            vec2(24.0, ROW),
                        );
                        let cresp = ui.interact(r, ui.id().with(("row-check", i)), Sense::click());
                        widgets::paint_checkbox(
                            ui,
                            r,
                            Some(ses.checked[i as usize]),
                            cresp.hovered(),
                        );
                        if cresp.clicked() {
                            toggle = Some(i);
                        }
                        let cell = |(x, w): (f32, f32)| {
                            Rect::from_min_max(
                                pos2(x, rect.top()),
                                pos2(x + w - 12.0, rect.bottom()),
                            )
                        };
                        // Name with a type icon.
                        let (icon, color) = category_look(&p, f.category());
                        let name_cell = cell(cols.name);
                        ui.painter().text(
                            pos2(name_cell.left() + 9.0, rect.center().y),
                            Align2::CENTER_CENTER,
                            icon.glyph(),
                            regular(15.0),
                            color,
                        );
                        let mut name_rect = name_cell;
                        name_rect.min.x += 26.0;
                        if f.status != Status::Deleted {
                            let w = widgets::paint_badge(
                                ui,
                                Rect::from_min_size(
                                    pos2(name_cell.right() - 62.0, rect.top()),
                                    vec2(62.0, ROW),
                                ),
                                f.status.label(),
                                p.info,
                            );
                            name_rect.max.x = name_cell.right() - 62.0 - 6.0;
                            let _ = w;
                        }
                        let cut = widgets::text_in(ui, name_rect, &f.name, regular(14.0), p.text);
                        widgets::text_in_right(
                            ui,
                            cell(cols.size),
                            &fmt_size(f.size),
                            regular(13.0),
                            p.muted,
                        );
                        if let Some(c) = cols.modified {
                            let t = fmt_time(f.modified, tz);
                            widgets::text_in(
                                ui,
                                cell(c),
                                if t.is_empty() { "Not known" } else { &t },
                                regular(13.0),
                                if t.is_empty() { p.faint } else { p.muted },
                            );
                        }
                        let cc = cell(cols.chance);
                        widgets::paint_badge(ui, cc, f.chance.label(), chance_color(&p, f.chance));
                        if let Some(c) = cols.folder {
                            let path = if f.path.is_empty() {
                                "(top folder)"
                            } else {
                                f.path.as_str()
                            };
                            widgets::text_in(ui, cell(c), path, regular(13.0), p.faint);
                        }
                        if cut {
                            resp.clone().on_hover_text(&f.name);
                        }
                        if resp.clicked() || resp.secondary_clicked() {
                            highlight = Some(i);
                        }
                        if resp.double_clicked() {
                            highlight = Some(i);
                            open_big = true;
                        }
                        resp.context_menu(|ui| {
                            if let Some(a) = row_menu(ui, ses, i) {
                                action = Some(a);
                            }
                        });
                    }
                });
                view = Some((out.state.offset.y, out.inner_rect.height()));
            });
        if let Some(v) = view {
            ses.list_view = v;
        }
        if show_all {
            self.show_everything(ses);
        }
        if let Some(col) = sort {
            ses.sort = if ses.sort.0 == col {
                (col, !ses.sort.1)
            } else {
                (col, !matches!(col, Col::Size | Col::Modified))
            };
            ses.rebuild_now();
        }
        if let Some(on) = all {
            ses.set_visible(on);
        }
        if let Some(i) = toggle {
            ses.toggle(i);
        }
        if let Some(i) = highlight {
            ses.highlighted = Some(i);
        }
        if open_big {
            self.big_preview = true;
        }
        action
    }

    /// Back to the whole list: no search, no filters, the top folder.
    fn show_everything(&mut self, ses: &mut Session) {
        self.search.clear();
        ses.set_search("");
        ses.folder = 0;
        ses.clear_filters();
    }

    fn grid_card(&mut self, ui: &mut Ui, ses: &mut Session, height: f32) -> Option<RowAction> {
        let p = pal(ui);
        const TILE_W: f32 = 150.0;
        const TILE_H: f32 = 152.0;
        const GAP: f32 = TILE_ROW - TILE_H;
        let mut action: Option<RowAction> = None;
        let mut show_all = false;
        let mut view: Option<(f32, f32, usize)> = None;
        let thumbs_on = self.settings.thumbnails;
        let mut toggle: Option<u32> = None;
        let mut highlight: Option<u32> = None;
        let mut open_big = false;
        Frame::new()
            .fill(p.card)
            .stroke(Stroke::new(1.0_f32, p.border))
            .corner_radius(CornerRadius::same(12))
            .inner_margin(Margin::symmetric(10, 10))
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.set_height(height - 22.0);
                if ses.visible.is_empty() {
                    show_all = self.empty_list(ui, ses);
                    return;
                }
                let width = ui.available_width();
                let per_row = (((width + GAP) / (TILE_W + GAP)).floor() as usize).max(1);
                let tile_w = (width - GAP * (per_row as f32 - 1.0)) / per_row as f32;
                let rows = ses.visible.len().div_ceil(per_row);
                ui.spacing_mut().item_spacing.y = 0.0;
                let mut area = egui::ScrollArea::vertical()
                    .id_salt("grid")
                    .auto_shrink([false, false]);
                if std::mem::take(&mut ses.scroll_to_highlight) {
                    if let Some(to) = ses.highlighted_row().and_then(|at| {
                        scroll_into_view(
                            (at / per_row) as f32 * TILE_ROW,
                            TILE_ROW,
                            ses.list_view.0,
                            ses.list_view.1,
                        )
                    }) {
                        area = area.vertical_scroll_offset(to);
                    }
                }
                let out = area.show_rows(ui, TILE_H + GAP, rows, |ui, range| {
                    for row in range {
                        let (line, _) =
                            ui.allocate_exact_size(vec2(width, TILE_H + GAP), Sense::hover());
                        for k in 0..per_row {
                            let Some(&i) = ses.visible.get(row * per_row + k) else {
                                break;
                            };
                            let f = &ses.files[i as usize];
                            let rect = Rect::from_min_size(
                                pos2(line.left() + k as f32 * (tile_w + GAP), line.top()),
                                vec2(tile_w, TILE_H),
                            );
                            let resp = ui.interact(rect, ui.id().with(("tile", i)), Sense::click());
                            let active = ses.highlighted == Some(i);
                            ui.painter().rect_filled(
                                rect,
                                CornerRadius::same(10),
                                if active {
                                    p.selected
                                } else if resp.hovered() {
                                    p.hover
                                } else {
                                    p.bg
                                },
                            );
                            ui.painter().rect_stroke(
                                rect,
                                CornerRadius::same(10),
                                Stroke::new(1.0_f32, if active { p.accent } else { p.border }),
                                StrokeKind::Inside,
                            );
                            let picture = Rect::from_min_max(
                                rect.min + vec2(6.0, 6.0),
                                pos2(rect.right() - 6.0, rect.top() + 104.0),
                            );
                            let ext = f.ext();
                            let wanted = thumbs_on && crate::preview::has_thumbnail(&ext, f.size);
                            if wanted {
                                if let Some(src) = &ses.source {
                                    self.previewer.want_thumb(i, f, src);
                                }
                            }
                            match self.previewer.thumbs.get(&i) {
                                Some(Some(tex)) if wanted => paint_fitted(ui, picture, tex),
                                _ => {
                                    let (icon, color) = category_look(&p, f.category());
                                    ui.painter().text(
                                        picture.center(),
                                        Align2::CENTER_CENTER,
                                        icon.glyph(),
                                        regular(34.0),
                                        color.gamma_multiply(0.85),
                                    );
                                }
                            }
                            widgets::text_in(
                                ui,
                                Rect::from_min_max(
                                    pos2(rect.left() + 10.0, rect.top() + 106.0),
                                    pos2(rect.right() - 8.0, rect.top() + 128.0),
                                ),
                                &f.name,
                                regular(13.0),
                                p.text,
                            );
                            let dot = pos2(rect.left() + 15.0, rect.top() + 138.0);
                            ui.painter()
                                .circle_filled(dot, 4.0, chance_color(&p, f.chance));
                            widgets::text_in(
                                ui,
                                Rect::from_min_max(
                                    pos2(rect.left() + 25.0, rect.top() + 128.0),
                                    pos2(rect.right() - 8.0, rect.top() + 148.0),
                                ),
                                &format!("{} · {}", fmt_size(f.size), f.chance.label()),
                                regular(11.5),
                                p.faint,
                            );
                            // Tick box on a small plate, top left.
                            let plate =
                                Rect::from_min_size(rect.min + vec2(8.0, 8.0), vec2(26.0, 26.0));
                            ui.painter().rect_filled(
                                plate,
                                CornerRadius::same(7),
                                p.card.gamma_multiply(0.92),
                            );
                            let cresp =
                                ui.interact(plate, ui.id().with(("tile-check", i)), Sense::click());
                            widgets::paint_checkbox(
                                ui,
                                plate,
                                Some(ses.checked[i as usize]),
                                cresp.hovered(),
                            );
                            if cresp.clicked() {
                                toggle = Some(i);
                            } else if resp.double_clicked() {
                                highlight = Some(i);
                                open_big = true;
                            } else if resp.clicked() || resp.secondary_clicked() {
                                highlight = Some(i);
                            }
                            resp.context_menu(|ui| {
                                if let Some(a) = row_menu(ui, ses, i) {
                                    action = Some(a);
                                }
                            });
                        }
                    }
                });
                view = Some((out.state.offset.y, out.inner_rect.height(), per_row));
            });
        if let Some((offset, tall, per_row)) = view {
            ses.list_view = (offset, tall);
            ses.per_row = per_row;
        }
        if show_all {
            self.show_everything(ses);
        }
        if let Some(i) = toggle {
            ses.toggle(i);
        }
        if let Some(i) = highlight {
            ses.highlighted = Some(i);
        }
        if open_big {
            self.big_preview = true;
        }
        action
    }

    // ----------------------------------------------------------------------------------
    // Selection bar
    // ----------------------------------------------------------------------------------

    pub(super) fn selection_bar(&mut self, ui: &mut Ui) {
        let p = pal(ui);
        let Some(mut ses) = self.ses.take() else {
            return;
        };
        let full = ui.max_rect();
        ui.painter().hline(
            full.x_range().expand(24.0),
            full.top(),
            Stroke::new(1.0_f32, p.border),
        );
        let mut recover = false;
        ui.horizontal_centered(|ui| {
            if ses.sel_count > 0 {
                ui.label(
                    RichText::new(format!(
                        "{} file{} selected",
                        fmt_count(ses.sel_count),
                        if ses.sel_count == 1 { "" } else { "s" }
                    ))
                    .font(semibold(15.0)),
                );
                ui.label(RichText::new(fmt_size(ses.sel_bytes)).color(p.muted));
            } else {
                ui.label(RichText::new("No files selected").font(semibold(15.0)));
                ui.label(
                    RichText::new("Tick files or whole folders to recover them").color(p.muted),
                );
            }
            ui.add_space(8.0);
            if widgets::ghost_button_enabled(ui, "Select all shown", !ses.visible.is_empty())
                .clicked()
            {
                ses.set_visible(true);
            }
            if widgets::ghost_button_enabled(ui, "Clear", ses.sel_count > 0).clicked() {
                ses.clear_selection();
            }
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                let ready = ses.sel_count > 0 && ses.source.is_some();
                let label = if ses.sel_count > 0 {
                    Icon::RECOVER.with(&format!(
                        "Recover {} file{}",
                        fmt_count(ses.sel_count),
                        if ses.sel_count == 1 { "" } else { "s" }
                    ))
                } else {
                    Icon::RECOVER.with("Recover")
                };
                if widgets::primary_button(ui, &label, ready)
                    .on_hover_text("Choose where to save the selected files (Ctrl+R)")
                    .clicked()
                {
                    recover = true;
                }
            });
        });
        self.ses = Some(ses);
        if recover {
            self.open_recover_dialog(None);
        }
    }

    // ----------------------------------------------------------------------------------
    // Preview
    // ----------------------------------------------------------------------------------

    pub(super) fn preview_panel(&mut self, ui: &mut Ui) {
        let p = pal(ui);
        let Some(mut ses) = self.ses.take() else {
            return;
        };
        let Some(i) = ses.highlighted.filter(|i| (*i as usize) < ses.files.len()) else {
            self.ses = Some(ses);
            return;
        };
        let f = ses.files[i as usize].clone();
        let full = ui.max_rect();
        ui.painter().vline(
            full.left() - 18.0,
            full.y_range().expand(18.0),
            Stroke::new(1.0_f32, p.border),
        );
        let (mut close, mut big, mut toggle, mut recover) = (false, false, false, false);
        ui.allocate_ui_with_layout(
            vec2(ui.available_width(), 30.0),
            Layout::left_to_right(Align::Center),
            |ui| {
                ui.label(RichText::new("PREVIEW").font(semibold(12.0)).color(p.faint));
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    if widgets::icon_button(ui, Icon::CLOSE, "Close the preview (Esc)").clicked() {
                        close = true;
                    }
                });
            },
        );
        egui::ScrollArea::vertical()
            .id_salt("preview-scroll")
            .auto_shrink([false, false])
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                // The picture, text or hex view.
                let (area, resp) =
                    ui.allocate_exact_size(vec2(ui.available_width(), 186.0), Sense::click());
                ui.painter().rect_filled(area, CornerRadius::same(10), p.bg);
                ui.painter().rect_stroke(
                    area,
                    CornerRadius::same(10),
                    Stroke::new(1.0_f32, p.border),
                    StrokeKind::Inside,
                );
                let inner = area.shrink(8.0);
                let (icon, color) = category_look(&p, f.category());
                match self.previewer.full.as_ref().filter(|x| x.id == i) {
                    Some(x) if x.loading => {
                        ui.put(
                            Rect::from_center_size(area.center(), vec2(28.0, 28.0)),
                            egui::Spinner::new().size(26.0).color(p.accent),
                        );
                    }
                    Some(x) if x.texture.is_some() => {
                        if let Some(tex) = &x.texture {
                            paint_fitted(ui, inner, tex);
                        }
                    }
                    Some(x) if !x.head.is_empty() => {
                        let (text, font) = match &x.text {
                            Some(t) => (t.chars().take(1200).collect::<String>(), regular(12.0)),
                            None => (
                                x.hex
                                    .lines()
                                    .take(13)
                                    .map(|l| &l[..l.len().min(60)])
                                    .collect::<Vec<_>>()
                                    .join("\n"),
                                mono(9.5),
                            ),
                        };
                        // Kurdish, Arabic and the like are laid out by Windows.
                        let native = x.text.is_some() && super::bidi::needs_native(&text) && {
                            let short: String = text.chars().take(700).collect();
                            super::bidi::paint_block(ui, inner, &short, 12.0, p.muted)
                        };
                        if !native {
                            let galley = ui.painter().layout(text, font, p.muted, inner.width());
                            ui.painter()
                                .with_clip_rect(inner)
                                .galley(inner.min, galley, p.muted);
                        }
                    }
                    _ => {
                        ui.painter().text(
                            area.center() - vec2(0.0, 10.0),
                            Align2::CENTER_CENTER,
                            icon.glyph(),
                            regular(40.0),
                            color,
                        );
                        ui.painter().text(
                            area.center() + vec2(0.0, 34.0),
                            Align2::CENTER_CENTER,
                            if ses.source.is_some() {
                                "No preview"
                            } else {
                                "The drive is not connected"
                            },
                            regular(12.5),
                            p.faint,
                        );
                    }
                }
                if resp
                    .on_hover_text("Open the large preview (Enter)")
                    .clicked()
                {
                    big = true;
                }
                ui.add_space(4.0);
                widgets::name_label(ui, &f.name, 16.0, true);
                ui.horizontal(|ui| {
                    widgets::badge(ui, f.chance.label(), chance_color(&p, f.chance));
                    widgets::badge(ui, f.status.label(), p.info);
                });
                ui.add(
                    egui::Label::new(RichText::new(f.chance.explain()).size(12.5).color(p.muted))
                        .wrap(),
                );
                if !f.note.is_empty() {
                    widgets::notice(ui, Icon::INFO, p.warn, &f.note);
                }
                ui.add_space(2.0);
                let dims = self
                    .previewer
                    .full
                    .as_ref()
                    .filter(|x| x.id == i && x.dims.0 > 0)
                    .map(|x| format!("{} x {} pixels", x.dims.0, x.dims.1));
                widgets::field(
                    ui,
                    "Size",
                    &match dims {
                        Some(d) => format!("{} · {d}", fmt_size(f.size)),
                        None => fmt_size(f.size),
                    },
                );
                widgets::field(ui, "Changed", &fmt_time(f.modified, self.tz));
                if f.created > 0 && f.created != f.modified {
                    widgets::field(ui, "Created", &fmt_time(f.created, self.tz));
                }
                widgets::field(
                    ui,
                    "Folder",
                    if f.path.is_empty() {
                        "(top folder)"
                    } else {
                        &f.path
                    },
                );
                widgets::field(ui, "Found through", f.origin.label());
                ui.add_space(4.0);
                let mut checked = ses.checked[i as usize];
                if widgets::checkbox(ui, &mut checked, "Include in the recovery").changed() {
                    toggle = true;
                }
                ui.add_space(2.0);
                ui.horizontal(|ui| {
                    if widgets::primary_button(
                        ui,
                        &Icon::RECOVER.with("Recover this file"),
                        ses.source.is_some(),
                    )
                    .clicked()
                    {
                        recover = true;
                    }
                    if widgets::ghost_button(ui, &Icon::VIEW.with("Larger")).clicked() {
                        big = true;
                    }
                });
                ui.add_space(8.0);
            });
        if toggle {
            ses.toggle(i);
        }
        if close {
            ses.highlighted = None;
        }
        self.ses = Some(ses);
        if big {
            self.big_preview = true;
        }
        if recover {
            self.open_recover_dialog(Some(i));
        }
    }

    pub(super) fn big_preview_dialog(&mut self, ctx: &egui::Context) {
        if !self.big_preview {
            return;
        }
        let Some((i, name, size)) = self.ses.as_ref().and_then(|s| {
            let i = s.highlighted?;
            let f = s.files.get(i as usize)?;
            Some((i, f.name.clone(), f.size))
        }) else {
            self.big_preview = false;
            return;
        };
        let p = super::theme::pal_ctx(ctx);
        let mut close = false;
        let screen = ctx.screen_rect().size();
        let (w, h) = (
            (screen.x - 160.0).clamp(600.0, 1100.0),
            (screen.y - 190.0).clamp(380.0, 760.0),
        );
        let modal = egui::Modal::new(egui::Id::new("big-preview"))
            .frame(super::app::modal_frame(ctx, 18))
            .show(ctx, |ui| {
                ui.set_width(w);
                ui.allocate_ui_with_layout(
                    vec2(w, 32.0),
                    Layout::left_to_right(Align::Center),
                    |ui| {
                        widgets::name_label(ui, &name, 17.0, false);
                        ui.label(RichText::new(fmt_size(size)).color(p.muted));
                        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                            if widgets::icon_button(ui, Icon::CLOSE, "Close (Esc)").clicked() {
                                close = true;
                            }
                        });
                    },
                );
                let (area, _) = ui.allocate_exact_size(vec2(w, h), Sense::hover());
                ui.painter().rect_filled(area, CornerRadius::same(10), p.bg);
                let full = self.previewer.full.as_ref().filter(|x| x.id == i);
                match full {
                    Some(x) if x.loading => {
                        ui.put(
                            Rect::from_center_size(area.center(), vec2(34.0, 34.0)),
                            egui::Spinner::new().size(30.0).color(p.accent),
                        );
                    }
                    Some(x) if x.texture.is_some() => {
                        if let Some(tex) = &x.texture {
                            paint_fitted(ui, area.shrink(8.0), tex);
                        }
                    }
                    Some(x) if !x.head.is_empty() => {
                        let mut child =
                            ui.new_child(egui::UiBuilder::new().max_rect(area.shrink(12.0)));
                        egui::ScrollArea::both()
                            .id_salt("big-text")
                            .auto_shrink([false, false])
                            .show(&mut child, |ui| {
                                let (text, font) = match &x.text {
                                    Some(t) => (t.as_str(), regular(14.0)),
                                    None => (x.hex.as_str(), mono(13.0)),
                                };
                                if x.text.is_some() && super::bidi::needs_native(text) {
                                    // Paragraph by paragraph, so that each one is
                                    // aligned by its own direction.
                                    let wide = ui.available_width() - 8.0;
                                    let mut drawn = true;
                                    for para in text.lines().take(400) {
                                        if para.trim().is_empty() {
                                            ui.add_space(8.0);
                                        } else if super::bidi::block(ui, para, 14.0, p.text, wide)
                                            .is_none()
                                        {
                                            drawn = false;
                                            break;
                                        }
                                    }
                                    if drawn {
                                        return;
                                    }
                                }
                                ui.add(
                                    egui::Label::new(RichText::new(text).font(font).color(p.text))
                                        .wrap_mode(if x.text.is_some() {
                                            egui::TextWrapMode::Wrap
                                        } else {
                                            egui::TextWrapMode::Extend
                                        })
                                        .selectable(true),
                                );
                            });
                    }
                    _ => {
                        ui.painter().text(
                            area.center(),
                            Align2::CENTER_CENTER,
                            "There is nothing to preview for this file.",
                            regular(15.0),
                            p.muted,
                        );
                    }
                }
                let caption = match full {
                    Some(x) if x.texture.is_some() && !x.caption.is_empty() => {
                        format!("{} · {} x {} pixels", x.caption, x.dims.0, x.dims.1)
                    }
                    Some(x) if x.texture.is_some() => format!("{} x {} pixels", x.dims.0, x.dims.1),
                    Some(x) if x.text.is_some() && !x.caption.is_empty() => x.caption.to_string(),
                    Some(x) if x.text.is_some() => "The first 64 KB as text".to_string(),
                    Some(x) if !x.head.is_empty() => {
                        "The first 8 KB, byte by byte (offset, hexadecimal, characters)".to_string()
                    }
                    _ => String::new(),
                };
                ui.label(RichText::new(caption).size(12.0).color(p.faint));
            });
        if close || modal.should_close() {
            self.big_preview = false;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn columns_always_fit() {
        for width in [
            420.0_f32, 500.0, 599.0, 600.0, 760.0, 859.0, 860.0, 1000.0, 1400.0, 2400.0,
        ] {
            let c = Columns::for_width(100.0, width);
            assert!(c.name.1 >= 138.0, "name column at {width}: {}", c.name.1);
            let end = c
                .folder
                .or(Some(c.chance))
                .map(|(x, w)| x + w)
                .unwrap_or(0.0);
            assert!(
                end <= 100.0 + width,
                "columns end at {end} for width {width}"
            );
            assert!(c.size.0 >= c.name.0 + c.name.1 - 0.5);
            assert_eq!(c.modified.is_some(), width >= 600.0);
            assert_eq!(c.folder.is_some(), width >= 860.0);
        }
    }
}
