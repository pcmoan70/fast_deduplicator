//! Golden tests against real camera files in fixtures/ (populated by
//! scripts/fetch_fixtures.sh). Tests skip when a fixture is absent so CI
//! without fixtures still passes the pure-logic tests.

use std::path::PathBuf;

use fd_core::formats::{self, Source};
use fd_core::meta::{FileKind, Timestamp};

fn fixture(name: &str) -> Option<PathBuf> {
    let p = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../fixtures")
        .join(name);
    (p.exists() && p.metadata().map(|m| m.len() > 0).unwrap_or(false)).then_some(p)
}

#[test]
fn timestamp_parse() {
    let t = Timestamp::from_exif("2025:05:03 15:12:50", Some("07")).unwrap();
    assert_eq!(t.unix_centis, 1746285170_07);
    let t2 = Timestamp::from_exif("2025:05:03 15:12:50", None).unwrap();
    assert_eq!(t2.unix_centis, 1746285170_00);
    // single-digit subsec means tenths
    let t3 = Timestamp::from_exif("2025:05:03 15:12:50", Some("5")).unwrap();
    assert_eq!(t3.unix_centis, 1746285170_50);
    assert!(Timestamp::from_exif("garbage", None).is_none());
}

#[test]
fn file_number_from_name() {
    use fd_core::meta::FileMeta;
    let m = FileMeta::new(PathBuf::from("/x/4P4A9770.CR3"), 1, FileKind::Cr3);
    assert_eq!(m.file_number, Some(9770));
    let m = FileMeta::new(PathBuf::from("/x/_P4A0001.CR3"), 1, FileKind::Cr3);
    assert_eq!(m.file_number, Some(1));
    let m = FileMeta::new(PathBuf::from("/x/nonum.CR3"), 1, FileKind::Cr3);
    assert_eq!(m.file_number, None);
}

#[test]
fn cr3_r5m2_golden() {
    let Some(p) = fixture("4P4A9687.CR3") else {
        eprintln!("skip: CR3 fixture missing");
        return;
    };
    let (m, bytes_read) = formats::parse_header(&p).unwrap();
    assert_eq!(m.kind, FileKind::Cr3);
    assert_eq!(m.model.as_deref(), Some("Canon EOS R5m2"));
    assert_eq!(m.make.as_deref(), Some("Canon"));
    assert!(m.serial.is_some());
    assert!(m.lens.is_some());
    assert_eq!(m.ts.unwrap().unix_centis, 1748157000_18);
    assert_eq!(m.exposure.iso, Some(640));
    assert_eq!(m.exposure.time, Some((1, 50)));
    assert_eq!((m.width, m.height), (8192, 5464));
    // header-only parse must stay cheap
    assert!(bytes_read <= 512 * 1024, "read {} bytes", bytes_read);
    // thumbnail is a bare JPEG
    let th = m.thumb.expect("THMB");
    assert!(th.is_bare_jpeg && th.width == 160);
    // preview extracts to a valid JPEG stream
    let (jpeg, w, h) = formats::extract_jpeg(&m, Source::Embedded).unwrap().expect("PRVW");
    assert_eq!((w, h), (1620, 1080));
    assert_eq!(&jpeg[0..2], &[0xFF, 0xD8]);
    // full-size embedded JPEG present with native dims
    let fs = m.fullsize.expect("fullsize trak");
    assert_eq!((fs.width, fs.height), (8192, 5464));
    assert!(fs.range.len > 1_000_000);
    // the Full source hands out that trak sample as a bare JPEG stream
    let (full, w, h) = formats::extract_jpeg(&m, Source::Full).unwrap().expect("trak jpeg");
    assert_eq!(&full[0..2], &[0xFF, 0xD8]);
    assert_eq!((w, h), (8192, 5464));
    assert_eq!(full.len() as u64, fs.range.len);
    // Canon MakerNote (CMT3): the eye-AF frame, 199x199 px at (+1969, +349)
    let b = m.af_box.expect("AFInfo2");
    assert!((b.cx - 0.7404).abs() < 1e-3 && (b.cy - 0.4361).abs() < 1e-3, "{b:?}");
    assert!((b.w - 0.02429).abs() < 1e-4 && (b.h - 0.03642).abs() < 1e-4, "{b:?}");
    assert_eq!(b.mode, 22);
    assert!(m.af_box_eye().is_some());
}

#[test]
fn jpeg_r5m2_golden() {
    let Some(p) = fixture("4P4A1410.JPG") else {
        eprintln!("skip: JPG fixture missing");
        return;
    };
    let (m, _) = formats::parse_header(&p).unwrap();
    assert_eq!(m.kind, FileKind::Jpeg);
    assert_eq!(m.model.as_deref(), Some("Canon EOS R5m2"));
    assert_eq!(m.ts.unwrap().unix_centis, 1746285170_00);
    assert_eq!(m.exposure.iso, Some(1000));
    assert_eq!((m.width, m.height), (8192, 5464));
    // Canon JPGs carry an MPF preview appendix; must beat whole-file decode
    let pv = m.preview.unwrap();
    assert!(pv.range.len < 1_000_000, "MPF preview not found");
    let (jpeg, _, _) = formats::extract_jpeg(&m, Source::Embedded).unwrap().unwrap();
    assert_eq!(&jpeg[0..2], &[0xFF, 0xD8]);
    assert_eq!(m.fullsize.unwrap().range.len, m.size);
    // Full source = the file itself
    let (full, w, h) = formats::extract_jpeg(&m, Source::Full).unwrap().unwrap();
    assert_eq!(full.len() as u64, m.size);
    assert_eq!((w, h), (8192, 5464));
    // AF frame present but a 960x2134 zone, not an eye
    assert!(m.af_box.is_some());
    assert!(m.af_box_eye().is_none());
    // lossless crop == the same window of the full decode (4:2:2 MCU = 16x8)
    let (crop, x0, y0) = fd_core::decode::decode_luma_crop(&full, 1000, 700, 300, 200).unwrap();
    assert_eq!((x0, y0), (992, 696));
    let whole = fd_core::decode::decode_luma_scaled(&full, usize::MAX / 2).unwrap();
    for row in 0..crop.height {
        let a = &crop.pixels[row * crop.width..(row + 1) * crop.width];
        let b = &whole.pixels[(y0 + row) * whole.width + x0..(y0 + row) * whole.width + x0 + crop.width];
        assert_eq!(a, b, "row {row}");
    }
}

#[test]
fn cr2_5d3_golden() {
    let Some(p) = fixture("5d3.CR2") else {
        eprintln!("skip: CR2 fixture missing");
        return;
    };
    let (m, _) = formats::parse_header(&p).unwrap();
    assert_eq!(m.kind, FileKind::Cr2);
    assert_eq!(m.model.as_deref(), Some("Canon EOS 5D Mark III"));
    assert!(m.ts.is_some());
    let (jpeg, _, _) = formats::extract_jpeg(&m, Source::Embedded).unwrap().expect("IFD0 jpeg");
    assert_eq!(&jpeg[0..2], &[0xFF, 0xD8]);
    // CR2 has no full-size JPEG: Full falls back to the same IFD0 stream
    let (full, _, _) = formats::extract_jpeg(&m, Source::Full).unwrap().expect("fallback");
    assert_eq!(full, jpeg);
    // DSLR AF point grid is not a subject frame
    assert!(m.af_box.is_none());
}
