//! Checks the deep scan against real files: every file given on the command
//! line is copied into an in-memory "disk" (cluster-aligned, with random data
//! between the files), the disk is deep-scanned, and each file must come back
//! with exactly its own bytes.
//!
//! Usage: cargo run --release --example carve_check -- <file>...
use rescueforge::recover::read_file_range;
use rescueforge::scan::{run_collect, ScanOptions};
use rescueforge::source::Source;
use rescueforge::util::Rng;

fn main() {
    let mut rng = Rng(0x1234_5678);
    let mut disk = rng.bytes(8192);
    let mut placed = Vec::new();
    for path in std::env::args().skip(1) {
        let Ok(data) = std::fs::read(&path) else {
            eprintln!("cannot read {path}");
            continue;
        };
        if data.is_empty() || disk.len() + data.len() > 1500 << 20 {
            continue;
        }
        disk.resize(disk.len().div_ceil(4096) * 4096, 0);
        placed.push((disk.len() as u64, data.len() as u64, path));
        disk.extend_from_slice(&data);
        let gap = 4096 + rng.below(3) as usize * 4096;
        let fill = rng.bytes(disk.len().div_ceil(4096) * 4096 - disk.len() + gap);
        disk.extend_from_slice(&fill);
    }
    let src = Source::from_bytes(disk);
    let opts = ScanOptions {
        deep: true,
        carve_off: Vec::new(),
        ..Default::default()
    };
    let started = std::time::Instant::now();
    let (files, _) = run_collect(&src, &opts);
    let secs = started.elapsed().as_secs_f64();
    let (mut exact, mut longer, mut shorter, mut missed) = (0, 0, 0, 0);
    for (off, len, path) in &placed {
        let name = std::path::Path::new(path)
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        match files.iter().find(|f| f.first_offset() == Some(*off)) {
            Some(f) => {
                let mut buf = vec![0u8; f.size.min(*len) as usize];
                read_file_range(&src, f, 0, &mut buf);
                let verdict = if f.size == *len {
                    exact += 1;
                    "exact"
                } else if f.size > *len {
                    longer += 1;
                    "LONGER"
                } else {
                    shorter += 1;
                    "SHORTER"
                };
                println!(
                    "{verdict:8} {:>10} -> {:>10} .{:5} {:?} {name}",
                    len,
                    f.size,
                    f.ext(),
                    f.chance
                );
            }
            None => {
                missed += 1;
                println!("MISSED   {len:>10}                     {name}");
            }
        }
    }
    let extra = files
        .iter()
        .filter(|f| !placed.iter().any(|p| Some(p.0) == f.first_offset()))
        .count();
    println!(
        "{} files: {exact} exact, {longer} longer, {shorter} shorter, {missed} missed; {extra} other finds; {:.0} MB/s",
        placed.len(),
        src.len() as f64 / 1048576.0 / secs
    );
}
