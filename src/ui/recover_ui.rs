//! The recovery dialog: where to save, the same-drive guard, progress and
//! the summary of what came back.

use super::app::{modal_frame, App, RecoverPhase, RecoverUi};
use super::state::Meter;
use super::theme::{pal_ctx, regular, semibold, Icon};
use super::widgets;
use crate::control::{Control, Progress};
use crate::recover::{self, Outcome, RecoverOptions, Verdict};
use crate::util::{fmt_count, fmt_size};
use eframe::egui::{self, vec2, Align2, CornerRadius, Frame, Margin, RichText, Sense};
use std::sync::atomic::Ordering;
use std::sync::mpsc::channel;
use std::sync::Arc;

impl App {
    /// Opens the dialog for the ticked files, or for one file.
    pub(super) fn open_recover_dialog(&mut self, single: Option<u32>) {
        let Some(ses) = &self.ses else { return };
        if ses.source.is_none() {
            self.toast(
                "The scanned drive is not available, so nothing can be recovered right now.",
                true,
            );
            return;
        }
        let indices = match single {
            Some(i) => vec![i],
            None => ses.selected_indices(),
        };
        if indices.is_empty() {
            self.toast("Tick the files you want to recover first.", false);
            return;
        }
        let bytes = indices.iter().map(|i| ses.files[*i as usize].size).sum();
        let dest = self.settings.last_destination.clone();
        let mut ui = RecoverUi {
            indices,
            bytes,
            dest,
            info: Default::default(),
            verdict: Verdict::Ok,
            confirmed: false,
            phase: RecoverPhase::Setup,
            fake: None,
        };
        self.judge_recover(&mut ui);
        self.recover = Some(ui);
    }

    fn judge_recover(&self, ui: &mut RecoverUi) {
        ui.confirmed = false;
        if ui.dest.trim().is_empty() {
            ui.info = Default::default();
            ui.verdict = Verdict::Ok;
            return;
        }
        ui.info = self.dest_info(ui.dest.trim());
        ui.verdict = match &self.ses {
            Some(s) => recover::judge(&s.spec, &s.disks, &ui.info),
            None => Verdict::Ok,
        };
    }

    fn start_recovery(&mut self, ctx: &egui::Context) {
        let Some(ui) = &mut self.recover else { return };
        if self.demo {
            self.toasts.push(super::app::Toast {
                text: "Demo mode: nothing is written. In the real program the files would now be copied.".into(),
                error: false,
                born: std::time::Instant::now(),
            });
            return;
        }
        let Some(ses) = &self.ses else { return };
        let Some(src) = ses.source.clone() else {
            return;
        };
        let files: Vec<_> = ui
            .indices
            .iter()
            .filter_map(|i| ses.files.get(*i as usize).cloned())
            .collect();
        let dest = std::path::PathBuf::from(ui.dest.trim());
        let opts = RecoverOptions {
            keep_folders: self.settings.keep_folders,
            verify: self.settings.verify,
        };
        let ctl = Arc::new(Control::new());
        let progress = Arc::new(Progress::new());
        let (tx, rx) = channel();
        let (c, p, wake) = (ctl.clone(), progress.clone(), ctx.clone());
        std::thread::Builder::new()
            .name("recover".into())
            .spawn(move || {
                let report = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    recover::recover(&src, &files, &dest, opts, &c, &p)
                }))
                .unwrap_or_else(|_| recover::Report {
                    dest: dest.clone(),
                    total: files.len() as u64,
                    failed: files.len() as u64,
                    stopped: true,
                    ..Default::default()
                });
                let _ = tx.send(report);
                wake.request_repaint();
            })
            .expect("recover thread");
        ui.phase = RecoverPhase::Running {
            ctl,
            progress,
            rx,
            meter: Meter::default(),
        };
        self.settings.last_destination = ui.dest.trim().to_string();
        self.save_settings();
    }

    pub(super) fn poll_recover(&mut self, ctx: &egui::Context) {
        let Some(ui) = &mut self.recover else { return };
        let RecoverPhase::Running {
            rx,
            progress,
            meter,
            ctl,
        } = &mut ui.phase
        else {
            return;
        };
        if ctl.is_paused() {
            meter.reset();
        } else {
            meter.sample(progress.done.load(Ordering::Relaxed));
        }
        ctx.request_repaint_after(std::time::Duration::from_millis(200));
        if let Ok(report) = rx.try_recv() {
            ui.phase = RecoverPhase::Done(report);
        }
    }

    pub(super) fn recover_dialog(&mut self, ctx: &egui::Context) {
        let Some(mut ui_state) = self.recover.take() else {
            return;
        };
        let p = pal_ctx(ctx);
        let (mut close, mut start, mut browse, mut rejudge, mut open_folder) =
            (false, false, false, false, false);
        let modal = egui::Modal::new(egui::Id::new("recover"))
            .frame(modal_frame(ctx, 24))
            .show(ctx, |ui| {
                ui.set_width(580.0);
                let n = ui_state.indices.len() as u64;
                let files = format!("{} file{}", fmt_count(n), if n == 1 { "" } else { "s" });
                match &ui_state.phase {
                    RecoverPhase::Setup if ui_state.fake.is_none() => {
                        ui.label(RichText::new(format!("Recover {files}")).font(semibold(21.0)));
                        ui.add_space(-2.0);
                        ui.label(
                            RichText::new(format!("{} will be copied to the folder you choose.", fmt_size(ui_state.bytes)))
                                .color(p.muted),
                        );
                        ui.add_space(8.0);
                        ui.label(RichText::new("SAVE TO").font(semibold(11.5)).color(p.faint));
                        ui.allocate_ui_with_layout(
                            vec2(ui.available_width(), 36.0),
                            egui::Layout::right_to_left(egui::Align::Center),
                            |ui| {
                                if widgets::ghost_button(ui, &Icon::FOLDER_OPEN.with("Browse")).clicked() {
                                    browse = true;
                                }
                                let edit = egui::TextEdit::singleline(&mut ui_state.dest)
                                    .hint_text(RichText::new("A folder on a different drive").color(p.faint))
                                    .desired_width(ui.available_width())
                                    .margin(Margin::symmetric(10, 8))
                                    .font(regular(14.0));
                                if ui.add(edit).changed() {
                                    rejudge = true;
                                }
                            },
                        );
                        ui.add_space(4.0);
                        let empty = ui_state.dest.trim().is_empty();
                        match &ui_state.verdict {
                            _ if empty => widgets::notice(
                                ui,
                                Icon::INFO,
                                p.info,
                                "Choose a folder on another drive, a USB stick or a network share. Recovered files are never written to the drive that is being recovered.",
                            ),
                            Verdict::Blocked(why) => {
                                widgets::notice(ui, Icon::ERROR, p.danger, why);
                            }
                            Verdict::Warn(why) => {
                                widgets::notice(ui, Icon::WARNING, p.warn, why);
                                ui.add_space(2.0);
                                widgets::checkbox(ui, &mut ui_state.confirmed, "I understand. Save there anyway.");
                            }
                            Verdict::Ok => {
                                let free = ui_state.info.free;
                                match free {
                                    Some(f) if f < ui_state.bytes => widgets::notice(
                                        ui,
                                        Icon::WARNING,
                                        p.warn,
                                        &format!(
                                            "Only {} is free on {} but the selection needs {}. Files that do not fit will be reported as failed.",
                                            fmt_size(f),
                                            ui_state.info.root,
                                            fmt_size(ui_state.bytes)
                                        ),
                                    ),
                                    _ => widgets::notice(
                                        ui,
                                        Icon::CHECK,
                                        p.good,
                                        &match free {
                                            Some(f) => format!(
                                                "This folder is on a different drive ({}, {} free). Safe to recover to.",
                                                ui_state.info.root.trim_end_matches('\\'),
                                                fmt_size(f)
                                            ),
                                            None => "This folder is not on the drive being recovered. Safe to recover to.".to_string(),
                                        },
                                    ),
                                }
                            }
                        }
                        ui.add_space(8.0);
                        let before = (self.settings.keep_folders, self.settings.verify);
                        widgets::checkbox(ui, &mut self.settings.keep_folders, "Recreate the original folders");
                        widgets::checkbox(ui, &mut self.settings.verify, "Read every recovered file back to check it");
                        if before != (self.settings.keep_folders, self.settings.verify) {
                            self.save_settings();
                        }
                        ui.add_space(12.0);
                        let ready = !empty
                            && match &ui_state.verdict {
                                Verdict::Ok => true,
                                Verdict::Warn(_) => ui_state.confirmed,
                                Verdict::Blocked(_) => false,
                            };
                        widgets::button_row(ui, |ui| {
                            if widgets::primary_button(ui, &Icon::RECOVER.with("Recover"), ready).clicked() {
                                start = true;
                            }
                            if widgets::ghost_button(ui, "Cancel").clicked() {
                                close = true;
                            }
                        });
                    }
                    RecoverPhase::Setup | RecoverPhase::Running { .. } => {
                        let (fraction, phase, detail) = match (&ui_state.fake, &ui_state.phase) {
                            (Some(f), _) => (f.fraction, f.phase.clone(), f.detail.clone()),
                            (None, RecoverPhase::Running { progress, meter, .. }) => (
                                progress.fraction(),
                                progress.phase(),
                                format!(
                                    "{} of {} files · {}",
                                    fmt_count(progress.found.load(Ordering::Relaxed)),
                                    fmt_count(n),
                                    Self::progress_text(progress, meter, false)
                                ),
                            ),
                            _ => (0.0, String::new(), String::new()),
                        };
                        ui.label(RichText::new(format!("Recovering {files}")).font(semibold(21.0)));
                        ui.add_space(-2.0);
                        ui.add(
                            egui::Label::new(RichText::new(format!("To {}", ui_state.dest.trim())).color(p.muted))
                                .truncate(),
                        );
                        ui.add_space(12.0);
                        ui.add(egui::Label::new(RichText::new(phase).font(semibold(14.0))).truncate());
                        widgets::progress_bar(ui, fraction, p.accent);
                        ui.add(egui::Label::new(RichText::new(detail).size(12.5).color(p.muted)).truncate());
                        ui.add_space(12.0);
                        widgets::button_row(ui, |ui| {
                            if widgets::ghost_button(ui, &Icon::STOP.with("Stop")).clicked() {
                                if let RecoverPhase::Running { ctl, .. } = &ui_state.phase {
                                    ctl.stop();
                                }
                                if ui_state.fake.is_some() {
                                    close = true;
                                }
                            }
                        });
                    }
                    RecoverPhase::Done(report) => {
                        let good = report.ok + report.damaged;
                        let (icon, color, title) = if report.failed == 0 && report.damaged == 0 && !report.stopped {
                            (Icon::DONE, p.good, format!("{} recovered", files_text(report.ok)))
                        } else if good > 0 {
                            (
                                Icon::WARNING,
                                p.warn,
                                format!("{} of {} recovered", fmt_count(good), files_text(report.total)),
                            )
                        } else {
                            (Icon::ERROR, p.danger, "Nothing was recovered".to_string())
                        };
                        ui.horizontal(|ui| {
                            let (rect, _) = ui.allocate_exact_size(vec2(44.0, 44.0), Sense::hover());
                            ui.painter().circle_filled(rect.center(), 22.0, color.gamma_multiply(0.16));
                            ui.painter().text(rect.center(), Align2::CENTER_CENTER, icon.glyph(), regular(21.0), color);
                            ui.vertical(|ui| {
                                ui.spacing_mut().item_spacing.y = 1.0;
                                ui.label(RichText::new(title).font(semibold(20.0)));
                                ui.add(
                                    egui::Label::new(
                                        RichText::new(format!("{} written to {}", fmt_size(report.bytes), report.dest.display()))
                                            .color(p.muted),
                                    )
                                    .truncate(),
                                );
                            });
                        });
                        ui.add_space(8.0);
                        let line = |ui: &mut egui::Ui, color, n: u64, text: &str| {
                            if n > 0 {
                                ui.horizontal(|ui| {
                                    widgets::badge(ui, &fmt_count(n), color);
                                    ui.add(egui::Label::new(text).wrap());
                                });
                            }
                        };
                        line(ui, p.good, report.ok, if self.settings.verify { "copied completely and read back correctly" } else { "copied completely" });
                        line(ui, p.warn, report.damaged, "written, but with parts missing (probably overwritten or unreadable on the source)");
                        line(ui, p.danger, report.failed, "could not be written");
                        if report.stopped {
                            ui.label(RichText::new("The recovery was stopped before it finished.").color(p.warn));
                        }
                        if !report.problems.is_empty() {
                            ui.add_space(6.0);
                            Frame::new()
                                .fill(p.bg)
                                .corner_radius(CornerRadius::same(10))
                                .inner_margin(Margin::same(12))
                                .show(ui, |ui| {
                                    ui.set_width(ui.available_width());
                                    egui::ScrollArea::vertical()
                                        .id_salt("recover-problems")
                                        .max_height(170.0)
                                        .show(ui, |ui| {
                                            for item in &report.problems {
                                                let (color, why) = match &item.outcome {
                                                    Outcome::Damaged(w) => (p.warn, w.as_str()),
                                                    Outcome::Failed(w) => (p.danger, w.as_str()),
                                                    Outcome::Ok => (p.good, ""),
                                                };
                                                ui.horizontal_top(|ui| {
                                                    ui.label(RichText::new("•").color(color));
                                                    ui.vertical(|ui| {
                                                        ui.spacing_mut().item_spacing.y = 0.0;
                                                        ui.add(egui::Label::new(RichText::new(&item.name).font(semibold(13.5))).truncate());
                                                        ui.add(egui::Label::new(RichText::new(why).size(12.5).color(p.muted)).wrap());
                                                    });
                                                });
                                            }
                                        });
                                });
                        }
                        ui.add_space(12.0);
                        widgets::button_row(ui, |ui| {
                            if widgets::primary_button(ui, "Close", true).clicked() {
                                close = true;
                            }
                            if widgets::ghost_button(ui, &Icon::FOLDER_OPEN.with("Open the folder")).clicked() {
                                open_folder = true;
                            }
                        });
                    }
                }
            });
        let running = matches!(ui_state.phase, RecoverPhase::Running { .. });
        if modal.should_close() && !running {
            close = true;
        }
        if browse && !self.demo {
            if let Some(dir) = rfd::FileDialog::new()
                .set_title("Where should the recovered files go?")
                .pick_folder()
            {
                ui_state.dest = dir.to_string_lossy().into_owned();
                rejudge = true;
            }
        } else if browse {
            self.toast(
                "Demo mode: type a folder, for example F:\\Recovered files.",
                false,
            );
        }
        if rejudge {
            self.judge_recover(&mut ui_state);
        }
        if open_folder {
            if self.demo {
                self.toast("Demo mode: nothing was written.", false);
            } else if let Err(e) = crate::sys::open_path(&ui_state.dest) {
                self.toast(e, true);
            }
        }
        if !close {
            self.recover = Some(ui_state);
            if start {
                self.start_recovery(ctx);
            }
        }
    }
}

fn files_text(n: u64) -> String {
    format!("{} file{}", fmt_count(n), if n == 1 { "" } else { "s" })
}
