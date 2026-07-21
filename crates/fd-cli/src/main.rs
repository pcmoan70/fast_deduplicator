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
    }
}

fn scan(
    dir: PathBuf,
    all: bool,
    dump: Option<PathBuf>,
    dump_full: Option<PathBuf>,
    limit: usize,
) -> Result<()> {
    let t0 = Instant::now();
    let mut paths: Vec<PathBuf> = jwalk::WalkDir::new(&dir)
        .skip_hidden(true)
        .into_iter()
        .filter_map(|e| e.ok())
        .filter(|e| e.file_type().is_file())
        .map(|e| e.path())
        .filter(|p| formats::kind_from_ext(p).is_some())
        .collect();
    paths.sort();
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
