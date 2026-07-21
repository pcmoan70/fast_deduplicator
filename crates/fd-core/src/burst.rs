//! Burst grouping: pair RAW+JPEG into logical images, then split the
//! timeline into bursts wherever the inter-frame gap exceeds a threshold.

use std::collections::HashMap;

use crate::meta::{FileKind, FileMeta};

/// One picture as the photographer thinks of it: a RAW and/or its paired
/// JPEG (same stem, same second). Indices point into the scanned meta slice.
#[derive(Debug, Clone)]
pub struct LogicalImage {
    pub raw: Option<usize>,
    pub jpeg: Option<usize>,
}

impl LogicalImage {
    /// Preferred index for metadata/preview work (RAW wins: better subsec
    /// fidelity and the cheap 1620px preview).
    pub fn primary(&self) -> usize {
        self.raw.or(self.jpeg).unwrap()
    }

    pub fn indices(&self) -> impl Iterator<Item = usize> + '_ {
        self.raw.into_iter().chain(self.jpeg)
    }
}

#[derive(Debug)]
pub struct Burst {
    pub images: Vec<LogicalImage>,
}

impl Burst {
    pub fn span_centis(&self, metas: &[FileMeta]) -> i64 {
        let ts: Vec<i64> = self
            .images
            .iter()
            .filter_map(|li| metas[li.primary()].ts.map(|t| t.unix_centis))
            .collect();
        match (ts.iter().min(), ts.iter().max()) {
            (Some(a), Some(b)) => b - a,
            _ => 0,
        }
    }
}

/// Pair RAW+JPEG by file stem (case-insensitive), keeping scan order stable.
pub fn pair(metas: &[FileMeta]) -> Vec<LogicalImage> {
    let mut by_stem: HashMap<String, LogicalImage> = HashMap::new();
    let mut order: Vec<String> = Vec::new();
    for (i, m) in metas.iter().enumerate() {
        let stem = m
            .path
            .file_stem()
            .map(|s| s.to_string_lossy().to_ascii_lowercase())
            .unwrap_or_else(|| i.to_string());
        let entry = by_stem.entry(stem.clone()).or_insert_with(|| {
            order.push(stem);
            LogicalImage {
                raw: None,
                jpeg: None,
            }
        });
        match m.kind {
            FileKind::Jpeg => {
                if entry.jpeg.is_none() {
                    entry.jpeg = Some(i)
                }
            }
            _ => {
                if entry.raw.is_none() {
                    entry.raw = Some(i)
                }
            }
        }
    }
    order.into_iter().map(|s| by_stem.remove(&s).unwrap()).collect()
}

/// Group logical images into bursts. Sorts by (camera serial, timestamp,
/// file number) and starts a new burst when the gap exceeds `max_gap_centis`
/// (default suggestion: 60 = 0.6 s) or the camera changes. Images without a
/// timestamp each become singletons at the end.
pub fn group(metas: &[FileMeta], images: Vec<LogicalImage>, max_gap_centis: i64) -> Vec<Burst> {
    let mut dated: Vec<LogicalImage> = Vec::new();
    let mut undated: Vec<LogicalImage> = Vec::new();
    for li in images {
        if metas[li.primary()].ts.is_some() {
            dated.push(li);
        } else {
            undated.push(li);
        }
    }
    dated.sort_by_key(|li| {
        let m = &metas[li.primary()];
        (
            m.serial.clone().unwrap_or_default(),
            m.ts.unwrap().unix_centis,
            m.file_number.unwrap_or(0),
        )
    });

    let mut bursts: Vec<Burst> = Vec::new();
    for li in dated {
        let m = &metas[li.primary()];
        let start_new = match bursts.last().and_then(|b| b.images.last()) {
            None => true,
            Some(prev_li) => {
                let p = &metas[prev_li.primary()];
                p.serial != m.serial
                    || m.ts.unwrap().unix_centis - p.ts.unwrap().unix_centis > max_gap_centis
            }
        };
        if start_new {
            bursts.push(Burst { images: Vec::new() });
        }
        bursts.last_mut().unwrap().images.push(li);
    }
    for li in undated {
        bursts.push(Burst { images: vec![li] });
    }
    bursts
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::meta::{FileKind, FileMeta, Timestamp};
    use std::path::PathBuf;

    fn meta(name: &str, kind: FileKind, centis: Option<i64>) -> FileMeta {
        let mut m = FileMeta::new(PathBuf::from(name), 1, kind);
        m.ts = centis.map(|c| Timestamp { unix_centis: c });
        m.serial = Some("S1".into());
        m
    }

    #[test]
    fn pairs_raw_and_jpeg() {
        let metas = vec![
            meta("/a/IMG_1.CR3", FileKind::Cr3, Some(100)),
            meta("/a/IMG_1.JPG", FileKind::Jpeg, Some(100)),
            meta("/a/IMG_2.CR3", FileKind::Cr3, Some(105)),
        ];
        let imgs = pair(&metas);
        assert_eq!(imgs.len(), 2);
        assert_eq!(imgs[0].raw, Some(0));
        assert_eq!(imgs[0].jpeg, Some(1));
        assert_eq!(imgs[1].jpeg, None);
    }

    #[test]
    fn splits_on_gap() {
        let metas = vec![
            meta("/a/1.CR3", FileKind::Cr3, Some(0)),
            meta("/a/2.CR3", FileKind::Cr3, Some(4)),
            meta("/a/3.CR3", FileKind::Cr3, Some(8)),
            meta("/a/4.CR3", FileKind::Cr3, Some(500)), // 4.9 s later
            meta("/a/5.CR3", FileKind::Cr3, None),      // undated
        ];
        let bursts = group(&metas, pair(&metas), 60);
        assert_eq!(bursts.len(), 3);
        assert_eq!(bursts[0].images.len(), 3);
        assert_eq!(bursts[1].images.len(), 1);
        assert_eq!(bursts[2].images.len(), 1);
    }

    #[test]
    fn unsorted_input_sorts_by_time() {
        let metas = vec![
            meta("/a/3.CR3", FileKind::Cr3, Some(8)),
            meta("/a/1.CR3", FileKind::Cr3, Some(0)),
            meta("/a/2.CR3", FileKind::Cr3, Some(4)),
        ];
        let bursts = group(&metas, pair(&metas), 60);
        assert_eq!(bursts.len(), 1);
        let names: Vec<_> = bursts[0]
            .images
            .iter()
            .map(|li| metas[li.primary()].path.to_str().unwrap())
            .collect();
        assert_eq!(names, ["/a/1.CR3", "/a/2.CR3", "/a/3.CR3"]);
    }
}
