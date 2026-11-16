//! The "Recover files" tab: pick a source and a kind of scan.

use super::app::{App, SourceEntry, SrcKind, Tab};
use super::theme::{pal, regular, semibold, Icon};
use super::widgets;
use crate::util::fmt_size;
use eframe::egui::{
    self, pos2, vec2, Align2, CornerRadius, Frame, Margin, Rect, RichText, Sense, Stroke, Ui,
};

impl SourceEntry {
    /// "NTFS · 931 GB · 212 GB free"
    pub fn summary(&self) -> String {
        let mut parts = vec![self.detail.clone()];
        if self.total > 0 {
            parts.push(fmt_size(self.total));
        }
        if let Some(free) = self.free {
            parts.push(format!("{} free", fmt_size(free)));
        }
        if self.kind != SrcKind::Image && !self.hardware.is_empty() {
            parts.push(self.hardware.clone());
        }
        parts.join(" · ")
    }

    pub fn icon(&self) -> Icon {
        match self.kind {
            SrcKind::Image => Icon::IMAGE_FILE,
            SrcKind::Disk => Icon::DISK,
            SrcKind::Drive if self.removable => Icon::USB,
            SrcKind::Drive => Icon::DRIVE,
        }
    }
}

impl App {
    pub(super) fn home_ui(&mut self, ui: &mut Ui) {
        let p = pal(ui);
        let (mut refresh, mut open_session, mut browse) = (false, false, false);
        Self::page_header(
            ui,
            "Recover files",
            "Choose the drive, disk or image that held the files, then start a scan",
            |ui| {
                if widgets::ghost_button(ui, &Icon::OPEN.with("Open saved scan")).clicked() {
                    open_session = true;
                }
                if widgets::ghost_button(ui, &Icon::REFRESH.with("Refresh")).clicked() {
                    refresh = true;
                }
            },
        );
        if refresh {
            self.load_sources();
        }
        if open_session {
            self.open_session();
        }
        if !self.elevated {
            widgets::notice(
                ui,
                Icon::WARNING,
                p.warn,
                "RescueForge is running without administrator rights. Windows only lets administrators read drives and disks directly, so scanning them needs \"Restart as administrator\" (top right). Disk image files can be scanned as they are.",
            );
            ui.add_space(4.0);
        }

        let total_w = ui.available_width();
        let side_w = 372.0_f32.min(total_w * 0.42);
        let list_w = total_w - side_w - 16.0;
        let height = ui.available_height();
        let mut start: Option<usize> = None;
        let mut make_image = false;
        ui.horizontal_top(|ui| {
            ui.spacing_mut().item_spacing.x = 16.0;
            // Left: the sources.
            ui.allocate_ui_with_layout(
                vec2(list_w, height),
                egui::Layout::top_down(egui::Align::Min),
                |ui| {
                    Frame::new()
                        .fill(p.card)
                        .stroke(Stroke::new(1.0_f32, p.border))
                        .corner_radius(CornerRadius::same(12))
                        .inner_margin(Margin::symmetric(6, 6))
                        .show(ui, |ui| {
                            ui.set_width(list_w - 12.0);
                            ui.set_height(height - 14.0);
                            egui::ScrollArea::vertical()
                                .auto_shrink([false, false])
                                .show(ui, |ui| {
                                    ui.spacing_mut().item_spacing.y = 2.0;
                                    for (kind, title, hint) in [
                                        (SrcKind::Drive, "DRIVES", "Volumes with a drive letter: the usual choice for deleted files"),
                                        (SrcKind::Disk, "PHYSICAL DISKS", "Whole devices: for lost partitions, or drives that show no letter"),
                                        (SrcKind::Image, "DISK IMAGES", "Copies of drives saved as files"),
                                    ] {
                                        self.source_group(ui, kind, title, hint);
                                        if kind == SrcKind::Image {
                                            ui.add_space(4.0);
                                            ui.horizontal(|ui| {
                                                ui.add_space(12.0);
                                                if widgets::ghost_button(ui, &Icon::ADD.with("Open an image file")).clicked() {
                                                    browse = true;
                                                }
                                            });
                                        }
                                        ui.add_space(8.0);
                                    }
                                });
                        });
                },
            );
            // Right: what to do with the selected source.
            ui.allocate_ui_with_layout(
                vec2(side_w, height),
                egui::Layout::top_down(egui::Align::Min),
                |ui| {
                  egui::ScrollArea::vertical()
                    .id_salt("scan-side")
                    .auto_shrink([false, false])
                    .show(ui, |ui| {
                    widgets::card(ui, |ui| {
                        ui.spacing_mut().item_spacing.y = 8.0;
                        let picked = self.picked.and_then(|i| self.sources.get(i)).cloned();
                        match &picked {
                            Some(s) => {
                                ui.label(RichText::new("SELECTED").font(semibold(11.5)).color(p.faint));
                                ui.add_space(-4.0);
                                ui.add(egui::Label::new(RichText::new(&s.title).font(semibold(18.0))).truncate());
                                ui.add_space(-6.0);
                                ui.add(egui::Label::new(RichText::new(s.summary()).size(12.5).color(p.muted)).wrap());
                                if s.ssd_trim {
                                    widgets::notice(
                                        ui,
                                        Icon::INFO,
                                        p.warn,
                                        "This is a solid-state drive with TRIM. It usually erases deleted files by itself within moments, so they are often gone for good. Lost partitions and formatted drives can still be worth a scan.",
                                    );
                                }
                            }
                            None => {
                                ui.label(RichText::new("Nothing selected").font(semibold(18.0)));
                                ui.label(RichText::new("Pick a drive, disk or image on the left.").color(p.muted));
                            }
                        }
                        ui.add_space(2.0);
                        if scan_choice(
                            ui,
                            !self.deep,
                            "Quick scan",
                            "Seconds to minutes",
                            "Reads the drive's own records of deleted files. Brings back names, folders and dates.",
                        ) {
                            self.deep = false;
                        }
                        if scan_choice(
                            ui,
                            self.deep,
                            "Deep scan",
                            "Minutes to hours",
                            "A quick scan, then every sector is read: finds files by their content, lost partitions and files from before a format.",
                        ) {
                            self.deep = true;
                        }
                        ui.add_space(2.0);
                        let before = self.settings.clone();
                        widgets::checkbox_enabled(ui, &mut self.settings.free_only, "Deep scan: search free space only (faster)", self.deep);
                        widgets::checkbox_enabled(ui, &mut self.settings.find_partitions, "Deep scan: look for lost partitions", self.deep);
                        widgets::checkbox(ui, &mut self.settings.include_existing, "Also list files that were not deleted");
                        if before != self.settings {
                            self.save_settings();
                        }
                        ui.add_space(4.0);
                        let ready = picked.is_some() && !self.busy();
                        let label = Icon::SCAN.with(if self.deep { "Start deep scan" } else { "Start quick scan" });
                        ui.scope(|ui| {
                            ui.spacing_mut().button_padding = vec2(16.0, 9.0);
                            let b = egui::Button::new(
                                RichText::new(label)
                                    .font(semibold(15.0))
                                    .color(if ready { p.on_accent } else { p.faint }),
                            )
                            .fill(if ready { p.accent } else { p.hover })
                            .stroke(Stroke::NONE)
                            .corner_radius(CornerRadius::same(9))
                            .min_size(vec2(ui.available_width(), 42.0));
                            if ui.add_enabled(ready, b).clicked() {
                                start = self.picked;
                            }
                        });
                        let can_image = picked.as_ref().map(|s| s.kind != SrcKind::Image).unwrap_or(false);
                        let b = egui::Button::new(
                            RichText::new(Icon::IMAGE_FILE.with("Create an image of it first"))
                                .color(if can_image { p.text } else { p.faint }),
                        )
                        .fill(egui::Color32::TRANSPARENT)
                        .stroke(Stroke::new(1.0_f32, p.border))
                        .corner_radius(CornerRadius::same(9))
                        .min_size(vec2(ui.available_width(), 36.0));
                        if ui
                            .add_enabled(can_image, b)
                            .on_hover_text("Recommended when the drive is failing: it is read once, and every scan after that works on the copy.")
                            .clicked()
                        {
                            make_image = true;
                        }
                    });
                    ui.add_space(4.0);
                    widgets::notice(
                        ui,
                        Icon::SHIELD,
                        p.good,
                        "The source is opened read-only. Recovered files must go to a different drive; RescueForge will not write to the one it is scanning.",
                    );
                    });
                },
            );
        });
        if browse {
            self.browse_image();
        }
        if let Some(i) = start {
            let ctx = ui.ctx().clone();
            self.start_scan(&ctx, i);
        }
        if make_image {
            self.imaging.source = self.picked;
            self.tab = Tab::Images;
        }
    }

    fn source_group(&mut self, ui: &mut Ui, kind: SrcKind, title: &str, hint: &str) {
        let p = pal(ui);
        let width = ui.available_width();
        let (hrect, _) = ui.allocate_exact_size(vec2(width, 30.0), Sense::hover());
        let used = widgets::text_in(
            ui,
            Rect::from_min_max(pos2(hrect.left() + 14.0, hrect.top()), hrect.max),
            title,
            semibold(12.0),
            p.faint,
        );
        let _ = used;
        let title_w = ui
            .painter()
            .layout_no_wrap(title.to_string(), semibold(12.0), p.faint)
            .size()
            .x;
        widgets::text_in(
            ui,
            Rect::from_min_max(
                pos2(hrect.left() + 26.0 + title_w, hrect.top()),
                hrect.max - vec2(12.0, 0.0),
            ),
            hint,
            regular(12.0),
            p.faint,
        );
        let indices: Vec<usize> = (0..self.sources.len())
            .filter(|i| self.sources[*i].kind == kind)
            .collect();
        if indices.is_empty() {
            let (rect, _) = ui.allocate_exact_size(vec2(width, 36.0), Sense::hover());
            let text = match kind {
                SrcKind::Drive => "No drives found.",
                SrcKind::Disk => "No physical disks found.",
                SrcKind::Image => "No image opened yet.",
            };
            ui.painter().text(
                rect.left_center() + vec2(14.0, 0.0),
                Align2::LEFT_CENTER,
                text,
                regular(13.5),
                p.muted,
            );
            return;
        }
        for i in indices {
            let s = &self.sources[i];
            let active = self.picked == Some(i);
            let (rect, resp) = ui.allocate_exact_size(vec2(width, 52.0), Sense::click());
            let inner = rect.shrink2(vec2(2.0, 1.0));
            if active {
                ui.painter()
                    .rect_filled(inner, CornerRadius::same(9), p.selected);
                ui.painter().rect_stroke(
                    inner,
                    CornerRadius::same(9),
                    Stroke::new(1.0_f32, p.accent.gamma_multiply(0.6)),
                    egui::StrokeKind::Inside,
                );
            } else if resp.hovered() {
                ui.painter()
                    .rect_filled(inner, CornerRadius::same(9), p.hover);
            }
            // Icon tile.
            let tile =
                Rect::from_center_size(pos2(rect.left() + 34.0, rect.center().y), vec2(36.0, 36.0));
            ui.painter().rect_filled(
                tile,
                CornerRadius::same(10),
                if active {
                    p.accent.gamma_multiply(0.22)
                } else {
                    p.hover
                },
            );
            ui.painter().text(
                tile.center(),
                Align2::CENTER_CENTER,
                s.icon().glyph(),
                regular(18.0),
                if active { p.accent } else { p.muted },
            );
            // Usage bar on the right for drives.
            let bar_w = if s.free.is_some() && width > 520.0 {
                150.0
            } else {
                0.0
            };
            let text_right = rect.right() - 18.0 - bar_w - if bar_w > 0.0 { 14.0 } else { 0.0 };
            let mut title_right = text_right;
            if s.ssd_trim {
                let w = 78.0;
                widgets::paint_badge(
                    ui,
                    Rect::from_min_size(pos2(text_right - w, rect.top() + 6.0), vec2(w, 20.0)),
                    "SSD · TRIM",
                    p.warn,
                );
                title_right -= w + 8.0;
            }
            widgets::text_in(
                ui,
                Rect::from_min_max(
                    pos2(rect.left() + 64.0, rect.top() + 6.0),
                    pos2(title_right, rect.top() + 28.0),
                ),
                &s.title,
                semibold(15.0),
                p.text,
            );
            widgets::text_in(
                ui,
                Rect::from_min_max(
                    pos2(rect.left() + 64.0, rect.top() + 27.0),
                    pos2(text_right, rect.bottom() - 6.0),
                ),
                &s.summary(),
                regular(12.5),
                p.muted,
            );
            if bar_w > 0.0 {
                let free = s.free.unwrap_or(0);
                let used = if s.total > 0 {
                    1.0 - free as f32 / s.total as f32
                } else {
                    0.0
                };
                let bar = Rect::from_min_size(
                    pos2(rect.right() - 18.0 - bar_w, rect.center().y + 5.0),
                    vec2(bar_w, 6.0),
                );
                ui.painter()
                    .rect_filled(bar, CornerRadius::same(3), p.border);
                ui.painter().rect_filled(
                    Rect::from_min_size(bar.min, vec2(bar_w * used.clamp(0.02, 1.0), 6.0)),
                    CornerRadius::same(3),
                    if used > 0.9 { p.danger } else { p.info },
                );
                widgets::text_in_right(
                    ui,
                    Rect::from_min_max(
                        pos2(bar.left(), rect.top() + 12.0),
                        pos2(bar.right(), rect.center().y + 2.0),
                    ),
                    &format!("{:.0}% used", used * 100.0),
                    regular(12.0),
                    p.faint,
                );
            }
            if resp.clicked() {
                self.picked = Some(i);
            }
            if resp.double_clicked() {
                self.picked = Some(i);
            }
        }
    }
}

/// One of the two big "kind of scan" choices. Returns true when clicked.
fn scan_choice(ui: &mut Ui, active: bool, title: &str, time: &str, text: &str) -> bool {
    let p = pal(ui);
    let width = ui.available_width();
    let galley = widgets_wrap(ui, text, width - 58.0);
    let height = 40.0 + galley.size().y + 12.0;
    let (rect, resp) = ui.allocate_exact_size(vec2(width, height), Sense::click());
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
        egui::StrokeKind::Inside,
    );
    // Radio mark.
    let c = pos2(rect.left() + 22.0, rect.top() + 23.0);
    ui.painter().circle_stroke(
        c,
        8.0,
        Stroke::new(1.5_f32, if active { p.accent } else { p.faint }),
    );
    if active {
        ui.painter().circle_filled(c, 4.5, p.accent);
    }
    ui.painter().text(
        pos2(rect.left() + 42.0, rect.top() + 23.0),
        Align2::LEFT_CENTER,
        title,
        semibold(15.5),
        p.text,
    );
    ui.painter().text(
        pos2(rect.right() - 14.0, rect.top() + 23.0),
        Align2::RIGHT_CENTER,
        time,
        regular(12.0),
        p.faint,
    );
    ui.painter()
        .galley(pos2(rect.left() + 42.0, rect.top() + 38.0), galley, p.muted);
    resp.clicked()
}

fn widgets_wrap(ui: &Ui, text: &str, width: f32) -> std::sync::Arc<egui::Galley> {
    let p = pal(ui);
    ui.painter()
        .layout(text.to_string(), regular(12.5), p.muted, width.max(40.0))
}
