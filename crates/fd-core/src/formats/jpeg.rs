//! JPEG header parser: APP1 EXIF (via the TIFF walker), SOF dimensions, and
//! the MPF (Multi-Picture Format) APP2 index. Canon EOS JPEGs append a
//! 1620x1080 preview image at the end of the file; scoring reads that
//! (~300 KB) instead of decoding the 45 MP main image.

use super::exif;
use super::tiff::Tiff;
use crate::meta::{ByteRange, FileMeta, PreviewInfo};

pub fn parse(header: &[u8], file_size: u64, meta: &mut FileMeta) -> bool {
    if header.len() < 4 || header[0] != 0xFF || header[1] != 0xD8 {
        return false;
    }
    let mut mpf_preview: Option<ByteRange> = None;
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
            0xE2 => {
                // APP2 MPF: index of appended images (Canon: 1620px preview)
                if header.get(body..body + 4).map(|s| s == b"MPF\0") == Some(true) {
                    mpf_preview = parse_mpf(header, body + 4, file_size);
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
    meta.preview = Some(match mpf_preview {
        Some(range) => PreviewInfo {
            range,
            width: 0,
            height: 0,
            is_bare_jpeg: true,
        },
        None => whole,
    });
    meta.fullsize = Some(whole);
    true
}

/// MPF index: TIFF structure at `tiff_start`; tag 0xB002 holds 16-byte MP
/// entries (attr, size, offset). Offsets are relative to the TIFF header;
/// image 0 (offset 0) is the primary. Returns the largest non-primary image.
fn parse_mpf(header: &[u8], tiff_start: usize, file_size: u64) -> Option<ByteRange> {
    let t = Tiff::parse(header.get(tiff_start..)?).ok()?;
    let (entries, _) = t.ifd(t.first_ifd);
    let e = entries.iter().find(|e| e.tag == 0xB002)?;
    let v = t.value_bytes(e)?;
    let rd32 = |b: &[u8], o: usize| -> Option<u32> {
        let s = b.get(o..o + 4)?;
        Some(if t.le {
            u32::from_le_bytes([s[0], s[1], s[2], s[3]])
        } else {
            u32::from_be_bytes([s[0], s[1], s[2], s[3]])
        })
    };
    let mut best: Option<ByteRange> = None;
    for j in 0..(e.count as usize / 16) {
        let size = rd32(v, j * 16 + 4)? as u64;
        let off = rd32(v, j * 16 + 8)? as u64;
        if off == 0 {
            continue; // primary image
        }
        let abs = tiff_start as u64 + off;
        if abs + size <= file_size && best.map(|b| size > b.len).unwrap_or(true) {
            best = Some(ByteRange {
                offset: abs,
                len: size,
            });
        }
    }
    best
}
