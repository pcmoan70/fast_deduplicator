pub mod canon;
pub mod cr2;
pub mod cr3;
pub mod exif;
pub mod jpeg;
pub mod tiff;

use std::path::Path;

use crate::io::{FileReader, ReadRange};
use crate::meta::{FileKind, FileMeta};

#[derive(Debug, thiserror::Error)]
pub enum ParseError {
    #[error("unsupported file type")]
    Unsupported,
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    #[error("cr3: {0}")]
    Cr3(#[from] cr3::Cr3Error),
}

pub fn kind_from_ext(path: &Path) -> Option<FileKind> {
    let ext = path.extension()?.to_str()?.to_ascii_lowercase();
    match ext.as_str() {
        "cr3" => Some(FileKind::Cr3),
        "cr2" => Some(FileKind::Cr2),
        "jpg" | "jpeg" => Some(FileKind::Jpeg),
        "hif" | "heif" | "heic" => Some(FileKind::Heif),
        _ => None,
    }
}

/// Which embedded JPEG the main preview, sharpness scoring and tracking are
/// computed from. Thumbnails always use the embedded preview.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Source {
    /// The camera's embedded preview (Canon JPG: 1620px MPF appendix; CR3: PRVW).
    #[default]
    Embedded,
    /// The full-size image (JPG: the file itself; CR3: the native-res embedded
    /// JPEG). CR2 has none and falls back to its IFD0 preview.
    Full,
}

impl std::str::FromStr for Source {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "embedded" => Ok(Source::Embedded),
            "full" => Ok(Source::Full),
            _ => Err(format!("expected embedded|full, got {s}")),
        }
    }
}

/// Stage-A header parse: open, read header chunk, extract metadata and
/// preview byte ranges. Returns the meta plus bytes read (for benching).
pub fn parse_header(path: &Path) -> Result<(FileMeta, u64), ParseError> {
    let kind = kind_from_ext(path).ok_or(ParseError::Unsupported)?;
    let mut r = FileReader::open(path)?;
    let size = r.len();
    let mut meta = FileMeta::new(path.to_path_buf(), size, kind);
    let header = r.header().to_vec();
    meta.content_key =
        xxhash_rust::xxh3::xxh3_64(&header[..header.len().min(64 * 1024)]);
    match kind {
        FileKind::Cr3 => cr3::parse(&mut r, &header, &mut meta)?,
        FileKind::Cr2 => {
            if !cr2::is_cr2(&header) || !cr2::parse(&header, &mut meta) {
                return Err(ParseError::Unsupported);
            }
        }
        FileKind::Jpeg => {
            if !jpeg::parse(&header, size, &mut meta) {
                return Err(ParseError::Unsupported);
            }
        }
        FileKind::Heif => { /* M4: thumbnail item parse; metadata via nom-exif */ }
    }
    Ok((meta, r.bytes_read))
}

/// Stage-B extraction: the JPEG bytes to work from. `Embedded` = preview
/// (thumb fallback); `Full` = full-size JPEG, falling back to the preview
/// where a format has none (CR2). Second value = pixel dims if known.
pub fn extract_jpeg(
    meta: &FileMeta,
    source: Source,
) -> Result<Option<(Vec<u8>, u32, u32)>, ParseError> {
    let info = match source {
        Source::Embedded => meta.preview.or(meta.thumb),
        Source::Full => meta.fullsize.or(meta.preview).or(meta.thumb),
    };
    let Some(info) = info else {
        return Ok(None);
    };
    let mut r = FileReader::open(&meta.path)?;
    let buf = r.read_range(info.range.offset, info.range.len as usize)?;
    if info.is_bare_jpeg {
        return Ok(Some((buf, info.width, info.height)));
    }
    // CR3 preview uuid area: locate PRVW jpeg within
    if let Some((start, len, w, h)) = cr3::locate_prvw_jpeg(&buf) {
        return Ok(Some((buf[start..start + len].to_vec(), w, h)));
    }
    Ok(None)
}
