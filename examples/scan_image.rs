//! Scans a disk image and prints what was found, one file per line:
//! `status <tab> recoverability <tab> size <tab> crc32 <tab> path`.
//!
//! Usage: cargo run --example scan_image -- <image> [--deep] [--existing] [--all-space]
use rescueforge::recover::read_file_range;
use rescueforge::scan::{run_collect, ScanOptions};
use rescueforge::util::Crc32;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let Some(path) = args.first() else {
        eprintln!("usage: scan_image <image> [--deep] [--existing] [--all-space]");
        std::process::exit(2);
    };
    let has = |f: &str| args.iter().any(|a| a == f);
    let src = match rescueforge::source::open_image(std::path::Path::new(path)) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("{e}");
            std::process::exit(1);
        }
    };
    let opts = ScanOptions {
        deep: has("--deep"),
        include_existing: has("--existing"),
        free_only: !has("--all-space"),
        ..Default::default()
    };
    let (files, log) = run_collect(&src, &opts);
    for line in log {
        eprintln!("# {line}");
    }
    let mut buf = vec![0u8; 1 << 20];
    for f in &files {
        let mut crc = Crc32::new();
        let mut pos = 0u64;
        while pos < f.size {
            let n = read_file_range(&src, f, pos, &mut buf);
            if n == 0 {
                break;
            }
            crc.update(&buf[..n]);
            pos += n as u64;
        }
        let full = if f.path.is_empty() {
            f.name.clone()
        } else {
            format!("{}/{}", f.path, f.name)
        };
        println!(
            "{}\t{:?}\t{}\t{}\t{}",
            f.status.label(),
            f.chance,
            f.size,
            crc.finish(),
            full
        );
    }
}
