//! JPEG header parser: APP1 EXIF (via the TIFF walker) + SOF dimensions.
//! The file itself is its own preview/fullsize.

use super::exif;
use super::tiff::Tiff;
use crate::meta::{ByteRange, FileMeta, PreviewInfo};

pub fn parse(header: &[u8], file_size: u64, meta: &mut FileMeta) -> bool {
    if header.len() < 4 || header[0] != 0xFF || header[1] != 0xD8 {
        return false;
    }
    let mut pos = 2usize;
    while pos + 4 <= header.len() {
        if header[pos] != 0xFF {
            break;
        }
        let marker = header[pos + 1];
        // Standalone markers without length
        if (0xD0..=0xD9).contains(&marker) || marker == 0x01 {
            pos += 2;
            continue;
        }
        let seg_len = u16::from_be_bytes([header[pos + 2], header[pos + 3]]) as usize;
        if seg_len < 2 {
            break;
        }
        let body = pos + 4;
        match marker {
            0xE1 => {
                // APP1 Exif
                if header.get(body..body + 6).map(|s| s == b"Exif\0\0") == Some(true) {
                    let tiff_start = body + 6;
                    let tiff_end = (pos + 2 + seg_len).min(header.len());
                    if let Some(tiff_bytes) = header.get(tiff_start..tiff_end) {
                        if let Ok(t) = Tiff::parse(tiff_bytes) {
                            exif::apply_ifd0(&t, meta);
                        }
                    }
                }
            }
            // SOF0..SOF15 except DHT(C4)/JPG(C8)/DAC(CC)
            0xC0..=0xCF if marker != 0xC4 && marker != 0xC8 && marker != 0xCC => {
                if let Some(s) = header.get(body + 1..body + 5) {
                    meta.height = u16::from_be_bytes([s[0], s[1]]) as u32;
                    meta.width = u16::from_be_bytes([s[2], s[3]]) as u32;
                }
                break; // dims found; EXIF always precedes SOF in camera JPEGs
            }
            0xDA => break, // start of scan
            _ => {}
        }
        pos += 2 + seg_len;
    }
    let whole = PreviewInfo {
        range: ByteRange {
            offset: 0,
            len: file_size,
        },
        width: meta.width,
        height: meta.height,
        is_bare_jpeg: true,
    };
    meta.preview = Some(whole);
    meta.fullsize = Some(whole);
    true
}
