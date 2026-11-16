//! The Disk images, Scan log and Settings tabs.

use super::app::{App, ImagingPhase, SrcKind, Tab};
use super::state::Meter;
use super::theme::{self, pal, regular, semibold, Icon};
use super::widgets;
use crate::carve::TYPES;
use crate::control::{Control, Progress};
use crate::model::Category;
use crate::recover::{self, Verdict};
use crate::settings::ThemeChoice;
use crate::util::{fmt_count, fmt_size};
use eframe::egui::{self, vec2, Align, Layout, Margin, RichText, Ui};
use std::sync::atomic::Ordering;
use std::sync::mpsc::channel;
use std::sync::Arc;

impl App {
    // ----------------------------------------------------------------------------------
    // Disk images
    // ----------------------------------------------------------------------------------

    fn judge_image(&mut self) {
        self.imaging.confirmed = false;
        let dest = self.imaging.dest.trim().to_string();
        let Some(src) = self.imaging.source.and_then(|i| self.sources.get(i)) else {
            self.imaging.verdict = Verdict::Ok;
            return;
        };
        if dest.is_empty() {
            self.imaging.verdict = Verdict::Ok;
            return;
        }
        let info = self.dest_info(&dest);
        self.imaging.verdict = recover::judge(&src.spec, &src.disks, &info);
        self.imaging.info = info;
    }

    fn start_imaging(&mut self, ctx: &egui::Context) {
        if self.demo {
            self.toast(
                "Demo mode: nothing is written. In the real program the drive would now be copied.",
                false,
            );
            return;
        }
        if self.busy() {
            self.toast(
                "Something is still running. Stop it or wait for it to finish first.",
                true,
            );
            return;
        }
        let Some(entry) = self
            .imaging
            .source
            .and_then(|i| self.sources.get(i))
            .cloned()
        else {
            return;
        };
        let source = match self.open_source(&entry) {
            Ok(s) => s,
            Err(e) => {
                self.toast(e, true);
                return;
            }
        };
        let dest = std::path::PathBuf::from(self.imaging.dest.trim());
        let ctl = Arc::new(Control::new());
        let progress = Arc::new(Progress::new());
        let (tx, rx) = channel();
        let (c, p, s, wake) = (ctl.clone(), progress.clone(), source.clone(), ctx.clone());
        std::thread::Builder::new()
            .name("imaging".into())
            .spawn(move || {
                let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    crate::imaging::create_image(&s, &dest, &c, &p)
                }))
                .unwrap_or_else(|_| {
                    Err("Creating the image ran into an internal error.".to_string())
                });
                let _ = tx.send(result);
                wake.request_repaint();
            })
            .expect("imaging thread");
        self.imaging.phase = ImagingPhase::Running {
            ctl,
            progress,
            rx,
            meter: Meter::default(),
            source,
        };
    }

    pub(super) fn poll_imaging(&mut self) {
        let ImagingPhase::Running {
            rx,
            progress,
            meter,
            ctl,
            ..
        } = &mut self.imaging.phase
        else {
            return;
        };
        if ctl.is_paused() {
            meter.reset();
        } else {
            meter.sample(progress.done.load(Ordering::Relaxed));
        }
        if let Ok(result) = rx.try_recv() {
            self.imaging.phase = match result {
                Ok(report) => ImagingPhase::Done(report),
                Err(e) => ImagingPhase::Failed(e),
            };
        }
    }

    pub(super) fn images_ui(&mut self, ui: &mut Ui) {
        let p = pal(ui);
        Self::page_header(
            ui,
            "Disk images",
            "Copy a whole drive into one file, then scan the copy instead of the drive",
            |_| {},
        );
        let ctx = ui.ctx().clone();
        let (mut browse_dest, mut start, mut rejudge, mut open_image, mut scan_image) =
            (false, false, false, false, None::<String>);
        egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
            ui.set_max_width(860.0);
            ui.spacing_mut().item_spacing.y = 12.0;
            widgets::card(ui, |ui| {
                widgets::section(ui, "Create an image of a drive");
                ui.add(
                    egui::Label::new(
                        RichText::new("Recommended when a drive is failing (clicking, vanishing, very slow) or simply too precious to work on directly. The drive is read once from start to end. Sectors that cannot be read are filled with zeros and listed, and the copy continues.")
                            .size(13.0)
                            .color(p.muted),
                    )
                    .wrap(),
                );
                ui.add_space(2.0);
                let running = matches!(self.imaging.phase, ImagingPhase::Running { .. }) || self.imaging.fake.is_some();
                ui.add_enabled_ui(!running, |ui| {
                    ui.label(RichText::new("DRIVE TO COPY").font(semibold(11.5)).color(p.faint));
                    let current = self
                        .imaging
                        .source
                        .and_then(|i| self.sources.get(i))
                        .map(|s| format!("{}  ·  {}", s.title, fmt_size(s.total)))
                        .unwrap_or_else(|| "Choose a drive or disk".to_string());
                    egui::ComboBox::from_id_salt("image-source")
                        .selected_text(current)
                        .width(ui.available_width().min(560.0))
                        .show_ui(ui, |ui| {
                            for (i, s) in self.sources.iter().enumerate() {
                                if s.kind == SrcKind::Image {
                                    continue;
                                }
                                let label = format!("{}  ·  {}", s.title, s.summary());
                                if ui.selectable_label(self.imaging.source == Some(i), label).clicked() {
                                    self.imaging.source = Some(i);
                                    rejudge = true;
                                }
                            }
                        });
                    ui.add_space(2.0);
                    ui.label(RichText::new("SAVE THE IMAGE AS").font(semibold(11.5)).color(p.faint));
                    ui.allocate_ui_with_layout(
                        vec2(ui.available_width().min(700.0), 36.0),
                        Layout::right_to_left(Align::Center),
                        |ui| {
                            if widgets::ghost_button(ui, &Icon::FOLDER_OPEN.with("Browse")).clicked() {
                                browse_dest = true;
                            }
                            let edit = egui::TextEdit::singleline(&mut self.imaging.dest)
                                .hint_text(RichText::new("A file on a different drive with enough free space").color(p.faint))
                                .desired_width(ui.available_width())
                                .margin(Margin::symmetric(10, 8))
                                .font(regular(14.0));
                            if ui.add(edit).changed() {
                                rejudge = true;
                            }
                        },
                    );
                });
                let picked = self.imaging.source.and_then(|i| self.sources.get(i)).cloned();
                let has_dest = !self.imaging.dest.trim().is_empty();
                let mut ready = picked.is_some() && has_dest && !running;
                if let (Some(src), true) = (&picked, has_dest) {
                    match &self.imaging.verdict {
                        Verdict::Blocked(why) => {
                            widgets::notice(ui, Icon::ERROR, p.danger, &why.replace("This folder", "This file").replace("recovering from", "copying"));
                            ready = false;
                        }
                        Verdict::Warn(why) => {
                            widgets::notice(ui, Icon::WARNING, p.warn, &why.replace("This folder", "This file"));
                            widgets::checkbox(ui, &mut self.imaging.confirmed, "I understand. Save there anyway.");
                            ready &= self.imaging.confirmed;
                        }
                        Verdict::Ok => match self.imaging.info.free {
                            Some(free) if free < src.total => {
                                widgets::notice(
                                    ui,
                                    Icon::ERROR,
                                    p.danger,
                                    &format!(
                                        "The image needs {} but only {} is free on {}.",
                                        fmt_size(src.total),
                                        fmt_size(free),
                                        self.imaging.info.root.trim_end_matches('\\')
                                    ),
                                );
                                ready = false;
                            }
                            Some(free) => widgets::notice(
                                ui,
                                Icon::CHECK,
                                p.good,
                                &format!(
                                    "The image will be {} large. {} is free on {}.",
                                    fmt_size(src.total),
                                    fmt_size(free),
                                    self.imaging.info.root.trim_end_matches('\\')
                                ),
                            ),
                            None => {}
                        },
                    }
                }
                // Progress or result.
                let mut dismiss = false;
                match (&self.imaging.fake, &self.imaging.phase) {
                    (Some(f), _) => {
                        ui.add_space(2.0);
                        ui.label(RichText::new(&f.phase).font(semibold(14.0)));
                        widgets::progress_bar(ui, f.fraction, p.accent);
                        ui.add(egui::Label::new(RichText::new(&f.detail).size(12.5).color(p.muted)).wrap());
                        ui.horizontal(|ui| {
                            let _ = widgets::ghost_button(ui, &Icon::PAUSE.with("Pause"));
                            let _ = widgets::ghost_button(ui, &Icon::STOP.with("Stop"));
                        });
                    }
                    (None, ImagingPhase::Running { ctl, progress, meter, source, .. }) => {
                        let paused = ctl.is_paused();
                        ui.add_space(2.0);
                        ui.label(RichText::new(if paused { "Paused".to_string() } else { progress.phase() }).font(semibold(14.0)));
                        widgets::progress_bar(ui, progress.fraction(), if paused { p.muted } else { p.accent });
                        let mut detail = Self::progress_text(progress, meter, paused);
                        let bad = source.log().bad_bytes();
                        if bad > 0 {
                            detail.push_str(&format!(
                                " · {} unreadable area{} ({}) filled with zeros",
                                fmt_count(source.log().ranges().len() as u64),
                                if source.log().ranges().len() == 1 { "" } else { "s" },
                                fmt_size(bad)
                            ));
                        }
                        ui.add(egui::Label::new(RichText::new(detail).size(12.5).color(p.muted)).wrap());
                        ui.horizontal(|ui| {
                            let label = if paused { Icon::PLAY.with("Resume") } else { Icon::PAUSE.with("Pause") };
                            if widgets::ghost_button(ui, &label).clicked() {
                                ctl.set_paused(!paused);
                            }
                            if widgets::ghost_button(ui, &Icon::STOP.with("Stop")).on_hover_text("Stop and delete the incomplete image").clicked() {
                                ctl.stop();
                            }
                        });
                        ctx.request_repaint_after(std::time::Duration::from_millis(250));
                    }
                    (None, ImagingPhase::Done(report)) => {
                        let text = if report.bad_bytes == 0 {
                            format!("Image created: {} ({}). Every sector could be read.", report.path.display(), fmt_size(report.bytes))
                        } else {
                            format!(
                                "Image created: {} ({}). {} in {} area{} could not be read and is zeros in the image; files stored there will be damaged.",
                                report.path.display(),
                                fmt_size(report.bytes),
                                fmt_size(report.bad_bytes),
                                fmt_count(report.bad_areas as u64),
                                if report.bad_areas == 1 { "" } else { "s" }
                            )
                        };
                        widgets::notice(ui, Icon::DONE, if report.bad_bytes == 0 { p.good } else { p.warn }, &text);
                        ui.horizontal(|ui| {
                            if widgets::primary_button(ui, &Icon::SCAN.with("Scan this image"), true).clicked() {
                                scan_image = Some(report.path.to_string_lossy().into_owned());
                            }
                            if widgets::ghost_button(ui, "Create another").clicked() {
                                dismiss = true;
                            }
                        });
                    }
                    (None, ImagingPhase::Failed(e)) => {
                        widgets::notice(ui, Icon::ERROR, p.danger, e);
                        if widgets::ghost_button(ui, "OK").clicked() {
                            dismiss = true;
                        }
                    }
                    (None, ImagingPhase::Setup) => {
                        ui.add_space(2.0);
                        if widgets::primary_button(ui, &Icon::IMAGE_FILE.with("Create image"), ready).clicked() {
                            start = true;
                        }
                    }
                }
                if dismiss {
                    self.imaging.phase = ImagingPhase::Setup;
                }
            });

            widgets::card(ui, |ui| {
                widgets::section(ui, "Scan an existing image");
                ui.add(
                    egui::Label::new(
                        RichText::new("Raw images (.img, .dd, .bin, .raw), fixed-size .vhd files and .iso files can be opened like a drive. Scanning an image does not need administrator rights.")
                            .size(13.0)
                            .color(p.muted),
                    )
                    .wrap(),
                );
                ui.horizontal(|ui| {
                    if widgets::ghost_button(ui, &Icon::OPEN.with("Open an image file")).clicked() {
                        open_image = true;
                    }
                });
                let recent: Vec<(String, String)> = self
                    .sources
                    .iter()
                    .filter(|s| s.kind == SrcKind::Image)
                    .map(|s| (s.device.clone(), format!("{}  ·  {}", s.detail, fmt_size(s.total))))
                    .collect();
                if !recent.is_empty() {
                    ui.label(RichText::new("RECENT").font(semibold(11.5)).color(p.faint));
                    for (device, label) in recent {
                        ui.horizontal(|ui| {
                            ui.label(RichText::new(Icon::IMAGE_FILE.glyph()).color(p.muted));
                            ui.add(egui::Label::new(label).truncate());
                            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                                if widgets::ghost_button(ui, "Select").clicked() {
                                    scan_image = Some(device.clone());
                                }
                            });
                        });
                    }
                }
            });

            widgets::notice(
                ui,
                Icon::BULB,
                p.info,
                "An image is as large as the drive itself, whatever is stored on it. Keep it until you are sure everything has been recovered: you can scan it as often as you like, with different settings, without touching the original drive again.",
            );
        });

        if browse_dest {
            if self.demo {
                self.toast(
                    "Demo mode: type a file name, for example F:\\Images\\card.img.",
                    false,
                );
            } else {
                let name = self
                    .imaging
                    .source
                    .and_then(|i| self.sources.get(i))
                    .map(|s| crate::util::sanitize_name(&s.title))
                    .unwrap_or_else(|| "drive".into());
                if let Some(path) = rfd::FileDialog::new()
                    .set_title("Save the image as")
                    .set_file_name(format!("{name}.img"))
                    .add_filter("Disk image", &["img"])
                    .save_file()
                {
                    self.imaging.dest = path.to_string_lossy().into_owned();
                    rejudge = true;
                }
            }
        }
        if rejudge {
            self.judge_image();
        }
        if start {
            self.judge_image_keep_confirmation();
            self.start_imaging(&ctx);
        }
        if open_image {
            self.browse_image();
        }
        if let Some(path) = scan_image {
            if self.demo {
                self.picked = self.sources.iter().position(|s| s.kind == SrcKind::Image);
                self.tab = Tab::Recover;
            } else if self.add_image(&path).is_some() {
                self.imaging.phase =
                    match std::mem::replace(&mut self.imaging.phase, ImagingPhase::Setup) {
                        running @ ImagingPhase::Running { .. } => running,
                        _ => ImagingPhase::Setup,
                    };
                self.tab = Tab::Recover;
            }
        }
    }

    /// Re-checks the destination right before starting (the drive list may
    /// have changed) without throwing away the user's confirmation.
    fn judge_image_keep_confirmation(&mut self) {
        let confirmed = self.imaging.confirmed;
        self.judge_image();
        self.imaging.confirmed = confirmed;
    }

    // ----------------------------------------------------------------------------------
    // Scan log
    // ----------------------------------------------------------------------------------

    pub(super) fn log_ui(&mut self, ui: &mut Ui) {
        let p = pal(ui);
        let (lines, bad): (Vec<String>, Vec<(u64, u64)>) = match &self.ses {
            Some(s) => (
                s.log.clone(),
                s.source
                    .as_ref()
                    .map(|x| x.log().ranges())
                    .unwrap_or_default(),
            ),
            None => (Vec::new(), Vec::new()),
        };
        // The facts of the scan at a glance.
        let summary: Vec<(&str, String)> = match &self.ses {
            Some(s) => {
                use crate::model::{Origin, Status};
                let count = |f: &dyn Fn(&crate::model::FoundFile) -> bool| {
                    fmt_count(s.files.iter().filter(|x| f(x)).count() as u64)
                };
                vec![
                    (
                        "Source",
                        format!("{} · {}", s.label, fmt_size(s.source_len)),
                    ),
                    (
                        "Scan",
                        format!(
                            "{}{}",
                            if s.options.deep {
                                "Deep scan"
                            } else {
                                "Quick scan"
                            },
                            if s.scanning() || self.fake_scan.is_some() {
                                ", still running".to_string()
                            } else if s.stopped {
                                ", stopped early".to_string()
                            } else {
                                match s.elapsed_secs {
                                    Some(t) => {
                                        format!(", finished in {}", crate::util::fmt_duration(t))
                                    }
                                    None => ", opened from a saved session".to_string(),
                                }
                            }
                        ),
                    ),
                    ("Files found", fmt_count(s.files.len() as u64)),
                    (
                        "With name and folder",
                        count(&|f| f.origin != Origin::Carved),
                    ),
                    ("By content only", count(&|f| f.origin == Origin::Carved)),
                    ("Deleted", count(&|f| f.status == Status::Deleted)),
                    (
                        "From lost partitions or an earlier file system",
                        count(&|f| f.status == Status::Lost),
                    ),
                    (
                        "Rated good",
                        count(&|f| f.chance == crate::model::Chance::Good),
                    ),
                    (
                        "Unreadable on the source",
                        if bad.is_empty() {
                            "Nothing: every sector that was needed could be read".to_string()
                        } else {
                            fmt_size(bad.iter().map(|b| b.1).sum())
                        },
                    ),
                ]
            }
            None => Vec::new(),
        };
        let mut copy = false;
        Self::page_header(
            ui,
            "Scan log",
            "What the scan noticed about the drive: file systems, partitions, unreadable areas",
            |ui| {
                if !lines.is_empty()
                    && widgets::ghost_button(ui, &Icon::COPY.with("Copy")).clicked()
                {
                    copy = true;
                }
            },
        );
        if lines.is_empty() && bad.is_empty() {
            widgets::empty_state(
                ui,
                Icon::LOG,
                p.muted,
                "Nothing logged yet",
                "Start a scan. Notes about the file systems, partitions and read errors it meets are collected here.",
            );
            return;
        }
        let mut text = lines.join("\r\n");
        egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
            ui.set_max_width(900.0);
            ui.spacing_mut().item_spacing.y = 12.0;
            widgets::card(ui, |ui| {
                widgets::section(ui, "Summary");
                ui.add_space(2.0);
                egui::Grid::new("log-summary")
                    .num_columns(2)
                    .spacing([28.0, 8.0])
                    .show(ui, |ui| {
                        for (label, value) in &summary {
                            ui.label(RichText::new(*label).color(p.muted));
                            ui.label(value.as_str());
                            ui.end_row();
                        }
                    });
            });
            widgets::card(ui, |ui| {
                widgets::section(ui, "Notes");
                for line in &lines {
                    ui.horizontal_top(|ui| {
                        ui.label(RichText::new("•").color(p.accent));
                        ui.add(egui::Label::new(line.as_str()).wrap().selectable(true));
                    });
                }
            });
            if !bad.is_empty() {
                widgets::card(ui, |ui| {
                    let total: u64 = bad.iter().map(|b| b.1).sum();
                    widgets::section(ui, "Unreadable areas");
                    ui.add(
                        egui::Label::new(
                            RichText::new(format!(
                                "{} in {} area{} could not be read, even after retrying. They were treated as zeros. Files that used this space will be damaged.",
                                fmt_size(total),
                                fmt_count(bad.len() as u64),
                                if bad.len() == 1 { "" } else { "s" }
                            ))
                            .color(p.muted),
                        )
                        .wrap(),
                    );
                    text.push_str("\r\n\r\nUnreadable areas (byte offset, length):");
                    for (off, len) in bad.iter().take(200) {
                        ui.label(
                            RichText::new(format!("at byte {}: {}", fmt_count(*off), fmt_size(*len)))
                                .font(egui::FontId::monospace(12.5)),
                        );
                        text.push_str(&format!("\r\n{off}\t{len}"));
                    }
                    if bad.len() > 200 {
                        ui.label(RichText::new(format!("and {} more", fmt_count(bad.len() as u64 - 200))).color(p.faint));
                    }
                });
            }
        });
        if copy {
            ui.ctx().copy_text(text);
            self.toast("Log copied.", false);
        }
    }

    // ----------------------------------------------------------------------------------
    // Settings
    // ----------------------------------------------------------------------------------

    pub(super) fn settings_ui(&mut self, ui: &mut Ui) {
        let p = pal(ui);
        Self::page_header(ui, "Settings", "Saved automatically", |_| {});
        let before = self.settings.clone();
        let (mut about, mut welcome, mut shortcuts, mut open_data) = (false, false, false, false);
        let data_dir = crate::sys::app_dir().to_string_lossy().into_owned();
        egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
            ui.set_max_width(860.0);
            ui.spacing_mut().item_spacing.y = 12.0;
            let row = |ui: &mut Ui, title: &str, text: &str, control: &mut dyn FnMut(&mut Ui)| {
                ui.horizontal(|ui| {
                    ui.vertical(|ui| {
                        ui.set_width((ui.available_width() - 290.0).max(200.0));
                        ui.spacing_mut().item_spacing.y = 1.0;
                        ui.label(RichText::new(title).font(regular(14.5)));
                        ui.add(egui::Label::new(RichText::new(text).size(12.5).color(p.muted)).wrap());
                    });
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| control(ui));
                });
            };
            let s = &mut self.settings;

            widgets::card(ui, |ui| {
                widgets::section(ui, "Appearance");
                ui.add_space(2.0);
                row(ui, "Theme", "\"System\" follows the Windows light or dark setting.", &mut |ui| {
                    let options = [ThemeChoice::Dark, ThemeChoice::Light, ThemeChoice::System];
                    let current = options.iter().position(|t| *t == s.theme).unwrap_or(0);
                    if let Some(i) = widgets::segmented(ui, &["Dark", "Light", "System"], current) {
                        s.theme = options[i];
                    }
                });
                widgets::divider(ui);
                row(
                    ui,
                    "Picture previews",
                    "Decode found pictures for thumbnails and the preview panel. Switch off if a damaged picture slows the list down.",
                    &mut |ui| {
                        if widgets::toggle(ui, s.thumbnails, true).clicked() {
                            s.thumbnails = !s.thumbnails;
                        }
                    },
                );
            });

            widgets::card(ui, |ui| {
                widgets::section(ui, "Scanning");
                ui.add_space(2.0);
                row(
                    ui,
                    "Kind of scan offered first",
                    "A quick scan reads the drive's own records. A deep scan also reads every sector.",
                    &mut |ui| {
                        if let Some(i) = widgets::segmented(ui, &["Quick scan", "Deep scan"], s.deep_default as usize) {
                            s.deep_default = i == 1;
                        }
                    },
                );
                widgets::divider(ui);
                row(
                    ui,
                    "Deep scan: search free space only",
                    "Skips space the drive's file system reports as in use by existing files. Faster, with fewer duplicates. Switch off if the file system itself is damaged.",
                    &mut |ui| {
                        if widgets::toggle(ui, s.free_only, true).clicked() {
                            s.free_only = !s.free_only;
                        }
                    },
                );
                widgets::divider(ui);
                row(
                    ui,
                    "Deep scan: look for lost partitions",
                    "Recognises volumes that are no longer in the partition table by their boot sectors and lists their files with names and folders.",
                    &mut |ui| {
                        if widgets::toggle(ui, s.find_partitions, true).clicked() {
                            s.find_partitions = !s.find_partitions;
                        }
                    },
                );
                widgets::divider(ui);
                row(
                    ui,
                    "Also list files that were not deleted",
                    "Useful when a drive is hard to open in Windows and you want everything off it.",
                    &mut |ui| {
                        if widgets::toggle(ui, s.include_existing, true).clicked() {
                            s.include_existing = !s.include_existing;
                        }
                    },
                );
            });

            widgets::card(ui, |ui| {
                ui.allocate_ui_with_layout(
                    vec2(ui.available_width(), 30.0),
                    Layout::left_to_right(Align::Center),
                    |ui| {
                        widgets::section(ui, "File types the deep scan looks for");
                        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                            if widgets::ghost_button(ui, "Defaults").clicked() {
                                s.carve_off = crate::carve::default_off();
                            }
                            if widgets::ghost_button(ui, "None").clicked() {
                                s.carve_off = TYPES.iter().map(|t| t.id.to_string()).collect();
                            }
                            if widgets::ghost_button(ui, "All").clicked() {
                                s.carve_off.clear();
                            }
                        });
                    },
                );
                ui.add(
                    egui::Label::new(
                        RichText::new("Files found through the drive's own records are always listed, whatever their type. This list only controls which types are recognised by content.")
                            .size(12.5)
                            .color(p.muted),
                    )
                    .wrap(),
                );
                for cat in Category::ALL {
                    let types: Vec<_> = TYPES.iter().filter(|t| t.cat == cat).collect();
                    if types.is_empty() {
                        continue;
                    }
                    ui.add_space(2.0);
                    ui.label(RichText::new(cat.label().to_uppercase()).font(semibold(11.5)).color(p.faint));
                    ui.horizontal_wrapped(|ui| {
                        ui.spacing_mut().item_spacing = vec2(18.0, 4.0);
                        for t in types {
                            let mut on = !s.carve_off.iter().any(|x| x == t.id);
                            if widgets::checkbox(ui, &mut on, t.label).changed() {
                                s.carve_off.retain(|x| x != t.id);
                                if !on {
                                    s.carve_off.push(t.id.to_string());
                                }
                            }
                        }
                    });
                }
            });

            widgets::card(ui, |ui| {
                widgets::section(ui, "Recovering");
                ui.add_space(2.0);
                row(
                    ui,
                    "Recreate the original folders",
                    "Files keep their place below the folder you recover to. Switched off, everything lands in one folder.",
                    &mut |ui| {
                        if widgets::toggle(ui, s.keep_folders, true).clicked() {
                            s.keep_folders = !s.keep_folders;
                        }
                    },
                );
                widgets::divider(ui);
                row(
                    ui,
                    "Check every recovered file",
                    "Reads each file back after writing it and compares it with what was read from the source.",
                    &mut |ui| {
                        if widgets::toggle(ui, s.verify, true).clicked() {
                            s.verify = !s.verify;
                        }
                    },
                );
            });

            widgets::card(ui, |ui| {
                widgets::section(ui, "Help and about");
                ui.add_space(2.0);
                row(ui, "How to recover safely", "The three rules shown when RescueForge first started.", &mut |ui| {
                    if widgets::ghost_button(ui, &Icon::BULB.with("Show")).clicked() {
                        welcome = true;
                    }
                });
                widgets::divider(ui);
                row(ui, "Keyboard shortcuts", "Search, select and recover without the mouse.", &mut |ui| {
                    if widgets::ghost_button(ui, &Icon::KEYBOARD.with("Show (F1)")).clicked() {
                        shortcuts = true;
                    }
                });
                widgets::divider(ui);
                row(ui, "Settings file", &data_dir, &mut |ui| {
                    if widgets::ghost_button(ui, &Icon::FOLDER_OPEN.with("Open folder")).clicked() {
                        open_data = true;
                    }
                });
                widgets::divider(ui);
                row(
                    ui,
                    "RescueForge",
                    &format!("Version {} · Developed by Zheer Khalid (zheer.dev) · MIT License", env!("CARGO_PKG_VERSION")),
                    &mut |ui| {
                        if widgets::ghost_button(ui, &Icon::INFO.with("About")).clicked() {
                            about = true;
                        }
                    },
                );
            });
            ui.add_space(8.0);
        });
        if before != self.settings {
            if before.theme != self.settings.theme {
                theme::apply_choice(ui.ctx(), self.settings.theme);
            }
            if before.deep_default != self.settings.deep_default {
                self.deep = self.settings.deep_default;
            }
            self.save_settings();
        }
        if about {
            self.about_open = true;
        }
        if welcome {
            self.welcome_open = true;
        }
        if shortcuts {
            self.shortcuts_open = true;
        }
        if open_data {
            if self.demo {
                self.toast("Demo mode: settings are not stored.", false);
            } else {
                let _ = std::fs::create_dir_all(&data_dir);
                if let Err(e) = crate::sys::open_path(&data_dir) {
                    self.toast(e, true);
                }
            }
        }
    }
}
