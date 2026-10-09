//! Developing and saving captured stills.

use crate::service::Captured;
use crate::settings::CaptureFormat;
use loupecam_isp::encode::{self, Format};
use serde::Serialize;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CaptureInfo {
    /// File name within the captures directory.
    pub name: String,
    /// Companion raw TIFF, if saved.
    pub raw_name: Option<String>,
    pub width: u32,
    pub height: u32,
    pub bytes: u64,
    pub exposure_us: f64,
    pub gain: f32,
    pub taken: String,
}

/// Develop, encode and write a capture into `dir`. Runs on a blocking thread.
pub fn save(c: &Captured, dir: &Path) -> anyhow::Result<CaptureInfo> {
    std::fs::create_dir_all(dir)?;
    let s = &c.settings.capture;
    let p = c.settings.develop_params(&c.frame, s.demosaic, &c.calibration);
    let format = match s.format {
        CaptureFormat::Png => Format::Png,
        CaptureFormat::Tiff => Format::Tiff,
        CaptureFormat::Jpeg => Format::Jpeg { quality: s.jpeg_quality },
    };
    let raw = &c.frame.raw;
    let (bytes, w, h) = if s.sixteen_bit && !matches!(format, Format::Jpeg { .. }) {
        let img = loupecam_isp::develop16(raw, &p);
        (encode::encode16(&img, format)?, img.width, img.height)
    } else {
        let img = loupecam_isp::develop(raw, &p);
        (encode::encode8(&img, format)?, img.width, img.height)
    };
    let now = jiff::Zoned::now();
    let stem = unique_stem(dir, &now.strftime("%Y%m%d-%H%M%S").to_string(), format.extension());
    let name = format!("{stem}.{}", format.extension());
    std::fs::write(dir.join(&name), &bytes)?;
    let raw_name = if s.save_raw {
        let n = format!("{stem}.raw.tiff");
        std::fs::write(dir.join(&n), encode::raw_tiff(raw)?)?;
        Some(n)
    } else {
        None
    };
    tracing::info!(file = %name, "saved capture");
    Ok(CaptureInfo {
        name,
        raw_name,
        width: w,
        height: h,
        bytes: bytes.len() as u64,
        exposure_us: c.exposure_us,
        gain: c.gain,
        taken: now.timestamp().to_string(),
    })
}

/// `stem`, or `stem-2`, `stem-3`… if a file with that stem already exists.
fn unique_stem(dir: &Path, stem: &str, ext: &str) -> String {
    let exists = |s: &str| dir.join(format!("{s}.{ext}")).exists();
    if !exists(stem) {
        return stem.to_string();
    }
    (2..).map(|n| format!("{stem}-{n}")).find(|s| !exists(s)).unwrap()
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CaptureEntry {
    pub name: String,
    pub bytes: u64,
    pub modified: String,
}

/// Image files in `dir`, newest first.
pub fn list(dir: &Path) -> std::io::Result<Vec<CaptureEntry>> {
    let mut v = Vec::new();
    let rd = match std::fs::read_dir(dir) {
        Ok(rd) => rd,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(v),
        Err(e) => return Err(e),
    };
    for e in rd.flatten() {
        let name = e.file_name().to_string_lossy().into_owned();
        let lower = name.to_ascii_lowercase();
        if !(lower.ends_with(".png") || lower.ends_with(".jpg") || lower.ends_with(".tiff") || lower.ends_with(".tif")) {
            continue;
        }
        let md = e.metadata()?;
        let modified = md
            .modified()
            .ok()
            .and_then(|t| jiff::Timestamp::try_from(t).ok())
            .map(|t| t.to_string())
            .unwrap_or_default();
        v.push(CaptureEntry { name, bytes: md.len(), modified });
    }
    v.sort_by(|a, b| b.modified.cmp(&a.modified));
    Ok(v)
}

/// Resolve a client-supplied file name inside `dir`, rejecting path traversal.
pub fn resolve(dir: &Path, name: &str) -> Option<PathBuf> {
    let ok = !name.is_empty()
        && !name.contains(['/', '\\'])
        && !name.starts_with('.')
        && name != ".."
        && Path::new(name).components().count() == 1;
    ok.then(|| dir.join(name)).filter(|p| p.is_file())
}
