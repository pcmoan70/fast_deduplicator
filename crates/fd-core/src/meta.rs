use std::path::PathBuf;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FileKind {
    Cr3,
    Cr2,
    Jpeg,
    Heif,
}

/// Absolute byte range within the source file.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ByteRange {
    pub offset: u64,
    pub len: u64,
}

#[derive(Debug, Clone, Copy)]
pub struct PreviewInfo {
    pub range: ByteRange,
    /// Pixel dimensions if known at header-parse time (0 = unknown).
    pub width: u32,
    pub height: u32,
    /// Range holds a bare JPEG stream (true) or a container area that still
    /// needs a local parse to locate the JPEG (false, e.g. CR3 PRVW uuid).
    pub is_bare_jpeg: bool,
}

/// Capture time with centisecond precision (EXIF SubSecTimeOriginal).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct Timestamp {
    pub unix_centis: i64,
}

impl Timestamp {
    /// Parse "YYYY:MM:DD HH:MM:SS" (+ optional subsec string) as local time.
    /// Timezone is irrelevant for burst grouping; only deltas matter.
    pub fn from_exif(dt: &str, subsec: Option<&str>) -> Option<Self> {
        let b = dt.as_bytes();
        if b.len() < 19 {
            return None;
        }
        let num = |s: &[u8]| -> Option<i64> {
            let s = std::str::from_utf8(s).ok()?.trim();
            s.parse().ok()
        };
        let (y, mo, d) = (num(&b[0..4])?, num(&b[5..7])?, num(&b[8..10])?);
        let (h, mi, s) = (num(&b[11..13])?, num(&b[14..16])?, num(&b[17..19])?);
        if !(1..=12).contains(&mo) || !(1..=31).contains(&d) {
            return None;
        }
        // days-from-civil (Howard Hinnant's algorithm)
        let (y2, mo2) = if mo <= 2 { (y - 1, mo + 12) } else { (y, mo) };
        let era = y2.div_euclid(400);
        let yoe = y2 - era * 400;
        let doy = (153 * (mo2 - 3) + 2) / 5 + d - 1;
        let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
        let days = era * 146097 + doe - 719468;
        let secs = days * 86400 + h * 3600 + mi * 60 + s;
        // SubSec is a string of decimal fraction digits; use first 2 (centis).
        let centis = subsec
            .map(|ss| {
                let t: String = ss.trim().chars().take(2).collect();
                let padded = format!("{:0<2}", t);
                padded.parse::<i64>().unwrap_or(0)
            })
            .unwrap_or(0);
        Some(Timestamp {
            unix_centis: secs * 100 + centis,
        })
    }
}

#[derive(Debug, Clone, Copy, Default)]
pub struct Exposure {
    /// Exposure time as (num, den), e.g. (1, 2000).
    pub time: Option<(u32, u32)>,
    pub f_number: Option<f32>,
    pub iso: Option<u32>,
    pub focal_mm: Option<f32>,
}

#[derive(Debug, Clone)]
pub struct FileMeta {
    pub path: PathBuf,
    pub size: u64,
    pub kind: FileKind,
    pub make: Option<String>,
    pub model: Option<String>,
    pub serial: Option<String>,
    pub lens: Option<String>,
    pub ts: Option<Timestamp>,
    pub exposure: Exposure,
    /// EXIF orientation (1 = normal).
    pub orientation: u16,
    /// Full image dimensions (0 = unknown).
    pub width: u32,
    pub height: u32,
    /// Camera file number from the filename digits (e.g. 4P4A9770 -> 9770).
    pub file_number: Option<u32>,
    pub thumb: Option<PreviewInfo>,
    pub preview: Option<PreviewInfo>,
    pub fullsize: Option<PreviewInfo>,
}

impl FileMeta {
    pub fn new(path: PathBuf, size: u64, kind: FileKind) -> Self {
        let file_number = path
            .file_stem()
            .and_then(|s| s.to_str())
            .map(|s| {
                let digits: String = s
                    .chars()
                    .rev()
                    .take_while(|c| c.is_ascii_digit())
                    .collect::<Vec<_>>()
                    .into_iter()
                    .rev()
                    .collect();
                digits
            })
            .filter(|d| !d.is_empty())
            .and_then(|d| d.parse().ok());
        FileMeta {
            path,
            size,
            kind,
            make: None,
            model: None,
            serial: None,
            lens: None,
            ts: None,
            exposure: Exposure::default(),
            orientation: 1,
            width: 0,
            height: 0,
            file_number,
            thumb: None,
            preview: None,
            fullsize: None,
        }
    }
}
