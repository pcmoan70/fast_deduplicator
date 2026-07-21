//! Output actions: XMP sidecars and pick harvesting. Originals are never
//! modified; sidecars sit next to the image file.

use std::io;
use std::path::{Path, PathBuf};

/// Lightroom/Bridge-compatible minimal sidecar.
pub fn xmp_sidecar_content(rating: u8, label: Option<&str>) -> String {
    let label_attr = label
        .map(|l| format!("\n   xmp:Label=\"{}\"", l))
        .unwrap_or_default();
    format!(
        "<x:xmpmeta xmlns:x=\"adobe:ns:meta/\">\n <rdf:RDF xmlns:rdf=\"http://www.w3.org/1999/02/22-rdf-syntax-ns#\">\n  <rdf:Description rdf:about=\"\"\n   xmlns:xmp=\"http://ns.adobe.com/xap/1.0/\"\n   xmp:Rating=\"{}\"{}/>\n </rdf:RDF>\n</x:xmpmeta>\n",
        rating, label_attr
    )
}

pub fn sidecar_path(image: &Path) -> PathBuf {
    image.with_extension("xmp")
}

pub fn write_sidecar(image: &Path, rating: u8, label: Option<&str>) -> io::Result<PathBuf> {
    let p = sidecar_path(image);
    std::fs::write(&p, xmp_sidecar_content(rating, label))?;
    Ok(p)
}

/// Copy a picked file (and an existing sidecar) into `dest`, never
/// overwriting: collisions get a numeric suffix.
pub fn copy_pick(image: &Path, dest: &Path) -> io::Result<PathBuf> {
    std::fs::create_dir_all(dest)?;
    let name = image.file_name().unwrap_or_default();
    let mut target = dest.join(name);
    let mut n = 1;
    while target.exists() {
        let stem = image.file_stem().unwrap_or_default().to_string_lossy();
        let ext = image
            .extension()
            .map(|e| format!(".{}", e.to_string_lossy()))
            .unwrap_or_default();
        target = dest.join(format!("{}_{}{}", stem, n, ext));
        n += 1;
    }
    std::fs::copy(image, &target)?;
    let sc = sidecar_path(image);
    if sc.exists() {
        std::fs::copy(&sc, sidecar_path(&target))?;
    }
    Ok(target)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sidecar_is_valid_xmp() {
        let s = xmp_sidecar_content(5, Some("Red"));
        assert!(s.contains("xmp:Rating=\"5\""));
        assert!(s.contains("xmp:Label=\"Red\""));
        assert!(s.starts_with("<x:xmpmeta"));
    }
}
