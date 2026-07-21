//! Preview JPEG decoding to luma (grayscale) planes for scoring.

use zune_core::colorspace::ColorSpace;
use zune_core::options::DecoderOptions;
use zune_jpeg::JpegDecoder;

pub struct Luma {
    pub pixels: Vec<u8>,
    pub width: usize,
    pub height: usize,
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
