//! Canon MakerNote: the AF area the camera used (AFInfo2). On mirrorless
//! bodies with subject detection this is the eye/face/body frame, which
//! gives the focus point per frame without any tracking.

use super::tiff::Tiff;
use crate::meta::{AfBox, FileMeta};

pub const TAG_AF_INFO2: u16 = 0x0026;

/// Walk the Canon MakerNote IFD at `ifd` (JPEG/CR2: the 0x927C value offset
/// inside the EXIF TIFF; CR3: CMT3's first IFD) and record the AF box.
pub fn apply_makernote(t: &Tiff, ifd: u32, meta: &mut FileMeta) {
    let (entries, _) = t.ifd(ifd);
    for e in &entries {
        if e.tag == TAG_AF_INFO2 {
            if let Some(words) = t.u16s(e) {
                meta.af_box = af_box_from_words(&words);
            }
        }
    }
}

/// AFInfo2 words: `[size, mode, npts, valid, cimg_w, cimg_h, af_w, af_h,
/// widths[npts], heights[npts], xs[npts], ys[npts], in_focus bits, selected
/// bits, ...]`. Positions are i16 relative to the image centre with +Y up, in
/// the stored (unrotated) frame. One to three non-zero areas is a subject
/// frame; dozens is a DSLR point grid, which says nothing about the subject.
pub(crate) fn af_box_from_words(w: &[u16]) -> Option<AfBox> {
    if w.len() < 8 || w[0] as usize != 2 * w.len() {
        return None;
    }
    let (mode, n) = (w[1], w[2] as usize);
    let (af_w, af_h) = (w[6] as f32, w[7] as f32);
    let bits = n.div_ceil(16);
    if af_w <= 0.0 || af_h <= 0.0 || w.len() < 8 + 4 * n + bits {
        return None;
    }
    let widths = &w[8..8 + n];
    let heights = &w[8 + n..8 + 2 * n];
    let xs = &w[8 + 2 * n..8 + 3 * n];
    let ys = &w[8 + 3 * n..8 + 4 * n];
    let in_focus = &w[8 + 4 * n..8 + 4 * n + bits];
    let areas: Vec<usize> = (0..n).filter(|&i| widths[i] != 0 && heights[i] != 0).collect();
    if areas.is_empty() || areas.len() > 3 {
        return None;
    }
    let focused = |i: usize| (in_focus[i / 16] >> (i % 16)) & 1 == 1;
    let i = areas.iter().copied().find(|&i| focused(i)).unwrap_or(areas[0]);
    let (x, y) = (xs[i] as i16 as f32, ys[i] as i16 as f32);
    Some(AfBox {
        cx: 0.5 + x / af_w,
        cy: 0.5 - y / af_h,
        w: widths[i] as f32 / af_w,
        h: heights[i] as f32 / af_h,
        mode,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn words(areas: &[(u16, u16, i16, i16)], in_focus: u16) -> Vec<u16> {
        let n = areas.len();
        let mut w = vec![0u16, 22, n as u16, 1, 8192, 5464, 8192, 5464];
        w.extend(areas.iter().map(|a| a.0));
        w.extend(areas.iter().map(|a| a.1));
        w.extend(areas.iter().map(|a| a.2 as u16));
        w.extend(areas.iter().map(|a| a.3 as u16));
        w.push(in_focus);
        w.push(in_focus);
        w[0] = 2 * w.len() as u16;
        w
    }

    #[test]
    fn single_area_maps_to_centre_relative_y_up() {
        let b = af_box_from_words(&words(&[(0, 0, 0, 0), (246, 251, -144, 349)], 0b10)).unwrap();
        assert!((b.cx - (0.5 - 144.0 / 8192.0)).abs() < 1e-6);
        assert!((b.cy - (0.5 - 349.0 / 5464.0)).abs() < 1e-6);
        assert!((b.w - 246.0 / 8192.0).abs() < 1e-6 && (b.h - 251.0 / 5464.0).abs() < 1e-6);
        assert_eq!(b.mode, 22);
    }

    #[test]
    fn grids_and_bad_sizes_are_ignored() {
        let grid: Vec<(u16, u16, i16, i16)> = (0..5).map(|i| (100, 100, i * 300, 0)).collect();
        assert!(af_box_from_words(&words(&grid, 0)).is_none());
        let mut bad = words(&[(246, 251, 0, 0)], 1);
        bad[0] += 2;
        assert!(af_box_from_words(&bad).is_none());
        assert!(af_box_from_words(&words(&[(0, 0, 0, 0)], 0)).is_none());
    }
}
