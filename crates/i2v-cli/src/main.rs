use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use clap::Parser;
use i2v_core::profile::{AlphaMode, Profile, RegularizeSettings, TraceSettings};
use vtracer::ColorImage;

const IMAGE_EXTS: &[&str] = &["png", "jpg", "jpeg", "gif", "bmp", "webp"];

/// PNG/JPG -> SVG, on top of VTracer 1.0.
///
/// `--defringe` turns on i2v-core's alpha-aware frontend instead of
/// VTracer's own RGB-only clustering. `--supersample N` (implies
/// --defringe) traces at N× resolution with bilinear-interpolated alpha for
/// a sub-pixel-accurate contour — skip it on pixel art, where a hard blocky
/// edge is the point. `--regularize` snaps near-circular contours to true
/// circles and near-axis straight runs onto the axis. See docs/SPEC.md §2/§3
/// for what each does and doesn't touch.
///
/// `--profile <file.json>` loads a full settings bundle instead of the
/// flags above (docs/SPEC.md §7) — for a reproducible run months later
/// without reconstructing a flag combination from shell history.
/// `--save-profile <file.json>` writes out whatever settings this run
/// actually used (flags or a loaded profile), so a good result becomes a
/// file you can hand to someone else or reuse.
///
/// When `input` is a directory, every image file in it is traced with the
/// same settings into `output` (created if missing), alongside a
/// `report.csv` (file, paths, colors, bytes).
#[derive(Parser)]
struct Args {
    input: PathBuf,
    output: PathBuf,

    /// Use i2v-core's AlphaFrontend: defringe translucent edges, then clip
    /// regions to the alpha channel instead of letting VTracer ignore it.
    #[arg(long)]
    defringe: bool,

    /// Trace at N× resolution with bilinear alpha for a sub-pixel contour.
    /// N=1 is equivalent to --defringe. Implies --defringe.
    #[arg(long, value_name = "N")]
    supersample: Option<u32>,

    /// Alpha at/above this value counts as inside the shape. Only used with
    /// --defringe or --supersample.
    #[arg(long, default_value_t = 128)]
    alpha_threshold: u8,

    /// Snap near-circular contours to true circles and near-axis straight
    /// runs onto horizontal/vertical (i2v_core::regularize::RegularizePass).
    #[arg(long)]
    regularize: bool,

    /// Forwarded to vtracer::Config::simplify.
    #[arg(long)]
    simplify: Option<f64>,

    /// Forwarded to vtracer::Config::max_colors.
    #[arg(long)]
    max_colors: Option<usize>,

    /// Load settings from a JSON profile instead of the flags above.
    #[arg(long, value_name = "FILE")]
    profile: Option<PathBuf>,

    /// Write the settings this run used to a JSON profile.
    #[arg(long, value_name = "FILE")]
    save_profile: Option<PathBuf>,
}

fn profile_from_args(args: &Args) -> Result<Profile> {
    if let Some(path) = &args.profile {
        let text = std::fs::read_to_string(path)
            .with_context(|| format!("reading profile {}", path.display()))?;
        return Profile::from_json(&text)
            .with_context(|| format!("parsing profile {}", path.display()));
    }

    let alpha = match (args.supersample, args.defringe) {
        (Some(factor), _) => AlphaMode::Supersample {
            alpha_threshold: args.alpha_threshold,
            factor,
        },
        (None, true) => AlphaMode::Alpha {
            alpha_threshold: args.alpha_threshold,
        },
        (None, false) => AlphaMode::Vanilla,
    };

    Ok(Profile {
        trace: TraceSettings {
            simplify: args.simplify,
            max_colors: args.max_colors,
            ..TraceSettings::default()
        },
        alpha,
        regularize: args.regularize.then(RegularizeSettings::default),
    })
}

fn trace_one(profile: &Profile, input: &Path) -> Result<String> {
    let decoded = image::open(input)
        .with_context(|| format!("opening {}", input.display()))?
        .to_rgba8();
    let (width, height) = (decoded.width() as usize, decoded.height() as usize);
    let img = ColorImage {
        pixels: decoded.into_raw(),
        width,
        height,
    };

    let svg = profile.build_pipeline()?.to_svg(&img)?;
    Ok(svg)
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

fn run_batch(profile: &Profile, input_dir: &Path, output_dir: &Path) -> Result<()> {
    std::fs::create_dir_all(output_dir)
        .with_context(|| format!("creating {}", output_dir.display()))?;

    let mut entries: Vec<PathBuf> = std::fs::read_dir(input_dir)
        .with_context(|| format!("reading {}", input_dir.display()))?
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
        anyhow::bail!("no image files found in {}", input_dir.display());
    }

    let mut report = String::from("file,paths,colors,bytes\n");
    let mut failures = Vec::new();

    for path in &entries {
        let name = path.file_name().unwrap().to_string_lossy().into_owned();
        match trace_one(profile, path) {
            Ok(svg) => {
                let stem = path.file_stem().unwrap().to_string_lossy();
                let out_path = output_dir.join(format!("{stem}.svg"));
                std::fs::write(&out_path, &svg)
                    .with_context(|| format!("writing {}", out_path.display()))?;
                let paths = svg.matches("<path").count();
                let colors = count_distinct_fills(&svg);
                report.push_str(&format!("{name},{paths},{colors},{}\n", svg.len()));
            }
            Err(e) => {
                eprintln!("{name}: {e}");
                failures.push(name);
            }
        }
    }

    let report_path = output_dir.join("report.csv");
    std::fs::write(&report_path, &report)
        .with_context(|| format!("writing {}", report_path.display()))?;
    println!(
        "{} file(s) traced, {} failed. Report: {}",
        entries.len() - failures.len(),
        failures.len(),
        report_path.display()
    );

    if !failures.is_empty() {
        anyhow::bail!("{} of {} file(s) failed", failures.len(), entries.len());
    }
    Ok(())
}

fn main() -> Result<()> {
    let args = Args::parse();
    let profile = profile_from_args(&args)?;

    if let Some(save_path) = &args.save_profile {
        std::fs::write(save_path, profile.to_json_pretty()?)
            .with_context(|| format!("writing {}", save_path.display()))?;
    }

    if args.input.is_dir() {
        run_batch(&profile, &args.input, &args.output)
    } else {
        let svg = trace_one(&profile, &args.input)?;
        std::fs::write(&args.output, svg)
            .with_context(|| format!("writing {}", args.output.display()))?;
        Ok(())
    }
}
