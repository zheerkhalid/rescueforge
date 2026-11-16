//! Build script: renders the RescueForge icon (vector, via tiny-skia) into a
//! multi-size .ico and a PNG for the window, then embeds the icon, manifest and version info.

#![allow(clippy::field_reassign_with_default)]

use std::{env, fs, path::PathBuf};
use tiny_skia::*;

fn rounded_rect(x0: f32, y0: f32, x1: f32, y1: f32, r: f32) -> Path {
    let mut pb = PathBuilder::new();
    pb.move_to(x0 + r, y0);
    pb.line_to(x1 - r, y0);
    pb.quad_to(x1, y0, x1, y0 + r);
    pb.line_to(x1, y1 - r);
    pb.quad_to(x1, y1, x1 - r, y1);
    pb.line_to(x0 + r, y1);
    pb.quad_to(x0, y1, x0, y1 - r);
    pb.line_to(x0, y0 + r);
    pb.quad_to(x0, y0, x0 + r, y0);
    pb.close();
    pb.finish().expect("rounded rect")
}

fn render_icon(size: u32) -> Vec<u8> {
    let mut pm = Pixmap::new(size, size).expect("pixmap");
    let s = size as f32 / 256.0;
    let id = Transform::identity();

    // Background: rounded square, amber -> ember orange gradient.
    let (x0, y0, x1, y1) = (8.0 * s, 8.0 * s, 248.0 * s, 248.0 * s);
    let bg = rounded_rect(x0, y0, x1, y1, 56.0 * s);
    let mut paint = Paint::default();
    paint.anti_alias = true;
    paint.shader = LinearGradient::new(
        Point::from_xy(x0, y0),
        Point::from_xy(x1, y1),
        vec![
            GradientStop::new(0.0, Color::from_rgba8(0xff, 0xc2, 0x33, 255)),
            GradientStop::new(1.0, Color::from_rgba8(0xee, 0x4f, 0x2c, 255)),
        ],
        SpreadMode::Pad,
        id,
    )
    .expect("gradient");
    pm.fill_path(&bg, &paint, FillRule::Winding, id, None);

    let fill = |pm: &mut Pixmap, path: &Path, r: u8, g: u8, b: u8| {
        let mut p = Paint::default();
        p.anti_alias = true;
        p.set_color_rgba8(r, g, b, 255);
        pm.fill_path(path, &p, FillRule::Winding, id, None);
    };

    // A sheet of paper with a folded corner: the file that comes back.
    let mut pb = PathBuilder::new();
    pb.move_to(74.0 * s, 42.0 * s);
    pb.line_to(136.0 * s, 42.0 * s);
    pb.line_to(176.0 * s, 82.0 * s);
    pb.line_to(176.0 * s, 190.0 * s);
    pb.quad_to(176.0 * s, 202.0 * s, 164.0 * s, 202.0 * s);
    pb.line_to(74.0 * s, 202.0 * s);
    pb.quad_to(62.0 * s, 202.0 * s, 62.0 * s, 190.0 * s);
    pb.line_to(62.0 * s, 54.0 * s);
    pb.quad_to(62.0 * s, 42.0 * s, 74.0 * s, 42.0 * s);
    pb.close();
    let sheet = pb.finish().expect("sheet");
    fill(&mut pm, &sheet, 0xff, 0xff, 0xff);
    let mut pb = PathBuilder::new();
    pb.move_to(136.0 * s, 42.0 * s);
    pb.line_to(176.0 * s, 82.0 * s);
    pb.line_to(146.0 * s, 82.0 * s);
    pb.quad_to(136.0 * s, 82.0 * s, 136.0 * s, 72.0 * s);
    pb.close();
    let fold = pb.finish().expect("fold");
    fill(&mut pm, &fold, 0xff, 0xd9, 0xb0);
    if size >= 40 {
        for (y, x_end) in [(108.0, 152.0), (130.0, 140.0), (152.0, 118.0)] {
            let line = rounded_rect(84.0 * s, y * s, x_end * s, (y + 10.0) * s, 5.0 * s);
            fill(&mut pm, &line, 0xfb, 0xc9, 0xa0);
        }
    }

    // Round badge, lower right: a turning-back arrow on deep slate.
    let (cx, cy) = (180.0 * s, 182.0 * s);
    let mut pb = PathBuilder::new();
    pb.push_circle(cx, cy, 52.0 * s);
    fill(&mut pm, &pb.finish().expect("ring"), 0xff, 0xff, 0xff);
    let mut pb = PathBuilder::new();
    pb.push_circle(cx, cy, 44.0 * s);
    fill(&mut pm, &pb.finish().expect("badge"), 0x1d, 0x2b, 0x3a);
    let r = 21.0 * s;
    let point = |deg: f32| {
        let t = deg.to_radians();
        (cx + r * t.cos(), cy + r * t.sin())
    };
    let (start, sweep) = (-125.0_f32, 285.0_f32);
    let mut pb = PathBuilder::new();
    for i in 0..=48 {
        let (x, y) = point(start + sweep * i as f32 / 48.0);
        if i == 0 {
            pb.move_to(x, y)
        } else {
            pb.line_to(x, y)
        }
    }
    let arc = pb.finish().expect("arc");
    let mut white = Paint::default();
    white.anti_alias = true;
    white.set_color_rgba8(0xff, 0xff, 0xff, 255);
    let mut stroke = Stroke::default();
    stroke.width = 9.5 * s;
    stroke.line_cap = LineCap::Round;
    pm.stroke_path(&arc, &white, &stroke, id, None);
    // Arrow head at the start of the arc, pointing against the clock.
    let t = start.to_radians();
    let (px, py) = point(start);
    let (dx, dy) = (t.sin(), -t.cos());
    let (nx, ny) = (t.cos(), t.sin());
    let (tip, half) = (17.0 * s, 15.0 * s);
    let mut pb = PathBuilder::new();
    pb.move_to(px + dx * tip, py + dy * tip);
    pb.line_to(px + nx * half - dx * 3.0 * s, py + ny * half - dy * 3.0 * s);
    pb.line_to(px - nx * half - dx * 3.0 * s, py - ny * half - dy * 3.0 * s);
    pb.close();
    fill(&mut pm, &pb.finish().expect("head"), 0xff, 0xff, 0xff);

    pm.encode_png().expect("png")
}

fn write_ico(path: &PathBuf, sizes: &[u32]) {
    let pngs: Vec<Vec<u8>> = sizes.iter().map(|&s| render_icon(s)).collect();
    let mut out = Vec::new();
    out.extend_from_slice(&[0, 0, 1, 0]);
    out.extend_from_slice(&(sizes.len() as u16).to_le_bytes());
    let mut offset = 6 + 16 * sizes.len() as u32;
    for (i, &s) in sizes.iter().enumerate() {
        let b = if s >= 256 { 0 } else { s as u8 };
        out.extend_from_slice(&[b, b, 0, 0, 1, 0, 32, 0]);
        out.extend_from_slice(&(pngs[i].len() as u32).to_le_bytes());
        out.extend_from_slice(&offset.to_le_bytes());
        offset += pngs[i].len() as u32;
    }
    for p in &pngs {
        out.extend_from_slice(p);
    }
    fs::write(path, out).expect("write ico");
}

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    let out = PathBuf::from(env::var("OUT_DIR").expect("OUT_DIR"));
    let ico = out.join("rescueforge.ico");
    write_ico(&ico, &[16, 20, 24, 32, 40, 48, 64, 256]);
    let png = render_icon(256);
    fs::write(out.join("icon-256.png"), &png).expect("write png");
    let docs_png = PathBuf::from("docs/assets/rescueforge-icon.png");
    if fs::read(&docs_png).ok().as_deref() != Some(&png[..]) {
        let _ = fs::create_dir_all("docs/assets");
        let _ = fs::write(&docs_png, &png);
    }

    if env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        return;
    }
    let ver = env::var("CARGO_PKG_VERSION").expect("version");
    let parts: Vec<u16> = ver
        .split(['.', '-'])
        .take(3)
        .map(|p| p.parse().unwrap_or(0))
        .collect();
    let (ma, mi, pa) = (
        parts[0],
        parts.get(1).copied().unwrap_or(0),
        parts.get(2).copied().unwrap_or(0),
    );
    let ver4 = format!("{ma}.{mi}.{pa}.0");
    println!("cargo:rerun-if-changed=res/rescueforge.manifest.in");
    let manifest = fs::read_to_string("res/rescueforge.manifest.in")
        .expect("manifest")
        .replace("@VERSION4@", &ver4);
    let manifest_path = out.join("rescueforge.manifest");
    fs::write(&manifest_path, manifest).expect("write manifest");
    let esc = |p: &PathBuf| p.display().to_string().replace('\\', "\\\\");
    let rc = format!(
        r#"#pragma code_page(65001)
1 ICON "{ico}"
1 24 "{manifest}"
1 VERSIONINFO
FILEVERSION {ma},{mi},{pa},0
PRODUCTVERSION {ma},{mi},{pa},0
FILEOS 0x40004
FILETYPE 0x1
BEGIN
  BLOCK "StringFileInfo"
  BEGIN
    BLOCK "040904b0"
    BEGIN
      VALUE "CompanyName", "Zheer Khalid (zheer.dev)"
      VALUE "FileDescription", "RescueForge file recovery"
      VALUE "FileVersion", "{ver}"
      VALUE "InternalName", "RescueForge"
      VALUE "LegalCopyright", "Copyright (c) 2026 Zheer Khalid. MIT License."
      VALUE "OriginalFilename", "RescueForge.exe"
      VALUE "ProductName", "RescueForge"
      VALUE "ProductVersion", "{ver}"
    END
  END
  BLOCK "VarFileInfo"
  BEGIN
    VALUE "Translation", 0x409, 1200
  END
END
"#,
        ico = esc(&ico),
        manifest = esc(&manifest_path),
    );
    let rc_path = out.join("rescueforge.rc");
    fs::write(&rc_path, rc).expect("write rc");
    embed_resource::compile(&rc_path, embed_resource::NONE);
    println!("cargo:rustc-link-arg-bins=/MANIFEST:NO");
}
