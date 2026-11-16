//! Application state, background work plumbing and the window frame
//! (header, navigation rail, dialogs, toasts).

use super::previewer::Previewer;
use super::state::Session;
use super::theme::{self, pal, pal_ctx, regular, semibold, Icon};
use super::widgets;
use crate::imaging::ImageReport;
use crate::model::SourceSpec;
use crate::recover::{Report, Verdict};
use crate::scan::{self, ScanMsg, ScanOptions};
use crate::settings::Settings;
use crate::source::Source;
use crate::sys::{self, DestInfo};
use crate::util::fmt_size;
use crate::{demo, session};
use eframe::egui::{
    self, vec2, Align, Align2, Color32, CornerRadius, Frame, Key, Layout, Margin, Rect, RichText,
    Sense, Stroke, TextureHandle, Ui,
};
use std::path::PathBuf;
use std::sync::mpsc::Receiver;
use std::sync::Arc;
use std::time::Instant;

/// How the app was started.
#[derive(Clone, Debug, Default)]
pub struct Options {
    /// Show an invented disk; nothing on this PC is read.
    pub demo: bool,
    /// Render one frame of the chosen view to this PNG, then exit.
    pub screenshot: Option<PathBuf>,
    /// Which screen to open first (see `--help`).
    pub view: Option<String>,
    /// Force the light theme (for screenshots).
    pub light: bool,
    /// A disk image to open and scan right away.
    pub open: Option<PathBuf>,
    /// Use a deep scan for `open`.
    pub deep: bool,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Tab {
    Recover,
    Results,
    Images,
    Log,
    Settings,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum SrcKind {
    Drive,
    Disk,
    Image,
}

/// One line of the source list.
#[derive(Clone, Debug)]
pub struct SourceEntry {
    pub spec: SourceSpec,
    pub kind: SrcKind,
    pub title: String,
    /// "NTFS", a disk model, or the path of an image.
    pub detail: String,
    pub hardware: String,
    pub total: u64,
    pub free: Option<u64>,
    /// What to open (`\\.\D:`, `\\.\PhysicalDrive1`, or a file path).
    pub device: String,
    pub disks: Vec<u32>,
    pub removable: bool,
    pub ssd_trim: bool,
}

pub struct Toast {
    pub text: String,
    pub error: bool,
    pub born: Instant,
}

struct Shot {
    path: PathBuf,
    frames: u32,
    requested: bool,
}

/// Invented progress numbers for the screenshots of running work.
#[derive(Clone)]
pub struct FakeProgress {
    pub fraction: f32,
    pub phase: String,
    pub detail: String,
}

pub enum RecoverPhase {
    Setup,
    Running {
        ctl: Arc<crate::control::Control>,
        progress: Arc<crate::control::Progress>,
        rx: Receiver<Report>,
        meter: super::state::Meter,
    },
    Done(Report),
}

/// The "Recover" dialog.
pub struct RecoverUi {
    pub indices: Vec<u32>,
    pub bytes: u64,
    pub dest: String,
    pub info: DestInfo,
    pub verdict: Verdict,
    pub confirmed: bool,
    pub phase: RecoverPhase,
    pub fake: Option<FakeProgress>,
}

pub enum ImagingPhase {
    Setup,
    Running {
        ctl: Arc<crate::control::Control>,
        progress: Arc<crate::control::Progress>,
        rx: Receiver<Result<ImageReport, String>>,
        meter: super::state::Meter,
        source: Source,
    },
    Done(ImageReport),
    Failed(String),
}

/// The "create an image" card on the Disk images tab.
pub struct ImagingUi {
    pub source: Option<usize>,
    pub dest: String,
    pub info: DestInfo,
    pub verdict: Verdict,
    pub confirmed: bool,
    pub phase: ImagingPhase,
    pub fake: Option<FakeProgress>,
}

pub struct App {
    pub(super) demo: bool,
    pub(super) elevated: bool,
    pub(super) settings: Settings,
    settings_dir: PathBuf,
    pub(super) tab: Tab,
    pub(super) tz: i32,

    pub(super) sources: Vec<SourceEntry>,
    pub(super) picked: Option<usize>,
    pub(super) deep: bool,
    demo_disk: Arc<std::sync::OnceLock<Source>>,
    /// Settings are written to disk (not in demo mode or when rendering a screenshot).
    persist: bool,

    pub(super) ses: Option<Session>,
    pub(super) search: String,
    pub(super) focus_search: bool,
    pub(super) previewer: Previewer,
    pub(super) fake_scan: Option<FakeProgress>,

    pub(super) recover: Option<RecoverUi>,
    pub(super) imaging: ImagingUi,
    pub(super) about_open: bool,
    pub(super) welcome_open: bool,
    pub(super) shortcuts_open: bool,
    pub(super) big_preview: bool,
    pub(super) toasts: Vec<Toast>,

    pub(super) logo: Option<TextureHandle>,
    shot: Option<Shot>,
}

/// Starts the window.
pub fn run(options: Options, icon: egui::IconData) -> eframe::Result<()> {
    let viewport = egui::ViewportBuilder::default()
        .with_title(if options.demo {
            "RescueForge (demo)"
        } else {
            "RescueForge"
        })
        .with_inner_size([1280.0, 800.0])
        .with_min_inner_size([1040.0, 660.0])
        .with_icon(icon.clone());
    let native = eframe::NativeOptions {
        viewport,
        ..Default::default()
    };
    eframe::run_native(
        "RescueForge",
        native,
        Box::new(move |cc| Ok(Box::new(App::new(cc, options, icon)))),
    )
}

pub(super) fn modal_frame(ctx: &egui::Context, margin: i8) -> Frame {
    let p = pal_ctx(ctx);
    Frame::new()
        .fill(p.card)
        .stroke(Stroke::new(1.0_f32, p.border))
        .corner_radius(CornerRadius::same(14))
        .inner_margin(Margin::same(margin))
}

impl App {
    fn new(cc: &eframe::CreationContext<'_>, options: Options, icon: egui::IconData) -> Self {
        let ctx = &cc.egui_ctx;
        theme::install(ctx);
        let settings_dir = sys::app_dir();
        let mut settings = if options.demo {
            Settings {
                welcome_seen: true,
                ..Default::default()
            }
        } else {
            Settings::load(&settings_dir)
        };
        if options.light {
            settings.theme = crate::settings::ThemeChoice::Light;
        }
        theme::apply_choice(ctx, settings.theme);
        let logo = (icon.width > 0).then(|| {
            ctx.load_texture(
                "logo",
                egui::ColorImage::from_rgba_unmultiplied(
                    [icon.width as usize, icon.height as usize],
                    &icon.rgba,
                ),
                egui::TextureOptions::LINEAR,
            )
        });
        let mut app = App {
            demo: options.demo,
            elevated: options.demo || sys::is_elevated(),
            deep: settings.deep_default,
            welcome_open: !settings.welcome_seen && options.screenshot.is_none(),
            settings,
            settings_dir,
            tab: Tab::Recover,
            tz: if options.demo {
                0
            } else {
                sys::tz_offset_secs()
            },
            sources: Vec::new(),
            picked: None,
            demo_disk: Arc::new(std::sync::OnceLock::new()),
            persist: !options.demo && options.screenshot.is_none(),
            ses: None,
            search: String::new(),
            focus_search: false,
            previewer: Previewer::new(ctx),
            fake_scan: None,
            recover: None,
            imaging: ImagingUi {
                source: None,
                dest: String::new(),
                info: DestInfo::default(),
                verdict: Verdict::Ok,
                confirmed: false,
                phase: ImagingPhase::Setup,
                fake: None,
            },
            about_open: false,
            shortcuts_open: false,
            big_preview: false,
            toasts: Vec::new(),
            logo,
            shot: options.screenshot.clone().map(|path| Shot {
                path,
                frames: 0,
                requested: false,
            }),
        };
        app.load_sources();
        if app.demo {
            // The invented disk takes a few seconds to build: start on it now.
            let disk = app.demo_disk.clone();
            let _ = std::thread::Builder::new()
                .name("demo-disk".into())
                .spawn(move || {
                    disk.get_or_init(|| Source::from_bytes(demo::disk()));
                });
            app.apply_view(options.view.as_deref().unwrap_or("home"));
        } else if let Some(path) = &options.open {
            if let Some(index) = app.add_image(&path.to_string_lossy()) {
                app.deep = options.deep;
                app.start_scan(ctx, index);
            }
        }
        app
    }

    // ----------------------------------------------------------------------------------
    // Sources
    // ----------------------------------------------------------------------------------

    pub(super) fn load_sources(&mut self) {
        let keep = self
            .picked
            .and_then(|i| self.sources.get(i))
            .map(|s| s.spec.clone());
        let mut out = Vec::new();
        if self.demo {
            for (i, d) in demo::sources().into_iter().enumerate() {
                let (spec, kind) = match d.kind {
                    0 => (
                        SourceSpec::Volume {
                            letter: d.letter.to_string(),
                            guid: format!("demo-{i}"),
                        },
                        SrcKind::Drive,
                    ),
                    1 => (
                        SourceSpec::Disk {
                            number: i as u32 - 4,
                        },
                        SrcKind::Disk,
                    ),
                    _ => (
                        SourceSpec::Image {
                            path: d.detail.to_string(),
                        },
                        SrcKind::Image,
                    ),
                };
                out.push(SourceEntry {
                    spec,
                    kind,
                    title: d.title.to_string(),
                    detail: d.detail.to_string(),
                    hardware: d.hardware.to_string(),
                    total: d.total,
                    free: d.free,
                    device: String::new(),
                    disks: vec![match d.letter {
                        "C" => 0,
                        "D" => 1,
                        _ => 2,
                    }],
                    removable: d.removable,
                    ssd_trim: d.ssd_trim,
                });
            }
        } else {
            let hw_text = |hw: &sys::Hardware| {
                let kind = if hw.solid_state {
                    "solid-state drive"
                } else if hw.removable {
                    "removable drive"
                } else {
                    "hard disk"
                };
                if hw.bus.is_empty() {
                    kind.to_string()
                } else {
                    format!("{} {kind}", hw.bus)
                }
            };
            for d in sys::list_drives() {
                out.push(SourceEntry {
                    spec: SourceSpec::Volume {
                        letter: d.letter.clone(),
                        guid: d.guid.clone(),
                    },
                    kind: SrcKind::Drive,
                    title: if d.label.is_empty() {
                        format!("Local disk ({}:)", d.letter)
                    } else {
                        format!("{} ({}:)", d.label, d.letter)
                    },
                    detail: if d.fs.is_empty() {
                        "Not readable by Windows".to_string()
                    } else {
                        d.fs.clone()
                    },
                    hardware: hw_text(&d.hw),
                    total: d.total,
                    free: (d.total > 0).then_some(d.free),
                    device: d.device_path(),
                    disks: d.disks.clone(),
                    removable: d.removable,
                    ssd_trim: d.hw.solid_state && d.hw.trim,
                });
            }
            for d in sys::list_disks() {
                out.push(SourceEntry {
                    spec: SourceSpec::Disk { number: d.number },
                    kind: SrcKind::Disk,
                    title: format!("Disk {}", d.number),
                    detail: if d.hw.model.is_empty() {
                        "Physical disk".to_string()
                    } else {
                        d.hw.model.clone()
                    },
                    hardware: hw_text(&d.hw),
                    total: d.size,
                    free: None,
                    device: d.device_path(),
                    disks: vec![d.number],
                    removable: d.hw.removable,
                    ssd_trim: d.hw.solid_state && d.hw.trim,
                });
            }
            for path in self.settings.recent_images.clone() {
                if let Ok(meta) = std::fs::metadata(&path) {
                    out.push(Self::image_entry(&path, meta.len()));
                }
            }
        }
        self.picked = keep
            .and_then(|k| out.iter().position(|s| s.spec == k))
            .or_else(|| {
                // Start on the first drive that is not the system drive.
                let system = std::env::var("SystemDrive").unwrap_or_else(|_| "C:".into());
                out.iter()
                    .position(|s| matches!(&s.spec, SourceSpec::Volume { letter, .. } if !system.starts_with(letter.as_str())))
                    .or(if out.is_empty() { None } else { Some(0) })
            });
        self.sources = out;
    }

    fn image_entry(path: &str, size: u64) -> SourceEntry {
        let name = std::path::Path::new(path)
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| path.to_string());
        SourceEntry {
            spec: SourceSpec::Image {
                path: path.to_string(),
            },
            kind: SrcKind::Image,
            title: name,
            detail: path.to_string(),
            hardware: "Disk image file".to_string(),
            total: size,
            free: None,
            device: path.to_string(),
            disks: Vec::new(),
            removable: false,
            ssd_trim: false,
        }
    }

    /// Adds an image file to the source list and selects it.
    pub(super) fn add_image(&mut self, path: &str) -> Option<usize> {
        match crate::source::open_image(std::path::Path::new(path)) {
            Ok(src) => {
                self.settings.remember_image(path);
                self.save_settings();
                let spec = SourceSpec::Image {
                    path: path.to_string(),
                };
                let at = match self.sources.iter().position(|s| s.spec == spec) {
                    Some(i) => i,
                    None => {
                        self.sources.push(Self::image_entry(path, src.len()));
                        self.sources.len() - 1
                    }
                };
                self.picked = Some(at);
                Some(at)
            }
            Err(e) => {
                self.toast(e, true);
                None
            }
        }
    }

    pub(super) fn browse_image(&mut self) {
        if self.demo {
            self.toast(
                "Demo mode: the invented image in the list can be scanned.",
                false,
            );
            return;
        }
        let picked = rfd::FileDialog::new()
            .set_title("Open a disk image")
            .add_filter(
                "Disk images",
                &["img", "dd", "bin", "raw", "vhd", "iso", "ima", "dsk"],
            )
            .add_filter("All files", &["*"])
            .pick_file();
        if let Some(path) = picked {
            if self.add_image(&path.to_string_lossy()).is_some() {
                self.tab = Tab::Recover;
            }
        }
    }

    pub(super) fn open_source(&mut self, entry: &SourceEntry) -> Result<Source, String> {
        if self.demo {
            return Ok(self
                .demo_disk
                .get_or_init(|| Source::from_bytes(demo::disk()))
                .clone());
        }
        match entry.kind {
            SrcKind::Image => crate::source::open_image(std::path::Path::new(&entry.device)),
            _ => crate::source::open_device(&entry.device),
        }
    }

    // ----------------------------------------------------------------------------------
    // Scanning
    // ----------------------------------------------------------------------------------

    pub(super) fn scan_options(&self) -> ScanOptions {
        ScanOptions {
            deep: self.deep,
            free_only: self.settings.free_only,
            find_partitions: self.settings.find_partitions,
            include_existing: self.settings.include_existing,
            carve_off: self.settings.carve_off.clone(),
            tz: self.tz,
        }
    }

    pub(super) fn busy(&self) -> bool {
        self.ses.as_ref().map(|s| s.scanning()).unwrap_or(false)
            || matches!(self.imaging.phase, ImagingPhase::Running { .. })
            || matches!(
                self.recover.as_ref().map(|r| &r.phase),
                Some(RecoverPhase::Running { .. })
            )
    }

    pub(super) fn start_scan(&mut self, ctx: &egui::Context, index: usize) {
        let Some(entry) = self.sources.get(index).cloned() else {
            return;
        };
        if self.busy() {
            self.toast(
                "Something is still running. Stop it or wait for it to finish first.",
                true,
            );
            return;
        }
        let source = match self.open_source(&entry) {
            Ok(s) => s,
            Err(e) => {
                self.toast(e, true);
                return;
            }
        };
        let options = self.scan_options();
        self.previewer.reset();
        let mut ses = Session::new(
            entry.spec.clone(),
            entry.title.clone(),
            Some(source.clone()),
            options.clone(),
        );
        ses.disks = entry.disks.clone();
        let wake = ctx.clone();
        ses.scan = Some(scan::start(source, options, move || wake.request_repaint()));
        self.ses = Some(ses);
        self.fake_scan = None;
        self.search.clear();
        self.tab = Tab::Results;
    }

    fn poll_scan(&mut self) {
        let Some(ses) = &mut self.ses else { return };
        // A bounded number of messages per frame keeps the window responsive.
        let messages: Vec<ScanMsg> = match &ses.scan {
            Some(handle) => (0..400).map_while(|_| handle.rx.try_recv().ok()).collect(),
            None => return,
        };
        let mut retracted = false;
        let mut done: Option<bool> = None;
        for m in messages {
            match m {
                ScanMsg::Files(files) => ses.add_files(files),
                ScanMsg::Retract(set) => {
                    ses.retract(&set);
                    retracted = true;
                }
                ScanMsg::Log(line) => ses.log.push(line),
                ScanMsg::Finished { stopped } => done = Some(stopped),
            }
        }
        if let (false, Some(handle)) = (ses.finished, &ses.scan) {
            if handle.ctl.is_paused() {
                ses.meter.reset();
            } else {
                ses.meter.sample(
                    handle
                        .progress
                        .done
                        .load(std::sync::atomic::Ordering::Relaxed),
                );
            }
        }
        if let Some(stopped) = done {
            ses.finished = true;
            ses.stopped = stopped;
            ses.elapsed_secs = Some(ses.started.elapsed().as_secs());
            ses.dirty = true;
            let n = ses.files.len();
            let text = if stopped {
                format!(
                    "Scan stopped. {} files found so far.",
                    crate::util::fmt_count(n as u64)
                )
            } else {
                format!(
                    "Scan finished. {} files found.",
                    crate::util::fmt_count(n as u64)
                )
            };
            self.toast(text, false);
        }
        if retracted {
            self.previewer.reset();
        }
    }

    // ----------------------------------------------------------------------------------
    // Sessions and settings
    // ----------------------------------------------------------------------------------

    /// Settings are not written while the drive they live on is being recovered.
    pub(super) fn save_settings(&mut self) {
        if !self.persist {
            return;
        }
        if let Some(ses) = &self.ses {
            let dest = sys::dest_info(&self.settings_dir);
            if matches!(
                crate::recover::judge(&ses.spec, &ses.disks, &dest),
                Verdict::Blocked(_)
            ) {
                return;
            }
        }
        if let Err(e) = self.settings.save(&self.settings_dir) {
            self.toast(e, true);
        }
    }

    pub(super) fn save_session(&mut self) {
        let Some(ses) = &self.ses else { return };
        if self.demo {
            self.toast("Demo mode: nothing is written.", false);
            return;
        }
        let name = format!(
            "{}.{}",
            crate::util::sanitize_name(&format!("RescueForge scan of {}", ses.label)),
            session::EXTENSION
        );
        let Some(path) = rfd::FileDialog::new()
            .set_title("Save this scan")
            .set_file_name(name)
            .add_filter("RescueForge session", &[session::EXTENSION])
            .save_file()
        else {
            return;
        };
        // The session file is a write like any other: never onto the scanned drive.
        if let Verdict::Blocked(why) =
            crate::recover::judge(&ses.spec, &ses.disks, &sys::dest_info(&path))
        {
            self.toast(format!("The session was not saved. {why}"), true);
            return;
        }
        let saved = session::Session {
            created: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_secs() as i64)
                .unwrap_or(0),
            source: ses.spec.clone(),
            source_label: ses.label.clone(),
            source_len: ses.source_len,
            options: ses.options.clone(),
            finished: ses.finished && !ses.stopped,
            files: ses.files.clone(),
            log: ses.log.clone(),
        };
        match saved.save(&path) {
            Ok(()) => {
                if let Some(s) = &mut self.ses {
                    s.saved = true;
                }
                self.toast(format!("Session saved to {}", path.display()), false);
            }
            Err(e) => self.toast(e, true),
        }
    }

    pub(super) fn open_session(&mut self) {
        if self.demo {
            self.toast("Demo mode: there are no saved sessions to open.", false);
            return;
        }
        if self.busy() {
            self.toast(
                "Something is still running. Stop it or wait for it to finish first.",
                true,
            );
            return;
        }
        let Some(path) = rfd::FileDialog::new()
            .set_title("Open a saved scan")
            .add_filter("RescueForge session", &[session::EXTENSION])
            .pick_file()
        else {
            return;
        };
        match session::Session::load(&path) {
            Ok(saved) => self.adopt_session(saved),
            Err(e) => self.toast(e, true),
        }
    }

    fn adopt_session(&mut self, saved: session::Session) {
        // Find the drive again so that previews and recovery work.
        let entry = match &saved.source {
            SourceSpec::Image { path } => Some(Self::image_entry(path, saved.source_len)),
            spec => {
                self.load_sources();
                self.sources
                    .iter()
                    .find(|s| match (&s.spec, spec) {
                        (
                            SourceSpec::Volume {
                                guid: a,
                                letter: la,
                            },
                            SourceSpec::Volume {
                                guid: b,
                                letter: lb,
                            },
                        ) => (!a.is_empty() && a == b) || (a.is_empty() && la == lb),
                        (a, b) => a == b,
                    })
                    .cloned()
            }
        };
        let (source, note, disks) = match entry {
            None => (
                None,
                "The drive this scan was made of is not connected. The list can be browsed, but previews and recovery need the drive.".to_string(),
                Vec::new(),
            ),
            Some(e) => match self.open_source(&e) {
                Ok(src) if src.len() == saved.source_len => (Some(src), String::new(), e.disks),
                Ok(_) => (
                    None,
                    "The drive now has a different size than when it was scanned, so it is probably not the same one. Previews and recovery are switched off.".to_string(),
                    Vec::new(),
                ),
                Err(err) => (None, format!("The scanned drive could not be opened. {err}"), Vec::new()),
            },
        };
        self.previewer.reset();
        let mut ses = Session::new(saved.source, saved.source_label, source, saved.options);
        ses.source_len = saved.source_len;
        ses.source_note = note;
        ses.disks = disks;
        ses.add_files(saved.files);
        ses.log = saved.log;
        ses.finished = true;
        ses.stopped = !saved.finished;
        ses.saved = true;
        ses.elapsed_secs = None;
        ses.rebuild_now();
        self.ses = Some(ses);
        self.tab = Tab::Results;
        self.toast("Saved scan opened.", false);
    }

    pub(super) fn toast(&mut self, text: impl Into<String>, error: bool) {
        self.toasts.push(Toast {
            text: text.into(),
            error,
            born: Instant::now(),
        });
    }

    pub(super) fn dest_info(&self, path: &str) -> DestInfo {
        if self.demo {
            let root: String = path.chars().take(3).collect::<String>().to_uppercase();
            return DestInfo {
                guid: String::new(),
                disks: vec![if root.starts_with('D') { 1 } else { 2 }],
                free: (root.len() == 3).then_some(212 << 30),
                root: if root.len() == 3 { root } else { String::new() },
            };
        }
        sys::dest_info(std::path::Path::new(path))
    }

    // ----------------------------------------------------------------------------------
    // Demo screens for screenshots
    // ----------------------------------------------------------------------------------

    fn apply_view(&mut self, view: &str) {
        self.picked = Some(1);
        if matches!(view, "home" | "home-quick") {
            self.deep = view == "home";
            return;
        }
        match view {
            "settings" => self.tab = Tab::Settings,
            "about" => self.about_open = true,
            "welcome" => self.welcome_open = true,
            "shortcuts" => self.shortcuts_open = true,
            "images" => self.tab = Tab::Images,
            _ => {}
        }
        if matches!(view, "settings" | "about" | "welcome" | "shortcuts") {
            return;
        }
        // Everything else shows results: run the real scanners on the invented disk.
        let entry = self.sources[1].clone();
        let source = self.open_source(&entry).expect("demo disk");
        let options = ScanOptions {
            deep: true,
            ..Default::default()
        };
        let (files, log) = scan::run_collect(&source, &options);
        let mut ses = Session::new(entry.spec, entry.title, Some(source), options);
        ses.disks = entry.disks;
        ses.add_files(files);
        // The invented drive is far larger than the disk that stands in for it.
        ses.source_len = entry.total;
        ses.log = log
            .into_iter()
            .map(|l| {
                let l = l.replace("128 MB (262,144 sectors)", "932 GB (1,953,525,168 sectors)");
                match l.find(" in free space (") {
                    Some(i) => format!(
                        "{} in free space (719 GB that is in use by existing files was skipped).",
                        &l[..i]
                    ),
                    None => l,
                }
            })
            .collect();
        ses.finished = true;
        ses.elapsed_secs = Some(4 * 3600 + 12 * 60);
        ses.rebuild_now();
        let picture = |ses: &Session, name: &str| {
            ses.files
                .iter()
                .position(|f| f.name == name)
                .map(|i| i as u32)
        };
        // A believable selection.
        let lisbon = ses.dirs.intern("Photos/2025 Summer in Lisbon");
        let thesis = ses.dirs.intern("Documents/Thesis");
        ses.set_folder(lisbon, true);
        ses.set_folder(thesis, true);
        if let Some(i) = picture(&ses, "IMG_2043.jpg") {
            ses.toggle(i);
        }
        self.tab = Tab::Results;
        match view {
            "preview" | "bigpreview" => {
                ses.folder = lisbon;
                ses.rebuild_now();
                ses.highlighted = picture(&ses, "IMG_2034.jpg");
                self.big_preview = view == "bigpreview";
            }
            "preview-text" => {
                ses.highlighted = picture(&ses, "Letter to the landlord.rtf");
            }
            "grid" => {
                ses.grid = true;
                ses.cat = Some(crate::model::Category::Pictures);
                ses.rebuild_now();
            }
            "scanning" => {
                ses.finished = false;
                self.fake_scan = Some(FakeProgress {
                    fraction: 0.42,
                    phase: "Deep scan: reading free space".to_string(),
                    detail: "42% · 391 GB of 931 GB · 164 MB/s · about 55 min left".to_string(),
                });
            }
            "recover" | "recover-blocked" | "recover-running" | "recover-done" => {
                let indices = ses.selected_indices();
                let dest = if view == "recover-blocked" {
                    "D:\\Recovered files"
                } else {
                    "F:\\Recovered files"
                };
                let info = self.dest_info(dest);
                let mut ui = RecoverUi {
                    bytes: ses.sel_bytes,
                    verdict: crate::recover::judge(&ses.spec, &ses.disks, &info),
                    info,
                    indices,
                    dest: dest.to_string(),
                    confirmed: false,
                    phase: RecoverPhase::Setup,
                    fake: None,
                };
                if view == "recover-running" {
                    ui.fake = Some(FakeProgress {
                        fraction: 0.61,
                        phase: "IMG_2047.jpg".to_string(),
                        detail: format!(
                            "19 of {} files · 61% · 48 MB/s · a few seconds left",
                            ui.indices.len()
                        ),
                    });
                }
                if view == "recover-done" {
                    let total = ui.indices.len() as u64;
                    ui.phase = RecoverPhase::Done(Report {
                        dest: PathBuf::from(dest),
                        total,
                        ok: total - 2,
                        damaged: 2,
                        failed: 0,
                        bytes: ses.sel_bytes,
                        problems: ["IMG_2038.jpg", "IMG_2050.jpg"]
                            .iter()
                            .map(|n| crate::recover::Item {
                                name: n.to_string(),
                                target: PathBuf::from(dest).join(n),
                                size: 82_000,
                                outcome: crate::recover::Outcome::Damaged(
                                    "Only 40.0 KB of 80.1 KB could be located; the rest was filled with zeros.".to_string(),
                                ),
                            })
                            .collect(),
                        stopped: false,
                    });
                }
                self.recover = Some(ui);
            }
            "log" => self.tab = Tab::Log,
            "images" => self.tab = Tab::Images,
            "imaging" => {
                self.tab = Tab::Images;
                self.imaging.source = Some(2);
                self.imaging.dest = "F:\\Images\\CAMERA card.img".to_string();
                self.imaging.info = self.dest_info(&self.imaging.dest);
                self.imaging.fake = Some(FakeProgress {
                    fraction: 0.37,
                    phase: "Copying the drive into an image file".to_string(),
                    detail: "37% · 22.0 GB of 59.4 GB · 71 MB/s · about 9 min left · 2 unreadable areas (8.00 KB) filled with zeros".to_string(),
                });
            }
            _ => {}
        }
        if view == "images" {
            self.imaging.source = Some(2);
            self.imaging.dest = "F:\\Images\\CAMERA card.img".to_string();
            self.imaging.info = self.dest_info(&self.imaging.dest);
        }
        self.ses = Some(ses);
    }

    // ----------------------------------------------------------------------------------
    // Keys and screenshots
    // ----------------------------------------------------------------------------------

    fn handle_keys(&mut self, ctx: &egui::Context) {
        let modal =
            self.recover.is_some() || self.about_open || self.welcome_open || self.shortcuts_open;
        let typing = ctx.wants_keyboard_input();
        let (mut find, mut refresh, mut esc, mut help) = (false, false, false, false);
        let (mut all, mut none, mut save, mut open, mut rec) = (false, false, false, false, false);
        let (mut up, mut down, mut space, mut enter, mut pause) =
            (false, false, false, false, false);
        ctx.input(|i| {
            find = i.modifiers.command && i.key_pressed(Key::F);
            refresh = i.key_pressed(Key::F5);
            esc = i.key_pressed(Key::Escape);
            help = i.key_pressed(Key::F1);
            all = i.modifiers.command && i.key_pressed(Key::A);
            none = i.modifiers.command && i.key_pressed(Key::D);
            save = i.modifiers.command && i.key_pressed(Key::S);
            open = i.modifiers.command && i.key_pressed(Key::O);
            rec = i.modifiers.command && i.key_pressed(Key::R);
            pause = i.modifiers.command && i.key_pressed(Key::P);
            up = i.key_pressed(Key::ArrowUp);
            down = i.key_pressed(Key::ArrowDown);
            space = i.key_pressed(Key::Space);
            enter = i.key_pressed(Key::Enter);
        });
        if help {
            self.shortcuts_open = !self.shortcuts_open;
        }
        if esc {
            if self.shortcuts_open {
                self.shortcuts_open = false;
            } else if self.big_preview {
                self.big_preview = false;
            } else if modal {
                // Dialogs close themselves.
            } else if !self.search.is_empty() {
                self.search.clear();
            } else if let Some(s) = &mut self.ses {
                s.highlighted = None;
            }
        }
        if modal {
            return;
        }
        if find {
            self.tab = if self.ses.is_some() {
                Tab::Results
            } else {
                self.tab
            };
            self.focus_search = true;
        }
        if refresh && self.tab == Tab::Recover {
            self.load_sources();
            self.toast("Drive list refreshed.", false);
        }
        if save && self.ses.is_some() {
            self.save_session();
        }
        if open {
            self.open_session();
        }
        if pause {
            if let Some(h) = self.ses.as_ref().and_then(|s| s.scan.as_ref()) {
                h.ctl.set_paused(!h.ctl.is_paused());
            }
        }
        if self.tab != Tab::Results {
            return;
        }
        if rec {
            self.open_recover_dialog(None);
        }
        let Some(s) = &mut self.ses else { return };
        if typing {
            return;
        }
        if all {
            s.set_visible(true);
        }
        if none {
            s.clear_selection();
        }
        if down {
            s.step_highlight(1);
        }
        if up {
            s.step_highlight(-1);
        }
        if let Some(h) = s.highlighted {
            if space {
                s.toggle(h);
            }
            if enter {
                self.big_preview = true;
            }
        }
    }

    fn handle_screenshot(&mut self, ctx: &egui::Context) {
        let waiting = self
            .previewer
            .full
            .as_ref()
            .map(|f| f.loading)
            .unwrap_or(false)
            || self.ses.as_ref().map(|s| s.scanning()).unwrap_or(false);
        let Some(shot) = &mut self.shot else { return };
        ctx.request_repaint();
        shot.frames += 1;
        if shot.frames == 1 {
            ctx.set_pixels_per_point(1.0);
        }
        if shot.frames == 3 {
            // Now that one point is one pixel, this gives an exact 1280 x 800 image.
            ctx.send_viewport_cmd(egui::ViewportCommand::InnerSize(vec2(1280.0, 800.0)));
        }
        let image = ctx.input(|i| {
            i.events.iter().find_map(|e| match e {
                egui::Event::Screenshot { image, .. } => Some(image.clone()),
                _ => None,
            })
        });
        if let Some(image) = image {
            let [w, h] = image.size;
            if let Some(parent) = shot.path.parent() {
                let _ = std::fs::create_dir_all(parent);
            }
            let result = image::save_buffer(
                &shot.path,
                image.as_raw(),
                w as u32,
                h as u32,
                image::ColorType::Rgba8,
            );
            if let Err(e) = result {
                eprintln!("screenshot failed: {e}");
            }
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
            self.shot = None;
            return;
        }
        // Give thumbnails a moment to arrive.
        if !shot.requested && shot.frames > 90 && (!waiting || shot.frames > 1500) {
            shot.requested = true;
            ctx.send_viewport_cmd(egui::ViewportCommand::Screenshot(egui::UserData::default()));
        }
    }

    // ----------------------------------------------------------------------------------
    // Frame: header and navigation
    // ----------------------------------------------------------------------------------

    fn header(&mut self, ctx: &egui::Context) {
        let p = pal_ctx(ctx);
        egui::TopBottomPanel::top("header")
            .exact_height(64.0)
            .frame(Frame::new().fill(p.rail).inner_margin(Margin::symmetric(18, 0)))
            .show(ctx, |ui| {
                let full = ui.max_rect();
                ui.painter().hline(
                    full.x_range().expand(18.0),
                    full.bottom(),
                    Stroke::new(1.0_f32, p.border),
                );
                ui.horizontal_centered(|ui| {
                    let (rect, _) = ui.allocate_exact_size(vec2(32.0, 32.0), Sense::hover());
                    if let Some(logo) = &self.logo {
                        ui.painter().image(
                            logo.id(),
                            rect,
                            Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
                            Color32::WHITE,
                        );
                    }
                    ui.add_space(2.0);
                    ui.label(RichText::new("RescueForge").font(semibold(19.0)));
                    if self.demo {
                        widgets::badge(ui, "DEMO", p.info)
                            .on_hover_text("An invented disk. Nothing on this PC is read or changed.");
                    }
                    ui.add_space(208.0 - ui.min_rect().width().min(208.0));

                    // Search
                    let search_width = (ui.available_width() - 400.0).clamp(220.0, 460.0);
                    let enabled = self.ses.is_some();
                    Frame::new()
                        .fill(if p.dark { p.card } else { p.bg })
                        .stroke(Stroke::new(1.0_f32, p.border))
                        .corner_radius(CornerRadius::same(10))
                        .inner_margin(Margin::symmetric(12, 6))
                        .show(ui, |ui| {
                            ui.set_width(search_width);
                            ui.horizontal(|ui| {
                                ui.label(RichText::new(Icon::SEARCH.glyph()).size(14.0).color(p.faint));
                                let hint = if enabled {
                                    "Search found files by name"
                                } else {
                                    "Search (after a scan)"
                                };
                                let edit = egui::TextEdit::singleline(&mut self.search)
                                    .hint_text(RichText::new(hint).color(p.faint))
                                    .frame(false)
                                    .desired_width(ui.available_width() - 56.0)
                                    .font(regular(15.0));
                                let resp = ui.add_enabled(enabled, edit);
                                if resp.changed() {
                                    self.tab = Tab::Results;
                                }
                                if self.focus_search {
                                    resp.request_focus();
                                    self.focus_search = false;
                                }
                                if !self.search.is_empty() {
                                    if widgets::icon_button(ui, Icon::CLOSE, "Clear search (Esc)").clicked() {
                                        self.search.clear();
                                    }
                                } else {
                                    ui.label(RichText::new("Ctrl+F").size(11.5).color(p.faint));
                                }
                            });
                        });

                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        if widgets::icon_button(ui, Icon::INFO, "About RescueForge").clicked() {
                            self.about_open = true;
                        }
                        if widgets::icon_button(ui, Icon::KEYBOARD, "Keyboard shortcuts (F1)").clicked() {
                            self.shortcuts_open = true;
                        }
                        if widgets::icon_button(ui, Icon::BULB, "How to recover safely").clicked() {
                            self.welcome_open = true;
                        }
                        ui.add_space(6.0);
                        if self.elevated {
                            widgets::badge(ui, &Icon::SHIELD.with("Administrator"), p.good).on_hover_text(
                                "RescueForge runs with administrator rights and can read drives and disks directly. It only ever reads them.",
                            );
                        } else {
                            if widgets::ghost_button(ui, "Restart as administrator").clicked() {
                                match sys::restart_as_admin() {
                                    Ok(()) => ctx.send_viewport_cmd(egui::ViewportCommand::Close),
                                    Err(e) => self.toast(e, true),
                                }
                            }
                            widgets::badge(ui, &Icon::WARNING.with("Standard rights"), p.warn).on_hover_text(
                                "Windows only lets administrators read drives and disks directly. Disk image files can be scanned without.",
                            );
                        }
                    });
                });
            });
    }

    fn rail(&mut self, ctx: &egui::Context) {
        let p = pal_ctx(ctx);
        egui::SidePanel::left("rail")
            .exact_width(208.0)
            .resizable(false)
            .frame(
                Frame::new()
                    .fill(p.rail)
                    .inner_margin(Margin::symmetric(10, 14)),
            )
            .show(ctx, |ui| {
                let full = ui.max_rect();
                ui.painter().vline(
                    full.right() + 10.0,
                    full.y_range().expand(14.0),
                    Stroke::new(1.0_f32, p.border),
                );
                ui.spacing_mut().item_spacing.y = 4.0;
                let found = self.ses.as_ref().map(|s| s.files.len());
                let log_lines = self.ses.as_ref().map(|s| s.log.len()).filter(|n| *n > 0);
                let scanning = self.ses.as_ref().map(|s| s.scanning()).unwrap_or(false)
                    || self.fake_scan.is_some();
                let items: [(Tab, Icon, &str, Option<usize>); 5] = [
                    (Tab::Recover, Icon::SCAN, "Recover files", None),
                    (Tab::Results, Icon::LIST, "Results", found),
                    (Tab::Images, Icon::IMAGE_FILE, "Disk images", None),
                    (Tab::Log, Icon::LOG, "Scan log", log_lines),
                    (Tab::Settings, Icon::SETTINGS, "Settings", None),
                ];
                for (tab, icon, label, count) in items {
                    if tab == Tab::Settings {
                        ui.add_space(8.0);
                        widgets::divider(ui);
                        ui.add_space(8.0);
                    }
                    let active = self.tab == tab;
                    let (rect, resp) =
                        ui.allocate_exact_size(vec2(ui.available_width(), 40.0), Sense::click());
                    if active {
                        ui.painter()
                            .rect_filled(rect, CornerRadius::same(9), p.selected);
                        let bar = Rect::from_min_size(
                            rect.left_center() - vec2(0.0, 9.0),
                            vec2(3.0, 18.0),
                        );
                        ui.painter()
                            .rect_filled(bar, CornerRadius::same(2), p.accent);
                    } else if resp.hovered() {
                        ui.painter()
                            .rect_filled(rect, CornerRadius::same(9), p.hover);
                    }
                    let fg = if active { p.text } else { p.muted };
                    ui.painter().text(
                        rect.left_center() + vec2(24.0, 0.0),
                        Align2::CENTER_CENTER,
                        icon.glyph(),
                        regular(16.0),
                        if active { p.accent } else { fg },
                    );
                    ui.painter().text(
                        rect.left_center() + vec2(46.0, -1.0),
                        Align2::LEFT_CENTER,
                        label,
                        if active {
                            semibold(14.5)
                        } else {
                            regular(14.5)
                        },
                        fg,
                    );
                    if let Some(n) = count {
                        ui.painter().text(
                            rect.right_center() - vec2(12.0, 1.0),
                            Align2::RIGHT_CENTER,
                            crate::util::fmt_count(n as u64),
                            regular(12.5),
                            if tab == Tab::Results && scanning {
                                p.accent
                            } else {
                                p.faint
                            },
                        );
                    }
                    if resp.clicked() {
                        self.tab = tab;
                    }
                }
                ui.with_layout(Layout::bottom_up(Align::Min), |ui| {
                    ui.add_space(2.0);
                    ui.horizontal(|ui| {
                        ui.add_space(12.0);
                        ui.label(
                            RichText::new(format!("Version {}", env!("CARGO_PKG_VERSION")))
                                .size(12.0)
                                .color(p.faint),
                        );
                    });
                    ui.horizontal(|ui| {
                        ui.add_space(12.0);
                        ui.label(
                            RichText::new(Icon::SHIELD.with("Sources are opened read-only"))
                                .size(12.0)
                                .color(p.faint),
                        );
                    });
                });
            });
    }

    // ----------------------------------------------------------------------------------
    // Dialogs and toasts
    // ----------------------------------------------------------------------------------

    fn about_dialog(&mut self, ctx: &egui::Context) {
        if !self.about_open {
            return;
        }
        let p = pal_ctx(ctx);
        let mut close = false;
        let modal = egui::Modal::new(egui::Id::new("about"))
            .frame(modal_frame(ctx, 28))
            .show(ctx, |ui| {
                ui.set_width(400.0);
                ui.vertical_centered(|ui| {
                    let (rect, _) = ui.allocate_exact_size(vec2(84.0, 84.0), Sense::hover());
                    if let Some(logo) = &self.logo {
                        ui.painter().image(
                            logo.id(),
                            rect,
                            Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
                            Color32::WHITE,
                        );
                    }
                    ui.add_space(6.0);
                    ui.label(RichText::new("RescueForge").font(semibold(26.0)));
                    ui.label(
                        RichText::new(format!("Version {}", env!("CARGO_PKG_VERSION"))).color(p.muted),
                    );
                    ui.add_space(10.0);
                    ui.label("Finds deleted files, lost partitions and files on formatted drives, and copies them to a safe place.");
                    ui.add_space(10.0);
                    ui.label(RichText::new("Developed by Zheer Khalid").font(semibold(15.0)));
                    ui.add_space(2.0);
                    ui.hyperlink_to(RichText::new("zheer.dev").color(p.accent), "https://zheer.dev");
                    ui.hyperlink_to(
                        RichText::new("github.com/zheerkhalid/rescueforge").color(p.accent),
                        "https://github.com/zheerkhalid/rescueforge",
                    );
                    ui.add_space(10.0);
                    ui.label(
                        RichText::new("Free and open source under the MIT License.\nThe drive being scanned is only ever read, never written.")
                            .size(12.0)
                            .color(p.faint),
                    );
                    ui.add_space(14.0);
                    if widgets::primary_button(ui, "Close", true).clicked() {
                        close = true;
                    }
                });
            });
        if close || modal.should_close() {
            self.about_open = false;
        }
    }

    fn welcome_dialog(&mut self, ctx: &egui::Context) {
        if !self.welcome_open {
            return;
        }
        let p = pal_ctx(ctx);
        let mut close = false;
        let modal = egui::Modal::new(egui::Id::new("welcome"))
            .frame(modal_frame(ctx, 26))
            .show(ctx, |ui| {
                ui.set_width(560.0);
                ui.label(RichText::new("Before you start").font(semibold(22.0)));
                ui.add_space(2.0);
                ui.label(
                    RichText::new("Deleted files stay on a drive until something new is written over them. Three habits decide how much comes back:")
                        .color(p.muted),
                );
                ui.add_space(10.0);
                let steps = [
                    (
                        "Stop using the drive",
                        "Every new file, download or installation can land on top of what you lost. Do not save anything to that drive, and do not install RescueForge on it.",
                    ),
                    (
                        "Recover to a different drive",
                        "Save what RescueForge finds to another disk, a USB stick or a network folder. RescueForge refuses to write to the drive it is recovering from.",
                    ),
                    (
                        "Quick scan first, deep scan second",
                        "A quick scan takes seconds to minutes and brings back names and folders. If it does not find your files, or the drive was formatted, run a deep scan: it reads everything and recognises files by their content.",
                    ),
                ];
                for (i, (title, text)) in steps.iter().enumerate() {
                    ui.horizontal_top(|ui| {
                        let (rect, _) = ui.allocate_exact_size(vec2(30.0, 30.0), Sense::hover());
                        ui.painter().circle_filled(rect.center(), 14.0, p.accent.gamma_multiply(0.18));
                        ui.painter().text(
                            rect.center(),
                            Align2::CENTER_CENTER,
                            (i + 1).to_string(),
                            semibold(15.0),
                            p.accent,
                        );
                        ui.vertical(|ui| {
                            ui.spacing_mut().item_spacing.y = 2.0;
                            ui.label(RichText::new(*title).font(semibold(15.0)));
                            ui.add(egui::Label::new(RichText::new(*text).color(p.muted)).wrap());
                        });
                    });
                    ui.add_space(6.0);
                }
                widgets::notice(
                    ui,
                    Icon::INFO,
                    p.info,
                    "Is the drive making noises, disappearing or reading very slowly? Create a disk image first (Disk images tab) and scan the image, so the failing drive is only read once.",
                );
                ui.add_space(12.0);
                widgets::button_row(ui, |ui| {
                    if widgets::primary_button(ui, "Got it", true).clicked() {
                        close = true;
                    }
                });
            });
        if close || modal.should_close() {
            self.welcome_open = false;
            if !self.settings.welcome_seen {
                self.settings.welcome_seen = true;
                self.save_settings();
            }
        }
    }

    fn shortcuts_dialog(&mut self, ctx: &egui::Context) {
        if !self.shortcuts_open {
            return;
        }
        let p = pal_ctx(ctx);
        let mut close = false;
        let modal = egui::Modal::new(egui::Id::new("shortcuts"))
            .frame(modal_frame(ctx, 24))
            .show(ctx, |ui| {
                ui.set_width(460.0);
                ui.label(RichText::new("Keyboard shortcuts").font(semibold(20.0)));
                ui.add_space(8.0);
                egui::Grid::new("shortcut-list")
                    .num_columns(2)
                    .spacing([18.0, 9.0])
                    .show(ui, |ui| {
                        for (keys, what) in SHORTCUTS {
                            widgets::badge(ui, keys, p.muted);
                            ui.label(*what);
                            ui.end_row();
                        }
                    });
                ui.add_space(10.0);
                widgets::button_row(ui, |ui| {
                    if widgets::primary_button(ui, "Close", true).clicked() {
                        close = true;
                    }
                });
            });
        if close || modal.should_close() {
            self.shortcuts_open = false;
        }
    }

    fn toasts_ui(&mut self, ctx: &egui::Context) {
        self.toasts
            .retain(|t| t.born.elapsed().as_secs_f32() < if t.error { 9.0 } else { 3.5 });
        if self.toasts.is_empty() || self.shot.is_some() {
            return;
        }
        ctx.request_repaint_after(std::time::Duration::from_millis(250));
        let p = pal_ctx(ctx);
        egui::Area::new(egui::Id::new("toasts"))
            .anchor(Align2::RIGHT_BOTTOM, vec2(-18.0, -74.0))
            .order(egui::Order::Foreground)
            .show(ctx, |ui| {
                ui.with_layout(Layout::bottom_up(Align::Max), |ui| {
                    for t in self.toasts.iter().rev().take(4) {
                        let color = if t.error { p.danger } else { p.good };
                        Frame::new()
                            .fill(p.card)
                            .stroke(Stroke::new(1.0_f32, color.gamma_multiply(0.7)))
                            .corner_radius(CornerRadius::same(10))
                            .inner_margin(Margin::symmetric(14, 10))
                            .show(ui, |ui| {
                                ui.set_max_width(440.0);
                                ui.horizontal(|ui| {
                                    let icon = if t.error { Icon::ERROR } else { Icon::DONE };
                                    ui.label(RichText::new(icon.glyph()).color(color));
                                    ui.add(egui::Label::new(t.text.as_str()).wrap());
                                });
                            });
                    }
                });
            });
    }

    /// The standard page header: title, subtitle and right-aligned actions.
    pub(super) fn page_header(
        ui: &mut Ui,
        title: &str,
        subtitle: &str,
        actions: impl FnOnce(&mut Ui),
    ) {
        let p = pal(ui);
        ui.allocate_ui_with_layout(
            vec2(ui.available_width(), 52.0),
            Layout::left_to_right(Align::Center),
            |ui| {
                ui.vertical(|ui| {
                    ui.spacing_mut().item_spacing.y = 2.0;
                    ui.add(egui::Label::new(RichText::new(title).font(semibold(24.0))).truncate());
                    ui.add(egui::Label::new(RichText::new(subtitle).color(p.muted)).truncate());
                });
                ui.with_layout(Layout::right_to_left(Align::Center), actions);
            },
        );
        ui.add_space(6.0);
    }

    /// "42% · 164 MB/s · about 55 min left" for running work.
    pub(super) fn progress_text(
        progress: &crate::control::Progress,
        meter: &super::state::Meter,
        paused: bool,
    ) -> String {
        use std::sync::atomic::Ordering::Relaxed;
        let (done, total) = (progress.done.load(Relaxed), progress.total.load(Relaxed));
        let mut parts = vec![format!("{:.0}%", progress.fraction() * 100.0)];
        if total > 0 {
            parts.push(format!(
                "{} of {}",
                fmt_size(done.min(total)),
                fmt_size(total)
            ));
        }
        if paused {
            parts.push("paused".to_string());
        } else if let Some(speed) = meter.speed() {
            parts.push(format!("{}/s", fmt_size(speed as u64)));
            if let Some(eta) = meter.eta(total.saturating_sub(done)) {
                parts.push(if eta < 5 {
                    "a few seconds left".to_string()
                } else {
                    format!("about {} left", crate::util::fmt_duration(eta))
                });
            }
        }
        parts.join(" · ")
    }
}

pub(super) const SHORTCUTS: &[(&str, &str)] = &[
    ("Ctrl+F", "Search the found files by name"),
    ("Up / Down", "Move through the list"),
    ("Space", "Tick or untick the highlighted file"),
    ("Enter", "Open the large preview"),
    ("Ctrl+A", "Tick every file that is shown"),
    ("Ctrl+D", "Untick everything"),
    ("Ctrl+R", "Recover the ticked files"),
    ("Ctrl+S", "Save the scan to a session file"),
    ("Ctrl+O", "Open a saved session"),
    ("Ctrl+P", "Pause or resume the scan"),
    ("F5", "Refresh the list of drives"),
    ("Esc", "Clear the search, close the preview or a dialog"),
    ("F1", "Show this list"),
];

impl eframe::App for App {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.poll_scan();
        self.poll_recover(ctx);
        self.poll_imaging();
        self.previewer.poll(ctx);
        self.handle_keys(ctx);
        self.handle_screenshot(ctx);
        if let Some(s) = &mut self.ses {
            s.set_search(&self.search);
            s.rebuild_if_needed();
            if s.scanning() {
                ctx.request_repaint_after(std::time::Duration::from_millis(250));
            }
        }
        if self.busy() {
            ctx.request_repaint_after(std::time::Duration::from_millis(250));
        }

        self.header(ctx);
        self.rail(ctx);

        let p = pal_ctx(ctx);
        let results = self.tab == Tab::Results && self.ses.is_some();
        if results {
            egui::TopBottomPanel::bottom("selection")
                .exact_height(60.0)
                .frame(
                    Frame::new()
                        .fill(p.rail)
                        .inner_margin(Margin::symmetric(24, 0)),
                )
                .show(ctx, |ui| self.selection_bar(ui));
            if self.ses.as_ref().and_then(|s| s.highlighted).is_some() {
                egui::SidePanel::right("preview")
                    .exact_width(330.0)
                    .resizable(false)
                    .frame(Frame::new().fill(p.rail).inner_margin(Margin::same(18)))
                    .show(ctx, |ui| self.preview_panel(ui));
            }
        }
        egui::CentralPanel::default()
            .frame(
                Frame::new()
                    .fill(p.bg)
                    .inner_margin(Margin::symmetric(24, 18)),
            )
            .show(ctx, |ui| match self.tab {
                Tab::Recover => self.home_ui(ui),
                Tab::Results => self.results_ui(ui),
                Tab::Images => self.images_ui(ui),
                Tab::Log => self.log_ui(ui),
                Tab::Settings => self.settings_ui(ui),
            });

        self.recover_dialog(ctx);
        self.big_preview_dialog(ctx);
        self.about_dialog(ctx);
        self.welcome_dialog(ctx);
        self.shortcuts_dialog(ctx);
        self.toasts_ui(ctx);
    }

    fn raw_input_hook(&mut self, _ctx: &egui::Context, raw_input: &mut egui::RawInput) {
        // Screenshots must not depend on where the mouse happens to be:
        // no hover highlights, no tooltips.
        if self.shot.is_some() {
            raw_input.events.retain(|e| {
                !matches!(
                    e,
                    egui::Event::PointerMoved(_)
                        | egui::Event::MouseMoved(_)
                        | egui::Event::PointerButton { .. }
                        | egui::Event::MouseWheel { .. }
                )
            });
            raw_input.events.push(egui::Event::PointerGone);
        }
    }

    fn on_exit(&mut self, _gl: Option<&eframe::glow::Context>) {
        if let Some(h) = self.ses.as_ref().and_then(|s| s.scan.as_ref()) {
            h.ctl.stop();
        }
        if let Some(RecoverUi {
            phase: RecoverPhase::Running { ctl, .. },
            ..
        }) = &self.recover
        {
            ctl.stop();
        }
        if let ImagingPhase::Running { ctl, .. } = &self.imaging.phase {
            ctl.stop();
        }
    }
}
