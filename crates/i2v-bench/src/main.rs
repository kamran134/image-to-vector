//! Benchmark harness (docs/SPEC.md §6 / milestone M5).
//!
//! For every image in `corpus/`, traces it with each candidate frontend,
//! renders the resulting SVG back to raster (`i2v_core::metrics::render_svg`,
//! via resvg) and scores it against the source: mean/p99 RGBA error and
//! SSIM. `vanilla` is always run as the baseline every other candidate is
//! compared to. A candidate PASSes a file only if its error/SSIM did not
//! regress past a small tolerance — see `Verdict` — matching the acceptance
//! rule in docs/SPEC.md §6: an improvement only counts if quality didn't
//! drop.

use std::path::Path;

use anyhow::{Context, Result};
use i2v_core::metrics::{rgba_error, ssim};
use vtracer::{ColorImage, Config};

struct Trace {
    paths: usize,
    colors: usize,
    bytes: usize,
    mean_err: f64,
    p99_err: f64,
    ssim: f64,
}

fn trace_and_score(candidate: &str, img: &ColorImage) -> Result<Trace> {
    let cfg = Config::default();
    let svg = match candidate {
        "vanilla" => cfg.build()?.to_svg(img)?,
        "defringe" => i2v_core::alpha_pipeline(&cfg, 128)?.to_svg(img)?,
        other => anyhow::bail!("unknown candidate {other}"),
    };

    let source =
        image::RgbaImage::from_raw(img.width as u32, img.height as u32, img.pixels.clone())
            .context("source pixels didn't fit width*height*4")?;
    let rendered = i2v_core::metrics::render_svg(&svg, img.width as u32, img.height as u32)
        .map_err(anyhow::Error::msg)?;
    let (mean_err, p99_err) = rgba_error(&source, &rendered);

    Ok(Trace {
        paths: svg.matches("<path").count(),
        colors: count_distinct_fills(&svg),
        bytes: svg.len(),
        mean_err,
        p99_err,
        ssim: ssim(&source, &rendered),
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

/// Acceptance rule (docs/SPEC.md §6): a candidate only counts as an
/// improvement over vanilla if it didn't make the image look worse.
/// `ERR_TOLERANCE` absorbs float noise, not real regressions.
const ERR_TOLERANCE: f64 = 0.5;
const SSIM_TOLERANCE: f64 = 0.002;

fn verdict(baseline: &Trace, candidate: &Trace) -> &'static str {
    let quality_ok = candidate.mean_err <= baseline.mean_err + ERR_TOLERANCE
        && candidate.ssim >= baseline.ssim - SSIM_TOLERANCE;
    if !quality_ok {
        return "REGRESSION";
    }
    let improved = candidate.colors < baseline.colors
        || candidate.paths < baseline.paths
        || candidate.mean_err < baseline.mean_err - ERR_TOLERANCE;
    if improved {
        "PASS (improved)"
    } else {
        "PASS (no change)"
    }
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
            "corpus is empty ({}). Run `cargo run -p i2v-bench --bin gen_corpus` first.",
            corpus.display()
        );
        return Ok(());
    }

    println!(
        "{:<24} {:<10} {:>6} {:>7} {:>8} {:>9} {:>7} {:>6}  verdict",
        "file", "variant", "paths", "colors", "bytes", "mean_err", "p99", "ssim"
    );

    let mut regressions = Vec::new();
    for path in &entries {
        let name = path.file_name().unwrap().to_string_lossy().into_owned();
        let decoded = match image::open(path) {
            Ok(d) => d.to_rgba8(),
            Err(e) => {
                eprintln!("{}: {e}", path.display());
                continue;
            }
        };
        let img = ColorImage {
            pixels: decoded.clone().into_raw(),
            width: decoded.width() as usize,
            height: decoded.height() as usize,
        };

        let vanilla = match trace_and_score("vanilla", &img) {
            Ok(t) => t,
            Err(e) => {
                eprintln!("{name} vanilla: {e}");
                continue;
            }
        };
        println!(
            "{:<24} {:<10} {:>6} {:>7} {:>8} {:>9.2} {:>7.1} {:>6.3}  baseline",
            name,
            "vanilla",
            vanilla.paths,
            vanilla.colors,
            vanilla.bytes,
            vanilla.mean_err,
            vanilla.p99_err,
            vanilla.ssim,
        );

        for candidate in ["defringe"] {
            match trace_and_score(candidate, &img) {
                Ok(t) => {
                    let v = verdict(&vanilla, &t);
                    println!(
                        "{:<24} {:<10} {:>6} {:>7} {:>8} {:>9.2} {:>7.1} {:>6.3}  {}",
                        "", candidate, t.paths, t.colors, t.bytes, t.mean_err, t.p99_err, t.ssim, v
                    );
                    if v == "REGRESSION" {
                        regressions.push(format!("{name} [{candidate}]"));
                    }
                }
                Err(e) => eprintln!("{name} {candidate}: {e}"),
            }
        }
    }

    if !regressions.is_empty() {
        eprintln!("\n{} regression(s):", regressions.len());
        for r in &regressions {
            eprintln!("  - {r}");
        }
        anyhow::bail!("quality regressed on {} file(s)", regressions.len());
    }

    Ok(())
}
