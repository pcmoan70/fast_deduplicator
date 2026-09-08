//! Preview JPEG decoding to luma (grayscale) planes for scoring.

use zune_core::colorspace::ColorSpace;
use zune_core::options::DecoderOptions;
use zune_jpeg::JpegDecoder;

pub struct Luma {
    pub pixels: Vec<u8>,
    pub width: usize,
    pub height: usize,
}

/// Normalized stored -> display coordinates for EXIF `orientation`; the
/// point-wise counterpart of `orient`.
pub fn orient_norm(x: f32, y: f32, orientation: u16) -> (f32, f32) {
    match orientation {
        2 => (1.0 - x, y),
        3 => (1.0 - x, 1.0 - y),
        4 => (x, 1.0 - y),
        5 => (y, x),
        6 => (1.0 - y, x),
        7 => (1.0 - y, 1.0 - x),
        8 => (y, 1.0 - x),
        _ => (x, y),
    }
}

/// The orientation that undoes `o` (6 and 8 swap, the rest are their own
/// inverse), so `orient_norm(.., inverse_orientation(o))` maps display
/// coordinates back to the stored frame.
pub fn inverse_orientation(o: u16) -> u16 {
    match o {
        6 => 8,
        8 => 6,
        o => o,
    }
}

/// Decode only a window of a large JPEG at full resolution: a lossless crop
/// transform (the rest is never inverse-DCT'd) followed by a 1:1 decode of
/// the small result. `x, y` round down to the MCU grid, so the returned
/// origin is where the crop really starts. The cost floor is one entropy
/// decode of the whole scan (~0.1-0.2 s for a 45 MP frame).
#[cfg(feature = "turbo")]
pub fn decode_luma_crop(
    jpeg: &[u8],
    x: usize,
    y: usize,
    w: usize,
    h: usize,
) -> Result<(Luma, usize, usize), DecodeError> {
    let err = |e: turbojpeg::Error| DecodeError(e.to_string());
    let hdr = turbojpeg::Decompressor::new().map_err(err)?.read_header(jpeg).map_err(err)?;
    let (mw, mh) = (hdr.subsamp.mcu_width(), hdr.subsamp.mcu_height());
    let (x0, y0) = ((x / mw) * mw, (y / mh) * mh);
    if x0 >= hdr.width || y0 >= hdr.height {
        return Err(DecodeError("crop outside the image".into()));
    }
    let (cw, ch) = ((x + w).min(hdr.width) - x0, (y + h).min(hdr.height) - y0);
    let mut t = turbojpeg::Transform::default();
    t.crop = Some(turbojpeg::TransformCrop { x: x0, y: y0, width: Some(cw), height: Some(ch) });
    t.gray = true;
    t.copy_none = true;
    let buf = turbojpeg::Transformer::new().map_err(err)?.transform_to_owned(&t, jpeg).map_err(err)?;
    let luma = decode_luma_scaled(&buf, usize::MAX / 2)?;
    Ok((luma, x0, y0))
}

#[cfg(not(feature = "turbo"))]
pub fn decode_luma_crop(
    jpeg: &[u8],
    x: usize,
    y: usize,
    w: usize,
    h: usize,
) -> Result<(Luma, usize, usize), DecodeError> {
    let full = decode_luma(jpeg)?;
    let (x0, y0) = (x.min(full.width - 1), y.min(full.height - 1));
    let (cw, ch) = ((x + w).min(full.width) - x0, (y + h).min(full.height) - y0);
    let mut pixels = Vec::with_capacity(cw * ch);
    for row in y0..y0 + ch {
        pixels.extend_from_slice(&full.pixels[row * full.width + x0..row * full.width + x0 + cw]);
    }
    Ok((Luma { pixels, width: cw, height: ch }, x0, y0))
}

impl Luma {
    /// Rotate/flip into display orientation (EXIF `orientation`, 1 = as is).
    pub fn upright(self, orientation: u16) -> Luma {
        let (pixels, width, height) = orient::<1>(self.pixels, self.width, self.height, orientation);
        Luma { pixels, width, height }
    }
}

/// Re-lay-out an `N`-bytes-per-pixel buffer so it displays upright for EXIF
/// `orientation`. 1 (and anything outside 2..=8) returns the input as is;
/// 5..=8 are the 90-degree cases, so width and height swap.
fn orient<const N: usize>(
    px: Vec<u8>,
    w: usize,
    h: usize,
    orientation: u16,
) -> (Vec<u8>, usize, usize) {
    if !(2..=8).contains(&orientation) {
        return (px, w, h);
    }
    let (ow, oh) = if orientation >= 5 { (h, w) } else { (w, h) };
    let mut out = vec![0u8; px.len()];
    // The 90-degree cases read the source column-wise (one cache line per
    // pixel), so work in T x T tiles to keep those lines hot: a 45 MP RGBA
    // frame drops from ~1.9 s to a fraction of the decode time.
    const T: usize = 128;
    for ty in (0..oh).step_by(T) {
        for tx in (0..ow).step_by(T) {
            for y in ty..(ty + T).min(oh) {
                // Source pixel index of output (0, y) and its step per x, from
                // the EXIF definition of each case.
                let (base, step): (usize, isize) = match orientation {
                    2 => (y * w + w - 1, -1),
                    3 => ((h - 1 - y) * w + w - 1, -1),
                    4 => ((h - 1 - y) * w, 1),
                    5 => (y, w as isize),
                    6 => ((h - 1) * w + y, -(w as isize)),
                    7 => ((h - 1) * w + (w - 1 - y), -(w as isize)),
                    _ => (w - 1 - y, w as isize),
                };
                for x in tx..(tx + T).min(ow) {
                    let si = (base as isize + x as isize * step) as usize * N;
                    let oi = (y * ow + x) * N;
                    out[oi..oi + N].copy_from_slice(&px[si..si + N]);
                }
            }
        }
    }
    (out, ow, oh)
}

#[derive(Debug, thiserror::Error)]
#[error("jpeg decode failed: {0}")]
pub struct DecodeError(String);

pub fn decode_luma(jpeg: &[u8]) -> Result<Luma, DecodeError> {
    let opts = DecoderOptions::default().jpeg_set_out_colorspace(ColorSpace::Luma);
    let mut dec = JpegDecoder::new_with_options(jpeg, opts);
    let pixels = dec.decode().map_err(|e| DecodeError(e.to_string()))?;
    let (width, height) = dec
        .dimensions()
        .ok_or_else(|| DecodeError("no dimensions".into()))?;
    Ok(Luma {
        pixels,
        width: width as usize,
        height: height as usize,
    })
}

/// Decode to luma at reduced scale: the smallest libjpeg-turbo DCT scaling
/// (1/1..1/8) that keeps the long edge >= `min_long_edge`. An 8192px JPEG
/// with min 1620 decodes at 1/4 (2048px) for ~1/16 the work. Falls back to
/// full-resolution zune-jpeg without the `turbo` feature.
#[cfg(feature = "turbo")]
pub fn decode_luma_scaled(jpeg: &[u8], min_long_edge: usize) -> Result<Luma, DecodeError> {
    let mut d = turbojpeg::Decompressor::new().map_err(|e| DecodeError(e.to_string()))?;
    let hdr = d.read_header(jpeg).map_err(|e| DecodeError(e.to_string()))?;
    let long = hdr.width.max(hdr.height);
    let mut chosen = turbojpeg::ScalingFactor::ONE;
    for denom in [8usize, 4, 2] {
        if long / denom >= min_long_edge {
            chosen = turbojpeg::ScalingFactor::new(1, denom);
            break;
        }
    }
    d.set_scaling_factor(chosen)
        .map_err(|e| DecodeError(e.to_string()))?;
    let (width, height) = (
        chosen.scale(hdr.width),
        chosen.scale(hdr.height),
    );
    let mut image = turbojpeg::Image {
        pixels: vec![0u8; width * height],
        width,
        pitch: width,
        height,
        format: turbojpeg::PixelFormat::GRAY,
    };
    d.decompress(jpeg, image.as_deref_mut())
        .map_err(|e| DecodeError(e.to_string()))?;
    Ok(Luma {
        pixels: image.pixels,
        width,
        height,
    })
}

#[cfg(not(feature = "turbo"))]
pub fn decode_luma_scaled(jpeg: &[u8], _min_long_edge: usize) -> Result<Luma, DecodeError> {
    decode_luma(jpeg)
}

pub struct Rgba {
    pub pixels: Vec<u8>,
    pub width: usize,
    pub height: usize,
}

impl Rgba {
    /// Rotate/flip into display orientation (EXIF `orientation`, 1 = as is).
    pub fn upright(self, orientation: u16) -> Rgba {
        let (pixels, width, height) = orient::<4>(self.pixels, self.width, self.height, orientation);
        Rgba { pixels, width, height }
    }
}

/// Display-only focus peaking, like a camera's MF peaking: pixels whose
/// high-frequency contrast passes `eye::PEAK_THRESHOLD` are painted red.
/// The same test drives the coverage score, so what lights up is what counts.
pub fn peaking_overlay(img: &mut Rgba) {
    let luma: Vec<u8> = img
        .pixels
        .chunks_exact(4)
        .map(|p| ((p[0] as u32 * 54 + p[1] as u32 * 183 + p[2] as u32 * 19) >> 8) as u8)
        .collect();
    let all = crate::score::Rect { x: 0, y: 0, w: img.width, h: img.height };
    let mask = crate::eye::peaking_mask(&luma, img.width, img.height, all);
    for (i, &on) in mask.iter().enumerate() {
        if on {
            img.pixels[i * 4..i * 4 + 3].copy_from_slice(&[255, 48, 48]);
        }
    }
}

/// Display-only auto brighten for browsing dark frames: if the 99.5th
/// percentile of luma is below `TARGET`, scale all channels linearly so it
/// lands there (gain capped at 4x, clipped). Bright images are left alone.
/// Never applied to the lumas that are scored or tracked.
pub fn auto_brighten(img: &mut Rgba) {
    const TARGET: f32 = 235.0;
    let mut hist = [0u32; 256];
    for p in img.pixels.chunks_exact(4) {
        let y = (p[0] as u32 * 54 + p[1] as u32 * 183 + p[2] as u32 * 19) >> 8;
        hist[y.min(255) as usize] += 1;
    }
    let n = (img.pixels.len() / 4) as u32;
    let cutoff = n - n / 200;
    let mut acc = 0u32;
    let mut p995 = 255usize;
    for (v, &c) in hist.iter().enumerate() {
        acc += c;
        if acc >= cutoff {
            p995 = v;
            break;
        }
    }
    let gain = (TARGET / p995.max(1) as f32).clamp(1.0, 4.0);
    if gain <= 1.01 {
        return;
    }
    let lut: [u8; 256] = std::array::from_fn(|v| (v as f32 * gain).min(255.0) as u8);
    for p in img.pixels.chunks_exact_mut(4) {
        p[0] = lut[p[0] as usize];
        p[1] = lut[p[1] as usize];
        p[2] = lut[p[2] as usize];
    }
}

/// RGBA decode at the smallest DCT scale keeping the long edge >=
/// `min_long_edge` (see decode_luma_scaled). Used for display textures.
#[cfg(feature = "turbo")]
pub fn decode_rgba_scaled(jpeg: &[u8], min_long_edge: usize) -> Result<Rgba, DecodeError> {
    let mut d = turbojpeg::Decompressor::new().map_err(|e| DecodeError(e.to_string()))?;
    let hdr = d.read_header(jpeg).map_err(|e| DecodeError(e.to_string()))?;
    let long = hdr.width.max(hdr.height);
    let mut chosen = turbojpeg::ScalingFactor::ONE;
    for denom in [8usize, 4, 2] {
        if long / denom >= min_long_edge {
            chosen = turbojpeg::ScalingFactor::new(1, denom);
            break;
        }
    }
    d.set_scaling_factor(chosen)
        .map_err(|e| DecodeError(e.to_string()))?;
    let (width, height) = (chosen.scale(hdr.width), chosen.scale(hdr.height));
    let mut image = turbojpeg::Image {
        pixels: vec![0u8; width * height * 4],
        width,
        pitch: width * 4,
        height,
        format: turbojpeg::PixelFormat::RGBA,
    };
    d.decompress(jpeg, image.as_deref_mut())
        .map_err(|e| DecodeError(e.to_string()))?;
    Ok(Rgba {
        pixels: image.pixels,
        width,
        height,
    })
}

#[cfg(not(feature = "turbo"))]
pub fn decode_rgba_scaled(jpeg: &[u8], _min_long_edge: usize) -> Result<Rgba, DecodeError> {
    let opts = DecoderOptions::default().jpeg_set_out_colorspace(ColorSpace::RGBA);
    let mut dec = JpegDecoder::new_with_options(jpeg, opts);
    let pixels = dec.decode().map_err(|e| DecodeError(e.to_string()))?;
    let (width, height) = dec
        .dimensions()
        .ok_or_else(|| DecodeError("no dimensions".into()))?;
    Ok(Rgba {
        pixels,
        width: width as usize,
        height: height as usize,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn luma3x2() -> Luma {
        Luma { pixels: vec![1, 2, 3, 4, 5, 6], width: 3, height: 2 }
    }

    /// The eight EXIF cases on a 3x2 pattern; 5..=8 come out 2x3.
    #[test]
    fn orient_all_exif_cases() {
        let cases: [(u16, &[u8], usize, usize); 8] = [
            (1, &[1, 2, 3, 4, 5, 6], 3, 2),
            (2, &[3, 2, 1, 6, 5, 4], 3, 2),
            (3, &[6, 5, 4, 3, 2, 1], 3, 2),
            (4, &[4, 5, 6, 1, 2, 3], 3, 2),
            (5, &[1, 4, 2, 5, 3, 6], 2, 3),
            (6, &[4, 1, 5, 2, 6, 3], 2, 3),
            (7, &[6, 3, 5, 2, 4, 1], 2, 3),
            (8, &[3, 6, 2, 5, 1, 4], 2, 3),
        ];
        for (o, want, w, h) in cases {
            let l = luma3x2().upright(o);
            assert_eq!((l.pixels.as_slice(), l.width, l.height), (want, w, h), "orientation {o}");
        }
    }

    #[test]
    fn orient_out_of_range_is_identity() {
        for o in [0, 9, 42] {
            let l = luma3x2().upright(o);
            assert_eq!((l.pixels, l.width, l.height), (vec![1, 2, 3, 4, 5, 6], 3, 2));
        }
    }

    /// Larger than one tile in both directions, so tile edges are exercised:
    /// every case composed with its inverse (6 <-> 8, the rest themselves)
    /// must give the original back.
    #[test]
    fn orient_round_trips_across_tiles() {
        let (w, h) = (150, 70);
        let pixels: Vec<u8> = (0..w * h).map(|i| (i * 7 % 251) as u8).collect();
        for (o, inv) in [(2, 2), (3, 3), (4, 4), (5, 5), (6, 8), (7, 7), (8, 6)] {
            let once = Luma { pixels: pixels.clone(), width: w, height: h }.upright(o);
            let back = once.upright(inv);
            assert_eq!((back.width, back.height), (w, h), "orientation {o}");
            assert!(back.pixels == pixels, "orientation {o} did not round-trip");
        }
    }

    /// `orient_norm` must agree with `orient` pixel for pixel: mark one
    /// pixel, rotate the image, and find it where the mapping says.
    #[test]
    fn orient_norm_matches_orient() {
        let (w, h) = (150usize, 70usize);
        let (sx, sy) = (37usize, 12usize);
        for o in 1..=8u16 {
            let mut px = vec![0u8; w * h];
            px[sy * w + sx] = 255;
            let l = Luma { pixels: px, width: w, height: h }.upright(o);
            let i = l.pixels.iter().position(|&v| v == 255).unwrap();
            let found = ((i % l.width) as f32 + 0.5) / l.width as f32;
            let found_y = ((i / l.width) as f32 + 0.5) / l.height as f32;
            let (ex, ey) = orient_norm((sx as f32 + 0.5) / w as f32, (sy as f32 + 0.5) / h as f32, o);
            assert!((found - ex).abs() < 1e-3 && (found_y - ey).abs() < 1e-3, "orientation {o}");
            let (bx, by) = orient_norm(ex, ey, inverse_orientation(o));
            assert!((bx - (sx as f32 + 0.5) / w as f32).abs() < 1e-3 && (by - (sy as f32 + 0.5) / h as f32).abs() < 1e-3);
        }
    }

    /// A dark frame is lifted so its highlights land near 235; a frame that
    /// already reaches the highlights is left untouched.
    #[test]
    fn auto_brighten_lifts_dark_frames_only() {
        let ramp = |max: u8| Rgba {
            pixels: (0..256u32).flat_map(|i| { let v = (i * max as u32 / 255) as u8; [v, v, v, 255] }).collect(),
            width: 16,
            height: 16,
        };
        let mut dark = ramp(100);
        auto_brighten(&mut dark);
        let top = *dark.pixels.chunks_exact(4).map(|p| &p[0]).max().unwrap();
        assert!((225..=245).contains(&top), "dark frame lifted to {top}");
        let mut bright = ramp(250);
        let before = bright.pixels.clone();
        auto_brighten(&mut bright);
        assert_eq!(bright.pixels, before);
    }

    /// Multi-byte pixels move as a unit: a 2x1 RGBA row rotated CCW (8) is
    /// a 1x2 column with the right-hand pixel on top.
    #[test]
    fn orient_keeps_rgba_pixels_intact() {
        let r = Rgba { pixels: vec![1, 2, 3, 4, 5, 6, 7, 8], width: 2, height: 1 }.upright(8);
        assert_eq!((r.pixels, r.width, r.height), (vec![5, 6, 7, 8, 1, 2, 3, 4], 1, 2));
    }
}
