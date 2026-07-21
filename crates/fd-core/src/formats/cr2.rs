//! CR2 (TIFF-based) header parser. IFD0 holds a reduced/full-size JPEG via
//! StripOffsets/StripByteCounts; IFD1 holds the 160x120 thumbnail. Raw data
//! (IFD3) is never touched.

use super::exif;
use super::tiff::Tiff;
use crate::meta::{ByteRange, FileMeta, PreviewInfo};

const TAG_STRIP_OFFSETS: u16 = 0x0111;
const TAG_STRIP_BYTE_COUNTS: u16 = 0x0117;
const TAG_JPEG_IF: u16 = 0x0201;
const TAG_JPEG_IF_LEN: u16 = 0x0202;

pub fn is_cr2(header: &[u8]) -> bool {
    header.len() >= 12 && &header[8..10] == b"CR" && header[10] == 2
}

pub fn parse(header: &[u8], meta: &mut FileMeta) -> bool {
    let Ok(t) = Tiff::parse(header) else {
        return false;
    };
    exif::apply_ifd0(&t, meta);

    // IFD0 strip = embedded JPEG preview (large)
    let (ifd0, next) = t.ifd(t.first_ifd);
    let mut strip: (Option<u32>, Option<u32>) = (None, None);
    for e in &ifd0 {
        match e.tag {
            TAG_STRIP_OFFSETS => strip.0 = t.uint(e),
            TAG_STRIP_BYTE_COUNTS => strip.1 = t.uint(e),
            _ => {}
        }
    }
    if let (Some(off), Some(len)) = strip {
        meta.preview = Some(PreviewInfo {
            range: ByteRange {
                offset: off as u64,
                len: len as u64,
            },
            width: 0,
            height: 0,
            is_bare_jpeg: true,
        });
    }

    // IFD1 = thumbnail
    if next != 0 {
        let (ifd1, _) = t.ifd(next);
        let mut th: (Option<u32>, Option<u32>) = (None, None);
        for e in &ifd1 {
            match e.tag {
                TAG_JPEG_IF => th.0 = t.uint(e),
                TAG_JPEG_IF_LEN => th.1 = t.uint(e),
                _ => {}
            }
        }
        if let (Some(off), Some(len)) = th {
            meta.thumb = Some(PreviewInfo {
                range: ByteRange {
                    offset: off as u64,
                    len: len as u64,
                },
                width: 0,
                height: 0,
                is_bare_jpeg: true,
            });
        }
    }
    true
}
