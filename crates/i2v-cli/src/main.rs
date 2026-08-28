use std::path::PathBuf;

use clap::Parser;
use vtracer::{ColorImage, Config};

/// PNG/JPG -> SVG, on top of VTracer 1.0. `--defringe` turns on i2v-core's
/// alpha-aware frontend (see crates/i2v-core) instead of VTracer's own
/// RGB-only clustering. `--supersample N` (implies --defringe) traces at N×
/// resolution with bilinear-interpolated alpha for a sub-pixel-accurate
/// contour instead of one locked to the source pixel grid — skip it on
/// pixel art, where a hard blocky edge is the point (see docs/SPEC.md §6).
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

    /// Forwarded to vtracer::Config::simplify.
    #[arg(long)]
    simplify: Option<f64>,

    /// Forwarded to vtracer::Config::max_colors.
    #[arg(long)]
    max_colors: Option<usize>,
}

fn main() -> anyhow::Result<()> {
    let args = Args::parse();

    let decoded = image::open(&args.input)?.to_rgba8();
    let (width, height) = (decoded.width() as usize, decoded.height() as usize);
    let img = ColorImage {
        pixels: decoded.into_raw(),
        width,
        height,
    };

    let cfg = Config {
        simplify: args.simplify,
        max_colors: args.max_colors,
        ..Config::default()
    };

    let svg = if let Some(factor) = args.supersample {
        let pipeline =
            i2v_core::supersample::supersampled_alpha_pipeline(&cfg, args.alpha_threshold, factor)?;
        pipeline.to_svg(&img)?
    } else if args.defringe {
        let pipeline = i2v_core::alpha_pipeline(&cfg, args.alpha_threshold)?;
        pipeline.to_svg(&img)?
    } else {
        cfg.build()?.to_svg(&img)?
    };

    std::fs::write(&args.output, svg)?;
    Ok(())
}
