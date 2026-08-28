//! Benchmark harness skeleton (docs/SPEC.md §7 / milestone M1).
//!
//! Runs vanilla VTracer over every image in `corpus/` and prints path/color/
//! size counts as a starting baseline table. Still missing, deliberately
//! left for the next pass rather than faked: rendering the output SVG back
//! to raster (resvg) and scoring it against the source by RGBA error/SSIM.
//! Without that, this only tells you the output got smaller or had fewer
//! paths — not whether it still looks right. Don't read anything from this
//! tool as a "better than VTracer" result until that lands.

use std::path::Path;

use anyhow::Result;
use vtracer::{ColorImage, Config};

struct Stats {
    name: String,
    paths: usize,
    colors: usize,
    bytes: usize,
}

fn trace_stats(path: &Path) -> Result<Stats> {
    let decoded = image::open(path)?.to_rgba8();
    let (width, height) = (decoded.width() as usize, decoded.height() as usize);
    let img = ColorImage {
        pixels: decoded.into_raw(),
        width,
        height,
    };

    let svg = Config::default().build()?.to_svg(&img)?;

    Ok(Stats {
        name: path.file_name().unwrap().to_string_lossy().into_owned(),
        paths: svg.matches("<path").count(),
        colors: count_distinct_fills(&svg),
        bytes: svg.len(),
    })
}

fn count_distinct_fills(svg: &str) -> usize {
    let mut fills: Vec<&str> = svg
        .match_indices("fill=\"#")
        .filter_map(|(i, _)| svg[i + 6..].split('"').next())
        .collect();
    fills.sort_unstable();
    fills.dedup();
    fills.len()
}

fn main() -> Result<()> {
    let corpus = Path::new(env!("CARGO_MANIFEST_DIR")).join("corpus");
    const IMAGE_EXTS: &[&str] = &["png", "jpg", "jpeg", "gif", "bmp", "webp"];
    let mut entries: Vec<_> = std::fs::read_dir(&corpus)?
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| p.is_file())
        .filter(|p| {
            p.extension()
                .and_then(|e| e.to_str())
                .is_some_and(|e| IMAGE_EXTS.contains(&e.to_lowercase().as_str()))
        })
        .collect();
    entries.sort();

    if entries.is_empty() {
        eprintln!(
            "corpus is empty ({}). Populate it per docs/SPEC.md \u{a7}7 before trusting this table.",
            corpus.display()
        );
        return Ok(());
    }

    println!(
        "{:<28} {:>8} {:>8} {:>10}",
        "file", "paths", "colors", "bytes"
    );
    for path in entries {
        match trace_stats(&path) {
            Ok(s) => println!(
                "{:<28} {:>8} {:>8} {:>10}",
                s.name, s.paths, s.colors, s.bytes
            ),
            Err(e) => eprintln!("{}: {e}", path.display()),
        }
    }

    Ok(())
}
