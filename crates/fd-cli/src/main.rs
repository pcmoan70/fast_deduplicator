use std::path::PathBuf;
use std::time::Instant;

use anyhow::Result;
use clap::{Parser, Subcommand};
use fd_core::formats;
use fd_core::meta::FileMeta;
use rayon::prelude::*;

#[derive(Parser)]
#[command(name = "fd", about = "fast_deduplicator CLI")]
struct Cli {
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// Scan a directory: parse headers, print metadata table + timings.
    Scan {
        dir: PathBuf,
        /// Print one line per file (default: summary + first/last few).
        #[arg(long)]
        all: bool,
        /// Extract preview JPEGs into this directory.
        #[arg(long)]
        dump_previews: Option<PathBuf>,
        /// Extract full-size embedded JPEGs into this directory.
        #[arg(long)]
        dump_fullsize: Option<PathBuf>,
        /// Limit number of files (0 = no limit).
        #[arg(long, default_value_t = 0)]
        limit: usize,
    },
    /// Group bursts, score sharpness, auto-pick the best N per burst.
    Cull {
        dir: PathBuf,
        /// Picks per burst.
        #[arg(long, default_value_t = 2)]
        top: usize,
        /// Burst gap threshold in seconds.
        #[arg(long, default_value_t = 0.6)]
        gap: f64,
        /// Write XMP sidecars (rating 3) next to picked files.
        #[arg(long)]
        xmp: bool,
        /// Copy picked files (RAW + paired JPEG + sidecar) here.
        #[arg(long)]
        copy_to: Option<PathBuf>,
        /// Score cache location (default: <dir>/.fd-cache.db).
        #[arg(long)]
        cache: Option<PathBuf>,
        /// Only list bursts and picks; write/copy nothing.
        #[arg(long)]
        dry_run: bool,
    },
    /// Time each pipeline stage for one file (parse/extract/decode/score).
    Bench { file: PathBuf },
}

fn main() -> Result<()> {
    match Cli::parse().cmd {
        Cmd::Scan {
            dir,
            all,
            dump_previews,
            dump_fullsize,
            limit,
        } => scan(dir, all, dump_previews, dump_fullsize, limit),
        Cmd::Cull {
            dir,
            top,
            gap,
            xmp,
            copy_to,
            cache,
            dry_run,
        } => cull(dir, top, gap, xmp, copy_to, cache, dry_run),
        Cmd::Bench { file } => bench(file),
    }
}

fn bench(file: PathBuf) -> Result<()> {
    use fd_core::{decode, score};
    let n = 10;
    let t = Instant::now();
    let mut meta = None;
    for _ in 0..n {
        meta = Some(formats::parse_header(&file)?.0);
    }
    println!("parse_header:  {:?}", t.elapsed() / n);
    let meta = meta.unwrap();
    let t = Instant::now();
    let mut prev = None;
    for _ in 0..n {
        prev = formats::extract_preview(&meta)?;
    }
    println!("extract:       {:?}", t.elapsed() / n);
    let (jpeg, _, _) = prev.expect("no preview");
    println!("preview bytes: {}", jpeg.len());
    let t = Instant::now();
    let mut luma = None;
    for _ in 0..n {
        luma = Some(decode::decode_luma_scaled(&jpeg, 1600).map_err(|e| anyhow::anyhow!("{e}"))?);
    }
    println!("decode:        {:?}", t.elapsed() / n);
    let luma = luma.unwrap();
    println!("decoded dims:  {}x{}", luma.width, luma.height);
    let t = Instant::now();
    let mut s = score::Sharpness::default();
    for _ in 0..n {
        s = score::score_global(&luma);
    }
    println!("score:         {:?}  (score {:.2})", t.elapsed() / n, s.score);
    Ok(())
}

fn collect_paths(dir: &PathBuf) -> Vec<PathBuf> {
    let mut paths: Vec<PathBuf> = jwalk::WalkDir::new(dir)
        .skip_hidden(true)
        .into_iter()
        .filter_map(|e| e.ok())
        .filter(|e| e.file_type().is_file())
        .map(|e| e.path())
        .filter(|p| formats::kind_from_ext(p).is_some())
        .collect();
    paths.sort();
    paths
}

fn cull(
    dir: PathBuf,
    top: usize,
    gap: f64,
    xmp: bool,
    copy_to: Option<PathBuf>,
    cache_path: Option<PathBuf>,
    dry_run: bool,
) -> Result<()> {
    use fd_core::{burst, cache::Cache, decode, output, score};

    let t0 = Instant::now();
    let paths = collect_paths(&dir);
    let metas: Vec<FileMeta> = paths
        .par_iter()
        .filter_map(|p| formats::parse_header(p).ok().map(|(m, _)| m))
        .collect();
    let t_scan = t0.elapsed();

    let images = burst::pair(&metas);
    let bursts = burst::group(&metas, images, (gap * 100.0) as i64);

    // Score every logical image's primary preview (cache-backed).
    let mut cache = Cache::open(
        &cache_path.unwrap_or_else(|| dir.join(".fd-cache.db")),
    )
    .map_err(|e| anyhow::anyhow!("cache: {e}"))?;
    let keys: Vec<String> = metas.iter().map(fd_core::cache::file_key).collect();
    let t1 = Instant::now();
    let todo: Vec<usize> = bursts
        .iter()
        .flat_map(|b| b.images.iter().map(|li| li.primary()))
        .filter(|&i| cache.get_score(&keys[i]).is_none())
        .collect();
    let fresh: Vec<(usize, f32)> = todo
        .par_iter()
        .filter_map(|&i| {
            let (jpeg, _, _) = formats::extract_preview(&metas[i]).ok()??;
            let luma = decode::decode_luma_scaled(&jpeg, 1600).ok()?;
            Some((i, score::score_global(&luma).score))
        })
        .collect();
    cache
        .put_scores(fresh.iter().map(|(i, s)| (keys[*i].as_str(), *s)))
        .map_err(|e| anyhow::anyhow!("cache: {e}"))?;
    let scored = fresh.len();
    let score_of = {
        let map: std::collections::HashMap<usize, f32> = fresh.into_iter().collect();
        move |i: usize, cache: &Cache| -> f32 {
            map.get(&i)
                .copied()
                .or_else(|| cache.get_score(&keys[i]))
                .unwrap_or(0.0)
        }
    };
    let t_score = t1.elapsed();

    // Rank within bursts, emit picks.
    let mut picks: Vec<usize> = Vec::new();
    println!("{:<6} {:>6} {:>8}  picks (score)", "burst", "frames", "span");
    for (bi, b) in bursts.iter().enumerate() {
        let mut ranked: Vec<(usize, f32)> = b
            .images
            .iter()
            .map(|li| {
                let i = li.primary();
                (i, score_of(i, &cache))
            })
            .collect();
        ranked.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap());
        let chosen: Vec<_> = ranked.iter().take(top).collect();
        let desc = chosen
            .iter()
            .map(|(i, s)| {
                format!(
                    "{} ({:.1})",
                    metas[*i].path.file_name().unwrap().to_string_lossy(),
                    s
                )
            })
            .collect::<Vec<_>>()
            .join(", ");
        println!(
            "{:<6} {:>6} {:>7.1}s  {}",
            bi,
            b.images.len(),
            b.span_centis(&metas) as f64 / 100.0,
            desc
        );
        picks.extend(chosen.iter().map(|(i, _)| *i));
    }

    println!(
        "\nscan: {:?} ({} files)  score: {:?} ({} fresh, {} cached)  bursts: {}  picks: {}",
        t_scan,
        metas.len(),
        t_score,
        scored,
        picks.len().saturating_sub(scored.min(picks.len())),
        bursts.len(),
        picks.len()
    );

    if dry_run {
        return Ok(());
    }
    if xmp {
        for &i in &picks {
            output::write_sidecar(&metas[i].path, 3, None)?;
        }
        println!("wrote {} XMP sidecars", picks.len());
    }
    if let Some(dest) = copy_to {
        let mut copied = 0;
        for &i in &picks {
            output::copy_pick(&metas[i].path, &dest)?;
            copied += 1;
        }
        println!("copied {} picks -> {}", copied, dest.display());
    }
    Ok(())
}

fn scan(
    dir: PathBuf,
    all: bool,
    dump: Option<PathBuf>,
    dump_full: Option<PathBuf>,
    limit: usize,
) -> Result<()> {
    let t0 = Instant::now();
    let mut paths = collect_paths(&dir);
    if limit > 0 {
        paths.truncate(limit);
    }
    let t_walk = t0.elapsed();

    let t1 = Instant::now();
    let results: Vec<_> = paths
        .par_iter()
        .map(|p| formats::parse_header(p))
        .collect();
    let t_parse = t1.elapsed();

    let mut metas: Vec<FileMeta> = Vec::new();
    let mut bytes_read = 0u64;
    let mut errors = 0usize;
    for (p, r) in paths.iter().zip(results) {
        match r {
            Ok((m, b)) => {
                metas.push(m);
                bytes_read += b;
            }
            Err(e) => {
                errors += 1;
                eprintln!("ERR {}: {}", p.display(), e);
            }
        }
    }

    let shown: Box<dyn Iterator<Item = &FileMeta>> = if all || metas.len() <= 12 {
        Box::new(metas.iter())
    } else {
        Box::new(metas.iter().take(6).chain(metas.iter().rev().take(6).rev()))
    };
    println!(
        "{:<14} {:<18} {:<22} {:>9} {:>11} {:>7} {:>9} {:>9} {:>10}",
        "file", "model", "time", "iso", "shutter", "focal", "thumb", "preview", "fullsize"
    );
    for m in shown {
        let name = m.path.file_name().unwrap_or_default().to_string_lossy();
        let ts = m
            .ts
            .map(|t| {
                let s = t.unix_centis / 100;
                let c = t.unix_centis % 100;
                format!("{}.{:02}", s, c)
            })
            .unwrap_or_else(|| "-".into());
        let sh = m
            .exposure
            .time
            .map(|(n, d)| format!("{}/{}", n, d))
            .unwrap_or_else(|| "-".into());
        let pr = |p: &Option<fd_core::PreviewInfo>| {
            p.map(|i| format!("{}K", i.range.len / 1024))
                .unwrap_or_else(|| "-".into())
        };
        println!(
            "{:<14} {:<18} {:<22} {:>9} {:>11} {:>7} {:>9} {:>9} {:>10}",
            name,
            m.model.as_deref().unwrap_or("-"),
            ts,
            m.exposure.iso.map(|i| i.to_string()).unwrap_or("-".into()),
            sh,
            m.exposure
                .focal_mm
                .map(|f| format!("{}mm", f))
                .unwrap_or("-".into()),
            pr(&m.thumb),
            pr(&m.preview),
            pr(&m.fullsize),
        );
    }

    let n = metas.len();
    println!(
        "\nwalk: {:?}  parse: {:?}  ({} files, {} errors, {:.0} files/s, {:.1} MB header I/O)",
        t_walk,
        t_parse,
        n,
        errors,
        n as f64 / t_parse.as_secs_f64(),
        bytes_read as f64 / 1e6
    );

    if let Some(out) = dump {
        std::fs::create_dir_all(&out)?;
        let t2 = Instant::now();
        let extracted: Vec<_> = metas
            .par_iter()
            .map(|m| {
                let r = formats::extract_preview(m)?;
                if let Some((jpeg, _, _)) = &r {
                    let name = m.path.file_stem().unwrap().to_string_lossy();
                    std::fs::write(out.join(format!("{}_preview.jpg", name)), jpeg)?;
                }
                Ok::<_, anyhow::Error>(r.map(|(j, w, h)| (j.len(), w, h)))
            })
            .collect();
        let ok = extracted.iter().filter(|r| matches!(r, Ok(Some(_)))).count();
        let total: usize = extracted
            .iter()
            .filter_map(|r| match r {
                Ok(Some((l, _, _))) => Some(*l),
                _ => None,
            })
            .sum();
        println!(
            "previews: {}/{} extracted in {:?} ({:.1} MB) -> {}",
            ok,
            n,
            t2.elapsed(),
            total as f64 / 1e6,
            out.display()
        );
    }

    if let Some(out) = dump_full {
        std::fs::create_dir_all(&out)?;
        let t3 = Instant::now();
        let ok: usize = metas
            .par_iter()
            .map(|m| -> Result<usize> {
                let Some(info) = m.fullsize else { return Ok(0) };
                let mut r = fd_core::io::FileReader::open(&m.path)?;
                use fd_core::io::ReadRange;
                let buf = r.read_range(info.range.offset, info.range.len as usize)?;
                let name = m.path.file_stem().unwrap().to_string_lossy();
                std::fs::write(out.join(format!("{}_full.jpg", name)), &buf)?;
                Ok(1)
            })
            .filter_map(|r| r.ok())
            .sum();
        println!(
            "fullsize: {}/{} extracted in {:?} -> {}",
            ok,
            n,
            t3.elapsed(),
            out.display()
        );
    }
    Ok(())
}
