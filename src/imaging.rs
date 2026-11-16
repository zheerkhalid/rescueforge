//! Copying a whole drive into an image file, so that a failing disk only has
//! to be read once and every later scan works on the copy.

use crate::control::{Control, Progress};
use crate::source::Source;
use std::fs::File;
use std::io::Write;
use std::path::{Path, PathBuf};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ImageReport {
    pub path: PathBuf,
    pub bytes: u64,
    /// Bytes that could not be read and are zeros in the image.
    pub bad_bytes: u64,
    pub bad_areas: usize,
}

/// Reads `src` from start to end into a new file at `dest`. Unreadable
/// sectors become zeros. Stopping removes the incomplete file.
pub fn create_image(
    src: &Source,
    dest: &Path,
    ctl: &Control,
    progress: &Progress,
) -> Result<ImageReport, String> {
    const CHUNK: usize = 4 << 20;
    let total = src.len();
    let mut out =
        File::create(dest).map_err(|e| format!("Could not create \"{}\": {e}", dest.display()))?;
    let fail = |out: File, text: String| -> Result<ImageReport, String> {
        drop(out);
        let _ = std::fs::remove_file(dest);
        Err(text)
    };
    progress.set_phase("Copying the drive into an image file");
    progress.set(0, total);
    let bad_before = src.log().bad_bytes();
    let areas_before = src.log().ranges().len();
    let mut buf = vec![0u8; CHUNK.min(total.max(1) as usize)];
    let mut pos = 0u64;
    while pos < total {
        if ctl.checkpoint() {
            return fail(
                out,
                "Stopped. The incomplete image was removed.".to_string(),
            );
        }
        let n = src.read_at(pos, &mut buf);
        if n == 0 {
            break;
        }
        if let Err(e) = out.write_all(&buf[..n]) {
            return fail(
                out,
                format!("Writing the image failed: {e}. The incomplete image was removed."),
            );
        }
        pos += n as u64;
        progress.set(pos, total);
    }
    if let Err(e) = out.flush().and_then(|_| out.sync_all()) {
        return fail(out, format!("Writing the image failed: {e}"));
    }
    progress.set_phase("Finished");
    Ok(ImageReport {
        path: dest.to_path_buf(),
        bytes: pos,
        bad_bytes: src.log().bad_bytes() - bad_before,
        bad_areas: src.log().ranges().len().saturating_sub(areas_before),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::source::FlakySource;
    use std::sync::Arc;

    #[test]
    fn images_a_source_and_survives_bad_sectors() {
        let dir = std::env::temp_dir().join(format!("rescueforge-imaging-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let data: Vec<u8> = (0..9_000_000u32).map(|i| (i % 249) as u8).collect();
        let src = Source::new(Arc::new(FlakySource {
            data: data.clone(),
            bad: vec![(5_000_192, 2048)],
            fail_first: false,
            attempts: Default::default(),
        }));
        let path = dir.join("copy.img");
        let progress = Progress::new();
        let report = create_image(&src, &path, &Control::new(), &progress).unwrap();
        assert_eq!(report.bytes, data.len() as u64);
        assert_eq!(report.bad_bytes, 2048);
        assert_eq!(report.bad_areas, 1);
        assert_eq!(progress.fraction(), 1.0);
        let copy = std::fs::read(&path).unwrap();
        assert_eq!(copy.len(), data.len());
        assert_eq!(&copy[..5_000_192], &data[..5_000_192]);
        assert!(copy[5_000_192..5_002_240].iter().all(|b| *b == 0));
        assert_eq!(&copy[5_002_240..], &data[5_002_240..]);
        // The image opens as a source again.
        let reopened = crate::source::open_image(&path).unwrap();
        assert_eq!(reopened.len(), data.len() as u64);

        // Stopping removes the partial file.
        let ctl = Control::new();
        ctl.stop();
        let partial = dir.join("partial.img");
        assert!(create_image(&src, &partial, &ctl, &Progress::new()).is_err());
        assert!(!partial.exists());
        assert!(create_image(
            &src,
            &dir.join("no").join("such").join("x.img"),
            &Control::new(),
            &Progress::new()
        )
        .is_err());
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
