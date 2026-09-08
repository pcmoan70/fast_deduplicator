//! EXIF tag extraction over the TIFF walker. Fills FileMeta fields from an
//! IFD0-rooted structure (CR2, JPEG APP1, CR3 CMT1) or a bare Exif IFD
//! (CR3 CMT2, whose first IFD *is* the Exif IFD).

use super::tiff::Tiff;
use crate::meta::{FileMeta, Timestamp};

pub const TAG_MAKE: u16 = 0x010F;
pub const TAG_MODEL: u16 = 0x0110;
pub const TAG_ORIENTATION: u16 = 0x0112;
pub const TAG_EXIF_IFD: u16 = 0x8769;
pub const TAG_DATETIME_ORIGINAL: u16 = 0x9003;
pub const TAG_SUBSEC_ORIGINAL: u16 = 0x9291;
pub const TAG_EXPOSURE_TIME: u16 = 0x829A;
pub const TAG_F_NUMBER: u16 = 0x829D;
pub const TAG_ISO: u16 = 0x8827;
pub const TAG_FOCAL_LENGTH: u16 = 0x920A;
pub const TAG_PIXEL_X: u16 = 0xA002;
pub const TAG_PIXEL_Y: u16 = 0xA003;
pub const TAG_BODY_SERIAL: u16 = 0xA431;
pub const TAG_LENS_MODEL: u16 = 0xA434;
pub const TAG_MAKER_NOTE: u16 = 0x927C;

/// Read IFD0 tags (make/model/orientation) and, if present, descend into the
/// Exif IFD. Returns the Exif IFD offset if the caller wants more.
pub fn apply_ifd0(t: &Tiff, meta: &mut FileMeta) -> Option<u32> {
    let (entries, _) = t.ifd(t.first_ifd);
    let mut exif_ifd = None;
    for e in &entries {
        match e.tag {
            TAG_MAKE => meta.make = t.ascii(e),
            TAG_MODEL => meta.model = t.ascii(e),
            TAG_ORIENTATION => meta.orientation = t.uint(e).unwrap_or(1) as u16,
            TAG_EXIF_IFD => exif_ifd = t.uint(e),
            _ => {}
        }
    }
    if let Some(off) = exif_ifd {
        apply_exif_ifd_at(t, off, meta);
    }
    exif_ifd
}

/// Apply Exif-IFD tags when the TIFF's first IFD is itself the Exif IFD
/// (CR3 CMT2).
pub fn apply_exif_ifd(t: &Tiff, meta: &mut FileMeta) {
    apply_exif_ifd_at(t, t.first_ifd, meta);
}

fn apply_exif_ifd_at(t: &Tiff, offset: u32, meta: &mut FileMeta) {
    let (entries, _) = t.ifd(offset);
    let mut dt: Option<String> = None;
    let mut subsec: Option<String> = None;
    for e in &entries {
        match e.tag {
            TAG_DATETIME_ORIGINAL => dt = t.ascii(e),
            TAG_SUBSEC_ORIGINAL => subsec = t.ascii(e),
            TAG_EXPOSURE_TIME => meta.exposure.time = t.rational(e),
            TAG_F_NUMBER => {
                meta.exposure.f_number = t
                    .rational(e)
                    .filter(|&(_, d)| d != 0)
                    .map(|(n, d)| n as f32 / d as f32)
            }
            TAG_ISO => meta.exposure.iso = t.uint(e),
            TAG_FOCAL_LENGTH => {
                meta.exposure.focal_mm = t
                    .rational(e)
                    .filter(|&(_, d)| d != 0)
                    .map(|(n, d)| n as f32 / d as f32)
            }
            TAG_PIXEL_X => {
                if meta.width == 0 {
                    meta.width = t.uint(e).unwrap_or(0)
                }
            }
            TAG_PIXEL_Y => {
                if meta.height == 0 {
                    meta.height = t.uint(e).unwrap_or(0)
                }
            }
            TAG_BODY_SERIAL => meta.serial = t.ascii(e),
            TAG_LENS_MODEL => meta.lens = t.ascii(e),
            // Canon's MakerNote is a bare IFD whose offsets share this TIFF.
            TAG_MAKER_NOTE => {
                if meta.make.as_deref().is_some_and(|m| m.starts_with("Canon")) {
                    if let Some(off) = t.value_offset(e) {
                        super::canon::apply_makernote(t, off, meta);
                    }
                }
            }
            _ => {}
        }
    }
    if let Some(dt) = dt {
        meta.ts = Timestamp::from_exif(&dt, subsec.as_deref());
    }
}
