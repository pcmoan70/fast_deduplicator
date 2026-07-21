//! Golden tests against real camera files in fixtures/ (populated by
//! scripts/fetch_fixtures.sh). Tests skip when a fixture is absent so CI
//! without fixtures still passes the pure-logic tests.

use std::path::PathBuf;

use fd_core::formats;
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
    let (jpeg, w, h) = formats::extract_preview(&m).unwrap().expect("PRVW");
    assert_eq!((w, h), (1620, 1080));
    assert_eq!(&jpeg[0..2], &[0xFF, 0xD8]);
    // full-size embedded JPEG present with native dims
    let fs = m.fullsize.expect("fullsize trak");
    assert_eq!((fs.width, fs.height), (8192, 5464));
    assert!(fs.range.len > 1_000_000);
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
    let (jpeg, _, _) = formats::extract_preview(&m).unwrap().unwrap();
    assert_eq!(&jpeg[0..2], &[0xFF, 0xD8]);
    assert_eq!(m.fullsize.unwrap().range.len, m.size);
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
    let (jpeg, _, _) = formats::extract_preview(&m).unwrap().expect("IFD0 jpeg");
    assert_eq!(&jpeg[0..2], &[0xFF, 0xD8]);
}
