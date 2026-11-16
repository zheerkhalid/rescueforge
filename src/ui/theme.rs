//! Colours, fonts and icon glyphs.

use crate::settings::ThemeChoice;
use eframe::egui::{
    self, Color32, CornerRadius, FontData, FontDefinitions, FontFamily, FontId, Stroke, TextStyle,
    Theme, ThemePreference, Visuals,
};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

pub const AMBER: Color32 = Color32::from_rgb(0xf6, 0xa8, 0x21);
pub const CORAL: Color32 = Color32::from_rgb(0xf0, 0x55, 0x4a);
pub const GREEN: Color32 = Color32::from_rgb(0x35, 0xc9, 0x8a);

#[derive(Clone, Copy)]
pub struct Palette {
    pub dark: bool,
    pub bg: Color32,
    pub rail: Color32,
    pub card: Color32,
    pub hover: Color32,
    pub selected: Color32,
    pub border: Color32,
    pub text: Color32,
    pub muted: Color32,
    pub faint: Color32,
    /// Amber, tuned for contrast on this background.
    pub accent: Color32,
    pub on_accent: Color32,
    pub danger: Color32,
    pub warn: Color32,
    pub good: Color32,
    pub info: Color32,
}

pub fn palette(dark: bool) -> Palette {
    if dark {
        Palette {
            dark,
            bg: Color32::from_rgb(0x12, 0x14, 0x18),
            rail: Color32::from_rgb(0x0d, 0x0f, 0x12),
            card: Color32::from_rgb(0x1a, 0x1d, 0x23),
            hover: Color32::from_rgb(0x22, 0x26, 0x2e),
            selected: Color32::from_rgb(0x33, 0x2a, 0x14),
            border: Color32::from_rgb(0x2c, 0x31, 0x3a),
            text: Color32::from_rgb(0xec, 0xee, 0xf2),
            muted: Color32::from_rgb(0xa0, 0xa8, 0xb4),
            faint: Color32::from_rgb(0x6a, 0x72, 0x7e),
            accent: AMBER,
            on_accent: Color32::from_rgb(0x24, 0x16, 0x02),
            danger: CORAL,
            warn: Color32::from_rgb(0xff, 0x8f, 0x4a),
            good: GREEN,
            info: Color32::from_rgb(0x4f, 0xb0, 0xe8),
        }
    } else {
        Palette {
            dark,
            bg: Color32::from_rgb(0xf5, 0xf5, 0xf3),
            rail: Color32::from_rgb(0xff, 0xff, 0xff),
            card: Color32::from_rgb(0xff, 0xff, 0xff),
            hover: Color32::from_rgb(0xee, 0xee, 0xea),
            selected: Color32::from_rgb(0xfd, 0xee, 0xcf),
            border: Color32::from_rgb(0xdd, 0xdd, 0xd6),
            text: Color32::from_rgb(0x1c, 0x1d, 0x21),
            muted: Color32::from_rgb(0x5c, 0x61, 0x6b),
            faint: Color32::from_rgb(0x90, 0x95, 0x9e),
            accent: Color32::from_rgb(0xb4, 0x5a, 0x06),
            on_accent: Color32::WHITE,
            danger: Color32::from_rgb(0xd6, 0x3a, 0x2f),
            warn: Color32::from_rgb(0xc2, 0x5a, 0x12),
            good: Color32::from_rgb(0x12, 0x80, 0x52),
            info: Color32::from_rgb(0x15, 0x6f, 0xb0),
        }
    }
}

pub fn pal(ui: &egui::Ui) -> Palette {
    palette(ui.visuals().dark_mode)
}

pub fn pal_ctx(ctx: &egui::Context) -> Palette {
    palette(ctx.style().visuals.dark_mode)
}

fn visuals(dark: bool) -> Visuals {
    let p = palette(dark);
    let mut v = if dark {
        Visuals::dark()
    } else {
        Visuals::light()
    };
    v.panel_fill = p.bg;
    v.window_fill = p.card;
    v.extreme_bg_color = if dark {
        Color32::from_rgb(0x0c, 0x0e, 0x11)
    } else {
        Color32::WHITE
    };
    v.faint_bg_color = p.hover;
    v.window_stroke = Stroke::new(1.0_f32, p.border);
    v.window_corner_radius = CornerRadius::same(14);
    v.menu_corner_radius = CornerRadius::same(10);
    v.override_text_color = Some(p.text);
    v.hyperlink_color = p.accent;
    v.selection.bg_fill = if dark {
        Color32::from_rgb(0x6b, 0x4a, 0x0e)
    } else {
        Color32::from_rgb(0xfb, 0xe0, 0xa8)
    };
    v.selection.stroke = Stroke::new(1.0_f32, p.accent);
    v.warn_fg_color = p.warn;
    v.error_fg_color = p.danger;
    let r = CornerRadius::same(8);
    for w in [
        &mut v.widgets.noninteractive,
        &mut v.widgets.inactive,
        &mut v.widgets.hovered,
        &mut v.widgets.active,
        &mut v.widgets.open,
    ] {
        w.corner_radius = r;
    }
    v.widgets.noninteractive.bg_stroke = Stroke::new(1.0_f32, p.border);
    v.widgets.noninteractive.fg_stroke = Stroke::new(1.0_f32, p.muted);
    v.widgets.inactive.bg_fill = p.hover;
    v.widgets.inactive.weak_bg_fill = p.hover;
    v.widgets.inactive.bg_stroke = Stroke::new(1.0_f32, p.border);
    v.widgets.inactive.fg_stroke = Stroke::new(1.0_f32, p.text);
    v.widgets.hovered.bg_fill = p.border;
    v.widgets.hovered.weak_bg_fill = p.border;
    v.widgets.hovered.bg_stroke = Stroke::new(1.0_f32, p.faint);
    v.widgets.hovered.fg_stroke = Stroke::new(1.0_f32, p.text);
    v.widgets.active.bg_fill = p.border;
    v.widgets.active.weak_bg_fill = p.border;
    v.widgets.active.bg_stroke = Stroke::new(1.0_f32, p.accent);
    v.widgets.active.fg_stroke = Stroke::new(1.0_f32, p.text);
    v.widgets.open.bg_fill = p.hover;
    v.widgets.open.weak_bg_fill = p.hover;
    v.widgets.open.bg_stroke = Stroke::new(1.0_f32, p.faint);
    v
}

static ICON_FONT: AtomicBool = AtomicBool::new(false);

fn load_fonts(ctx: &egui::Context) {
    let dir = std::env::var_os("SystemRoot")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| "C:\\Windows".into())
        .join("Fonts");
    let mut fonts = FontDefinitions::default();
    let mut add = |name: &str, file: &str| -> bool {
        match std::fs::read(dir.join(file)) {
            Ok(bytes) => {
                fonts
                    .font_data
                    .insert(name.to_string(), Arc::new(FontData::from_owned(bytes)));
                true
            }
            Err(_) => false,
        }
    };
    // System fonts are loaded at run time so the program stays small.
    let regular = add("ui", "segoeui.ttf");
    let semibold = add("ui-semibold", "seguisb.ttf");
    let icons = add("ui-icons", "segmdl2.ttf") || add("ui-icons", "SegoeIcons.ttf");
    ICON_FONT.store(icons, Ordering::Relaxed);
    // File names can be in any script: fall back to the fonts Windows ships
    // for Chinese, Japanese, Korean and symbols (about 35 MB of memory).
    let extra: Vec<&str> = [
        ("ui-cjk", "msyh.ttc"),
        ("ui-kr", "malgun.ttf"),
        ("ui-symbols", "seguisym.ttf"),
    ]
    .into_iter()
    .filter(|(name, file)| add(name, file))
    .map(|(name, _)| name)
    .collect();

    let defaults = fonts
        .families
        .get(&FontFamily::Proportional)
        .cloned()
        .unwrap_or_default();
    let mut body = Vec::new();
    if regular {
        body.push("ui".to_string());
    }
    if icons {
        body.push("ui-icons".to_string());
    }
    body.extend(defaults.iter().cloned());
    body.extend(extra.iter().map(|n| n.to_string()));
    let mut strong = Vec::new();
    if semibold {
        strong.push("ui-semibold".to_string());
    }
    strong.extend(body.iter().cloned());
    fonts.families.insert(FontFamily::Proportional, body);
    fonts
        .families
        .insert(FontFamily::Name("semibold".into()), strong);
    ctx.set_fonts(fonts);
}

pub fn semibold(size: f32) -> FontId {
    FontId::new(size, FontFamily::Name("semibold".into()))
}

pub fn regular(size: f32) -> FontId {
    FontId::new(size, FontFamily::Proportional)
}

/// One-time setup: fonts, spacing and both colour schemes.
pub fn install(ctx: &egui::Context) {
    load_fonts(ctx);
    ctx.set_visuals_of(Theme::Dark, visuals(true));
    ctx.set_visuals_of(Theme::Light, visuals(false));
    ctx.all_styles_mut(|style| {
        style.text_styles = [
            (TextStyle::Heading, semibold(22.0)),
            (TextStyle::Body, regular(14.0)),
            (TextStyle::Button, regular(14.0)),
            (TextStyle::Small, regular(12.0)),
            (
                TextStyle::Monospace,
                FontId::new(13.0, FontFamily::Monospace),
            ),
        ]
        .into();
        style.spacing.item_spacing = egui::vec2(8.0, 8.0);
        style.spacing.button_padding = egui::vec2(12.0, 6.0);
        style.spacing.interact_size.y = 30.0;
        style.spacing.scroll.bar_width = 10.0;
        style.spacing.scroll.floating = true;
        style.interaction.selectable_labels = false;
        style.visuals.indent_has_left_vline = false;
    });
}

pub fn apply_choice(ctx: &egui::Context, choice: ThemeChoice) {
    ctx.set_theme(match choice {
        ThemeChoice::Dark => ThemePreference::Dark,
        ThemeChoice::Light => ThemePreference::Light,
        ThemeChoice::System => ThemePreference::System,
    });
}

/// Icon glyphs from the system icon font (Segoe MDL2 Assets). If the font is
/// missing the icons are simply left out.
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct Icon(pub &'static str);

impl Icon {
    pub const SEARCH: Icon = Icon("\u{E721}");
    pub const SETTINGS: Icon = Icon("\u{E713}");
    pub const REFRESH: Icon = Icon("\u{E72C}");
    pub const FOLDER_OPEN: Icon = Icon("\u{E838}");
    pub const WARNING: Icon = Icon("\u{E7BA}");
    pub const INFO: Icon = Icon("\u{E946}");
    pub const SHIELD: Icon = Icon("\u{EA18}");
    pub const CHECK: Icon = Icon("\u{E73E}");
    pub const CLOSE: Icon = Icon("\u{E711}");
    pub const CHEVRON_RIGHT: Icon = Icon("\u{E76C}");
    pub const CHEVRON_DOWN: Icon = Icon("\u{E70D}");
    pub const DOCUMENT: Icon = Icon("\u{E8A5}");
    pub const SORT_UP: Icon = Icon("\u{E70E}");
    pub const SORT_DOWN: Icon = Icon("\u{E70D}");
    pub const DONE: Icon = Icon("\u{E930}");
    pub const ERROR: Icon = Icon("\u{EA39}");
    pub const DRIVE: Icon = Icon("\u{EDA2}");
    pub const USB: Icon = Icon("\u{E88E}");
    pub const DISK: Icon = Icon("\u{E958}");
    pub const IMAGE_FILE: Icon = Icon("\u{E8C8}");
    pub const PICTURE: Icon = Icon("\u{EB9F}");
    pub const VIDEO: Icon = Icon("\u{E714}");
    pub const AUDIO: Icon = Icon("\u{EC4F}");
    pub const ARCHIVE: Icon = Icon("\u{E7B8}");
    pub const OTHER: Icon = Icon("\u{E7C3}");
    pub const PAUSE: Icon = Icon("\u{E769}");
    pub const PLAY: Icon = Icon("\u{E768}");
    pub const STOP: Icon = Icon("\u{E71A}");
    pub const SAVE: Icon = Icon("\u{E74E}");
    pub const OPEN: Icon = Icon("\u{E8E5}");
    pub const RECOVER: Icon = Icon("\u{E896}");
    pub const LIST: Icon = Icon("\u{EA37}");
    pub const KEYBOARD: Icon = Icon("\u{E765}");
    pub const FILTER: Icon = Icon("\u{E71C}");
    pub const BULB: Icon = Icon("\u{EA80}");
    pub const VIEW: Icon = Icon("\u{E890}");
    pub const LOG: Icon = Icon("\u{E9D5}");
    pub const COPY: Icon = Icon("\u{E8C8}");
    pub const SCAN: Icon = Icon("\u{E773}");
    pub const ADD: Icon = Icon("\u{E710}");

    /// The glyph, or an empty string when the icon font is unavailable.
    pub fn glyph(self) -> &'static str {
        if ICON_FONT.load(Ordering::Relaxed) {
            self.0
        } else {
            ""
        }
    }

    /// `"<glyph>  text"` for buttons.
    pub fn with(self, text: &str) -> String {
        if ICON_FONT.load(Ordering::Relaxed) {
            format!("{}  {text}", self.0)
        } else {
            text.to_string()
        }
    }
}
