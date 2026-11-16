#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use eframe::egui;
use rescueforge::ui::{self, Options};
use std::path::PathBuf;

const USAGE: &str = "RescueForge

  RescueForge.exe                      open the program
  RescueForge.exe --demo               show an invented disk; nothing on this PC is read
  RescueForge.exe --screenshot <png>   render the window to a PNG and exit (use with --demo)
        [--view home|home-quick|scanning|results|preview|preview-text|bigpreview|grid|
                recover|recover-blocked|recover-running|recover-done|images|imaging|log|
                settings|welcome|shortcuts|about] [--light]
        (the view can also be set with the RESCUEFORGE_VIEW environment variable)
  RescueForge.exe <image> [--deep]     open a disk image and scan it right away
  RescueForge.exe --selftest           build disk images in a temp folder, delete files in
                                       them, scan, recover and compare every byte
  RescueForge.exe --list               list the drives and disks RescueForge can see";

/// Windowed programs have no console; attach to the one that started us so
/// that the command-line modes can print.
fn attach_console() {
    #[link(name = "kernel32")]
    extern "system" {
        fn AttachConsole(process: u32) -> i32;
    }
    unsafe {
        AttachConsole(u32::MAX);
    }
}

fn main() -> eframe::Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let has = |flag: &str| args.iter().any(|a| a == flag);
    let value = |flag: &str| {
        args.iter()
            .position(|a| a == flag)
            .and_then(|i| args.get(i + 1))
            .cloned()
    };

    if has("--help") || has("-h") || has("/?") {
        attach_console();
        println!("{USAGE}");
        return Ok(());
    }
    if has("--selftest") {
        attach_console();
        match rescueforge::selftest::run() {
            Ok(log) => {
                for line in log {
                    println!("{line}");
                }
                println!("SELFTEST PASSED");
                std::process::exit(0);
            }
            Err(e) => {
                println!("{e}");
                println!("SELFTEST FAILED");
                std::process::exit(1);
            }
        }
    }
    if has("--list") {
        attach_console();
        for line in rescueforge::selftest::list_sources() {
            println!("{line}");
        }
        return Ok(());
    }

    let options = Options {
        demo: has("--demo"),
        screenshot: value("--screenshot").map(PathBuf::from),
        view: value("--view").or_else(|| std::env::var("RESCUEFORGE_VIEW").ok()),
        light: has("--light"),
        // "RescueForge.exe disk.img" (or --open disk.img) scans an image right away.
        open: value("--open").map(PathBuf::from).or_else(|| {
            args.first()
                .filter(|a| !a.starts_with("--"))
                .map(PathBuf::from)
                .filter(|p| p.is_file())
        }),
        deep: has("--deep"),
    };
    ui::run(options, window_icon())
}

fn window_icon() -> egui::IconData {
    let bytes = include_bytes!(concat!(env!("OUT_DIR"), "/icon-256.png"));
    match image::load_from_memory(bytes) {
        Ok(img) => {
            let rgba = img.into_rgba8();
            let (width, height) = rgba.dimensions();
            egui::IconData {
                rgba: rgba.into_raw(),
                width,
                height,
            }
        }
        Err(_) => egui::IconData::default(),
    }
}
