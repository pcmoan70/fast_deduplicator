//! CR3 (ISO-BMFF) header parser. Reads only the moov area plus box headers:
//! EXIF from CMT1/CMT2, thumbnail from THMB, preview byte range from CTBO,
//! full-resolution embedded JPEG range from the trak sample tables.
//! Raw CRX track data is never touched.

use super::exif;
use super::tiff::Tiff;
use crate::io::ReadRange;
use crate::meta::{ByteRange, FileMeta, PreviewInfo};

/// Canon metadata uuid inside moov.
const UUID_CANON_META: [u8; 16] = [
    0x85, 0xc0, 0xb6, 0x87, 0x82, 0x0f, 0x11, 0xe0, 0x81, 0x11, 0xf4, 0xce, 0x46, 0x2b, 0x6a, 0x48,
];
/// Preview uuid (top level, holds PRVW box).
const UUID_PREVIEW: [u8; 16] = [
    0xea, 0xf4, 0x2b, 0x5e, 0x1c, 0x98, 0x4b, 0x88, 0xb9, 0xfb, 0xb7, 0xdc, 0x40, 0x6e, 0x4d, 0x16,
];

#[derive(Debug, thiserror::Error)]
pub enum Cr3Error {
    #[error("not a CR3 file")]
    NotCr3,
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
}

struct Box_ {
    kind: [u8; 4],
    /// Absolute file offset of the payload (after size+type[+largesize]).
    payload_off: u64,
    payload_len: u64,
}

fn be32(b: &[u8], off: usize) -> Option<u32> {
    b.get(off..off + 4)
        .map(|s| u32::from_be_bytes([s[0], s[1], s[2], s[3]]))
}

fn be64(b: &[u8], off: usize) -> Option<u64> {
    b.get(off..off + 8).map(|s| {
        u64::from_be_bytes([s[0], s[1], s[2], s[3], s[4], s[5], s[6], s[7]])
    })
}

/// Iterate boxes within `buf` (which starts at absolute offset `base`).
fn walk(buf: &[u8], base: u64) -> Vec<Box_> {
    let mut out = Vec::new();
    let mut pos = 0usize;
    while pos + 8 <= buf.len() {
        let size32 = be32(buf, pos).unwrap() as u64;
        let kind = [buf[pos + 4], buf[pos + 5], buf[pos + 6], buf[pos + 7]];
        let (hdr, size) = if size32 == 1 {
            match be64(buf, pos + 8) {
                Some(s) => (16u64, s),
                None => break,
            }
        } else if size32 == 0 {
            (8u64, (buf.len() - pos) as u64) // box extends to end
        } else {
            (8u64, size32)
        };
        if size < hdr {
            break;
        }
        out.push(Box_ {
            kind,
            payload_off: base + pos as u64 + hdr,
            payload_len: size - hdr,
        });
        pos += size as usize;
    }
    out
}

fn find<'a>(boxes: &'a [Box_], kind: &[u8; 4]) -> Option<&'a Box_> {
    boxes.iter().find(|b| &b.kind == kind)
}

/// Locate a JPEG SOI marker within the first `window` bytes of `buf`.
fn find_soi(buf: &[u8], window: usize) -> Option<usize> {
    let end = window.min(buf.len().saturating_sub(1));
    (0..end).find(|&i| buf[i] == 0xFF && buf[i + 1] == 0xD8)
}

pub fn parse<R: ReadRange>(r: &mut R, header: &[u8], meta: &mut FileMeta) -> Result<(), Cr3Error> {
    // ftyp brand check
    if header.len() < 12 || &header[4..8] != b"ftyp" || &header[8..12] != b"crx " {
        return Err(Cr3Error::NotCr3);
    }
    let top = walk(header, 0);
    let moov = find(&top, b"moov").ok_or(Cr3Error::NotCr3)?;

    // Ensure the whole moov payload is in memory.
    let moov_buf: Vec<u8>;
    let moov_bytes: &[u8] = if (moov.payload_off + moov.payload_len) as usize <= header.len() {
        &header[moov.payload_off as usize..(moov.payload_off + moov.payload_len) as usize]
    } else {
        moov_buf = r.read_range(moov.payload_off, moov.payload_len as usize)?;
        &moov_buf
    };
    let moov_children = walk(moov_bytes, moov.payload_off);
    let rel = |b: &Box_| -> (usize, usize) {
        let s = (b.payload_off - moov.payload_off) as usize;
        (s, s + b.payload_len as usize)
    };

    // --- Canon metadata uuid: CMT1/CMT2 EXIF, THMB, CTBO ---
    let mut preview_range: Option<ByteRange> = None;
    for b in moov_children.iter().filter(|b| &b.kind == b"uuid") {
        let (s, e) = rel(b);
        let payload = &moov_bytes[s..e];
        if payload.len() < 16 || payload[..16] != UUID_CANON_META {
            continue;
        }
        let inner = walk(&payload[16..], b.payload_off + 16);
        for ib in &inner {
            let is = (ib.payload_off - moov.payload_off) as usize;
            let ie = is + ib.payload_len as usize;
            let ibuf = &moov_bytes[is..ie.min(moov_bytes.len())];
            match &ib.kind {
                b"CMT1" => {
                    if let Ok(t) = Tiff::parse(ibuf) {
                        exif::apply_ifd0(&t, meta);
                    }
                }
                b"CMT2" => {
                    if let Ok(t) = Tiff::parse(ibuf) {
                        exif::apply_exif_ifd(&t, meta);
                    }
                }
                b"THMB" => {
                    // FullBox: ver/flags(4) w(2) h(2) jpeg_size(4) ...
                    let w = be32(ibuf, 4).map(|v| (v >> 16) as u32).unwrap_or(0);
                    let h = be32(ibuf, 4).map(|v| (v & 0xFFFF) as u32).unwrap_or(0);
                    let jpeg_size = be32(ibuf, 8).unwrap_or(0) as u64;
                    if let Some(soi) = find_soi(&ibuf[12..], 16) {
                        let off = ib.payload_off + 12 + soi as u64;
                        let len = jpeg_size.min(ib.payload_len - 12 - soi as u64);
                        if len > 0 {
                            meta.thumb = Some(PreviewInfo {
                                range: ByteRange { offset: off, len },
                                width: w,
                                height: h,
                                is_bare_jpeg: true,
                            });
                        }
                    }
                }
                b"CTBO" => {
                    // u32 count, then {u32 index, u64 offset, u64 size}
                    let count = be32(ibuf, 0).unwrap_or(0) as usize;
                    for i in 0..count {
                        let p = 4 + i * 20;
                        let (Some(idx), Some(off), Some(sz)) =
                            (be32(ibuf, p), be64(ibuf, p + 4), be64(ibuf, p + 12))
                        else {
                            break;
                        };
                        // index 2 = preview uuid box (contains PRVW)
                        if idx == 2 && sz > 0 {
                            preview_range = Some(ByteRange {
                                offset: off,
                                len: sz,
                            });
                        }
                    }
                }
                _ => {}
            }
        }
    }

    // --- traks: locate full-size embedded JPEG + image dimensions ---
    let mut best_dims = (0u32, 0u32);
    for trak in moov_children.iter().filter(|b| &b.kind == b"trak") {
        let (s, e) = rel(trak);
        if let Some(info) = parse_trak(&moov_bytes[s..e], trak.payload_off) {
            if info.width as u64 * info.height as u64
                > best_dims.0 as u64 * best_dims.1 as u64
            {
                best_dims = (info.width, info.height);
            }
            if info.is_jpeg {
                meta.fullsize = Some(PreviewInfo {
                    range: info.sample0,
                    width: info.width,
                    height: info.height,
                    is_bare_jpeg: true,
                });
            }
        }
    }
    if meta.width == 0 {
        meta.width = best_dims.0;
        meta.height = best_dims.1;
    }

    // --- preview range: from CTBO, else scan top-level boxes ---
    if preview_range.is_none() {
        for b in &top {
            if &b.kind == b"uuid" {
                let s = b.payload_off as usize;
                if let Some(pl) = header.get(s..s + 16) {
                    if pl == UUID_PREVIEW {
                        preview_range = Some(ByteRange {
                            offset: b.payload_off - 8,
                            len: b.payload_len + 8,
                        });
                    }
                }
            }
        }
    }
    if let Some(range) = preview_range {
        meta.preview = Some(PreviewInfo {
            range,
            width: 0,
            height: 0,
            is_bare_jpeg: false, // needs PRVW-local parse at extraction
        });
    }
    Ok(())
}

struct TrakInfo {
    is_jpeg: bool,
    width: u32,
    height: u32,
    sample0: ByteRange,
}

/// Descend trak/mdia/minf/stbl; read stsd (format), stsz (size), co64/stco.
fn parse_trak(buf: &[u8], base: u64) -> Option<TrakInfo> {
    let mut cur: &[u8] = buf;
    let mut cur_base = base;
    for path in [b"mdia", b"minf", b"stbl"] {
        let children = walk(cur, cur_base);
        let b = find(&children, path)?;
        let s = (b.payload_off - cur_base) as usize;
        let e = s + b.payload_len as usize;
        cur = cur.get(s..e)?;
        cur_base = b.payload_off;
    }
    let stbl = walk(cur, cur_base);
    let stbl_slice = |b: &Box_| -> Option<&[u8]> {
        let s = (b.payload_off - cur_base) as usize;
        cur.get(s..s + b.payload_len as usize)
    };

    // stsd: ver/flags(4) entry_count(4) entries...
    let stsd = stbl_slice(find(&stbl, b"stsd")?)?;
    let entry = stsd.get(8..)?;
    // VisualSampleEntry: width/height at +32/+34 from entry start
    let width = be32(entry, 32).map(|v| (v >> 16) as u32).unwrap_or(0);
    let height = be32(entry, 32).map(|v| (v & 0xFFFF) as u32).unwrap_or(0);
    // Full-size JPEG track: sample entry format CRAW containing a JPEG box
    // (CRX tracks contain CMP1 instead). Scan is robust across layouts.
    let is_jpeg = entry
        .windows(4)
        .take(256)
        .any(|w| w == b"JPEG")
        && !entry.windows(4).take(256).any(|w| w == b"CMP1");

    // stsz: ver/flags(4) sample_size(4) count(4) [sizes...]
    let stsz = stbl_slice(find(&stbl, b"stsz")?)?;
    let fixed = be32(stsz, 4)?;
    let size0 = if fixed != 0 { fixed } else { be32(stsz, 12)? };

    // co64 (u64 offsets) or stco (u32)
    let off0 = if let Some(b) = find(&stbl, b"co64") {
        be64(stbl_slice(b)?, 8)?
    } else if let Some(b) = find(&stbl, b"stco") {
        be32(stbl_slice(b)?, 8)? as u64
    } else {
        return None;
    };

    Some(TrakInfo {
        is_jpeg,
        width,
        height,
        sample0: ByteRange {
            offset: off0,
            len: size0 as u64,
        },
    })
}

/// Parse a preview-uuid area (as read from `FileMeta::preview.range`) and
/// return the bare JPEG range *within* that buffer plus dimensions.
pub fn locate_prvw_jpeg(buf: &[u8]) -> Option<(usize, usize, u32, u32)> {
    let prvw = buf.windows(4).take(128).position(|w| w == b"PRVW")?;
    // From 'PRVW': +4 ver/flags? layout: u32 unk, u16 unk, u16 w, u16 h,
    // u16 unk, u32 jpeg_size, jpeg. Offsets relative to tag start:
    let at = |o: usize| -> Option<usize> { Some(prvw + o) };
    let rd16 = |o: usize| -> Option<u32> {
        let i = at(o)?;
        buf.get(i..i + 2)
            .map(|s| u16::from_be_bytes([s[0], s[1]]) as u32)
    };
    let rd32 = |o: usize| -> Option<u32> {
        let i = at(o)?;
        buf.get(i..i + 4)
            .map(|s| u32::from_be_bytes([s[0], s[1], s[2], s[3]]))
    };
    let w = rd16(10)?;
    let h = rd16(12)?;
    let jpeg_size = rd32(16)? as usize;
    // JPEG should start right after the 20-byte PRVW header; scan as guard.
    let search_from = prvw + 20;
    let soi = find_soi(buf.get(search_from..)?, 32)? + search_from;
    let len = jpeg_size.min(buf.len() - soi);
    Some((soi, len, w, h))
}
