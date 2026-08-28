//! Module B: gradient detection (`docs/SPEC.md` §4).
//!
//! `GradientFitter` implements `vtracer::colorfit::ColorFitter` — the one
//! extension point in the pipeline that can rewrite *and remove/merge*
//! layers, not just recolor them in place (`fn fit(&self, seg: &mut
//! Segmentation)` gets the whole `Vec<Layer>`). It replaces a run of layers
//! whose *effective painted color* moves smoothly along one axis with a
//! single layer painted `Paint::Linear` — the fork's own addition to
//! `Paint` (`vendor/vtracer`, see VENDORED.md), since `Solid` was all
//! upstream had.
//!
//! **Design correction found while implementing, not assumed up front:**
//! the first version of this module assumed `ColorClusterFrontend`
//! produces clean, non-overlapping bands stacked edge-to-edge for a smooth
//! gradient — checking each `Layer`'s own mask directly for adjacency.
//! Tracing an actual gradient and inspecting the real segmentation showed
//! that's wrong: hierarchical color clustering paints a coarse full-canvas
//! layer first, then progressively smaller *overlapping* refinement layers
//! on top (painter's algorithm), not a flat partition. Checking each mask's
//! own bounding box for "does it touch its neighbour" against that data
//! matched nothing.
//!
//! What this does instead: resample the *effective visible color* along one
//! axis, at the cross-axis midpoint, by walking layers top-to-bottom
//! (`Segmentation`'s own documented paint order) and taking the first one
//! that covers each sampled point — i.e. reconstruct what a viewer actually
//! sees, the same compositing rule the renderer itself uses, rather than
//! reasoning about the layers' own overlapping geometry. That profile is
//! what gets checked for smooth, collinear (in OKLab) motion.
//!
//! **Scope, stated plainly:** detects one axis-aligned linear run spanning
//! most of the canvas along x or y. Not radial or diagonal gradients (a
//! diagonal one would need sampling along an arbitrary angle, not just the
//! two canvas axes — a real extension, not implemented here), and not
//! multiple independent gradient regions in one complex image — only the
//! single best run.

use visioncortex::BinaryImage;
use vtracer::colorfit::ColorFitter;
use vtracer::ir::{Layer, Paint, RegionMask, Segmentation};
use vtracer::{Color, PointI32};

/// Finds the longest run of smoothly, collinearly changing effective color
/// along one canvas axis and collapses whatever layers fall inside it into
/// one `Paint::Linear` layer. A no-op if nothing qualifies.
pub struct GradientFitter {
    /// Minimum length of a qualifying run, as a fraction of the canvas size
    /// along that axis — a real gradient banner fills most of the image,
    /// not a thin stripe.
    pub min_coverage: f64,
    /// Max OKLab perpendicular distance (see [`oklab_point_to_segment_dist`])
    /// a sampled point's color may sit from the run's fitted line.
    /// Calibrated empirically, not guessed: a plain RGB-linear ramp between
    /// two *saturated, distant* hues (red->blue, blue->green) bows through
    /// OKLab by 0.12-0.17 even though it's exactly the kind of gradient
    /// this is meant to catch — about the same as a flat jump between two
    /// arbitrary unrelated colors (red->green: 0.17), so deviation
    /// magnitude alone can't tell those apart at that end of the range.
    /// Realistic, moderate-saturation gradients (this project's own
    /// synthetic corpus: 0.02-0.04) sit well clear of that. `0.05` catches
    /// the corpus's gradients with room to spare while still rejecting
    /// unrelated-color jumps.
    pub max_deviation: f64,
    /// At most this many gradient stops in the output (evenly resampled
    /// across the qualifying run) — keeps the SVG small regardless of how
    /// many original layers fed into it.
    pub max_stops: usize,
    /// Minimum distinct colors a run must contain. Load-bearing, not a
    /// nicety: any 2 colors are trivially "collinear" — a line through two
    /// points always passes through both of them, distance 0 — so without
    /// this, two flat, unrelated color blocks (no gradient at all) pass the
    /// deviation check for free. Caught by testing, not assumed: an
    /// earlier version merged a red block directly abutting a green block
    /// into a fake "gradient" this way. 3 is the minimum that actually
    /// constrains anything (the third point has to fall near the line the
    /// first two already define).
    pub min_distinct_colors: usize,
    /// Max OKLab distance allowed between the 3 cross-axis samples
    /// (25%/50%/75%) taken at each position along the run — see
    /// [`resolve_profile`]. Also caught by the benchmark, not assumed: a
    /// synthetic radial-gradient-plus-noise image (`photo-poster-02.png`)
    /// regressed (SSIM 0.801 -> 0.766) because a single cross-axis sample
    /// can look locally linear along one line through 2D content that isn't
    /// linear at all. Calibrated the same way as `max_deviation`: computing
    /// the actual cross-axis OKLab spread for that radial image (mean 0.09,
    /// max 0.19 — real 2D structure, not noise) against a true linear
    /// gradient's (exactly 0 by construction, plus a few thousandths from
    /// `color_precision` quantization). `0.01` sits comfortably above the
    /// quantization noise floor and well below the radial signal.
    pub cross_axis_tolerance: f64,
}

impl Default for GradientFitter {
    fn default() -> Self {
        Self {
            min_coverage: 0.6,
            max_deviation: 0.05,
            max_stops: 8,
            min_distinct_colors: 3,
            cross_axis_tolerance: 0.01,
        }
    }
}

impl ColorFitter for GradientFitter {
    fn fit(&self, seg: &mut Segmentation) {
        let Some(run) = find_best_run(seg, self) else {
            return;
        };
        merge_run(seg, run, self.max_stops);
    }
}

#[derive(Clone, Copy, PartialEq)]
enum Axis {
    Y,
    X,
}

/// Effective color at one point, by walking layers in reverse (topmost
/// first) paint order and taking the first that covers it — the same rule a
/// renderer uses. `None` if nothing covers it.
fn effective_color(seg: &Segmentation, x: i32, y: i32) -> Option<Color> {
    seg.layers.iter().rev().find_map(|layer| {
        let lx = x - layer.mask.offset.x;
        let ly = y - layer.mask.offset.y;
        if lx >= 0
            && ly >= 0
            && (lx as usize) < layer.mask.width()
            && (ly as usize) < layer.mask.height()
            && layer.mask.image.get_pixel(lx as usize, ly as usize)
        {
            Some(layer.paint.color())
        } else {
            None
        }
    })
}

/// Effective color at each position `0..len` along `axis`, one sample per
/// position — `None` where a *linear* gradient wouldn't explain what's
/// there.
///
/// Sampling only the cross-axis midpoint (an earlier version's whole
/// approach) isn't enough on its own: a *radial* gradient can still look
/// locally smooth along one single sampled line through it, even though the
/// 2D content it's actually part of isn't a linear gradient at all — caught
/// on the benchmark corpus (`docs/SPEC.md` §6), not hypothetical: a
/// radial-plus-noise test image scored a real quality regression (SSIM
/// 0.801 -> 0.766, mean error 3x worse) from exactly this. Each position
/// now samples 3 cross-axis lines (25%/50%/75%) and only counts as covered
/// if all three agree in OKLab within `cross_axis_tolerance` — a true
/// linear gradient is constant across the cross axis by definition, so this
/// costs nothing on real linear content while rejecting radial/2D variation
/// before it ever reaches the collinearity check.
fn resolve_profile(
    seg: &Segmentation,
    axis: Axis,
    cross_axis_tolerance: f64,
) -> Vec<Option<Color>> {
    let (len, cross_len) = match axis {
        Axis::Y => (seg.height as i32, seg.width as i32),
        Axis::X => (seg.width as i32, seg.height as i32),
    };
    let cross_positions: Vec<i32> = [0.25, 0.5, 0.75]
        .iter()
        .map(|f| ((cross_len as f64) * f) as i32)
        .collect();

    (0..len)
        .map(|pos| {
            let samples: Vec<Color> = cross_positions
                .iter()
                .filter_map(|&cross| {
                    let (x, y) = match axis {
                        Axis::Y => (cross, pos),
                        Axis::X => (pos, cross),
                    };
                    effective_color(seg, x, y)
                })
                .collect();
            if samples.len() != cross_positions.len() {
                return None; // not fully covered across the cross axis
            }
            let labs: Vec<[f64; 3]> = samples.iter().map(to_oklab).collect();
            let consistent = labs
                .windows(2)
                .all(|w| oklab_point_to_segment_dist(w[0], w[1], w[1]) <= cross_axis_tolerance);
            consistent.then_some(samples[0])
        })
        .collect()
}

struct Run {
    axis: Axis,
    /// `[start, end)` along the axis.
    start: i32,
    end: i32,
    profile: Vec<Color>,
}

fn find_best_run(seg: &Segmentation, cfg: &GradientFitter) -> Option<Run> {
    [Axis::Y, Axis::X]
        .into_iter()
        .filter_map(|axis| find_run_on_axis(seg, axis, cfg))
        .max_by_key(|r| r.end - r.start)
}

fn find_run_on_axis(seg: &Segmentation, axis: Axis, cfg: &GradientFitter) -> Option<Run> {
    let profile = resolve_profile(seg, axis, cfg.cross_axis_tolerance);
    let len = profile.len();
    if len == 0 {
        return None;
    }

    // Longest contiguous run of `Some(_)` samples that stays within
    // max_deviation of the line through its own first/last sample.
    let mut best: Option<(usize, usize)> = None;
    let mut i = 0;
    while i < len {
        if profile[i].is_none() {
            i += 1;
            continue;
        }
        let mut j = i + 1;
        while j < len && profile[j].is_some() {
            j += 1;
        }
        // [i, j) is a maximal run of covered samples; find the longest
        // collinear sub-run within it by growing from the start.
        let mut k = i;
        while k < j {
            let start_color = profile[i].unwrap();
            let end_color = profile[k].unwrap();
            let a = to_oklab(&start_color);
            let b = to_oklab(&end_color);
            let ok = (i..=k).all(|p| {
                let c = to_oklab(&profile[p].unwrap());
                oklab_point_to_segment_dist(c, a, b) <= cfg.max_deviation
            });
            if !ok {
                break;
            }
            k += 1;
        }
        let run_len = k - i;
        let distinct = distinct_color_count(&profile[i..k]);
        if distinct >= cfg.min_distinct_colors && best.is_none_or(|(s, e)| e - s < run_len) {
            best = Some((i, k));
        }
        i = j.max(k);
    }

    let (start, end) = best?;
    if ((end - start) as f64 / len as f64) < cfg.min_coverage {
        return None;
    }
    let run_profile: Vec<Color> = (start..end).map(|p| profile[p].unwrap()).collect();
    Some(Run {
        axis,
        start: start as i32,
        end: end as i32,
        profile: run_profile,
    })
}

fn distinct_color_count(colors: &[Option<Color>]) -> usize {
    let mut seen: std::collections::HashSet<(u8, u8, u8)> = std::collections::HashSet::new();
    for c in colors.iter().flatten() {
        seen.insert((c.r, c.g, c.b));
    }
    seen.len()
}

fn resample_stops(profile: &[Color], max_stops: usize) -> Vec<(f64, Color)> {
    let n = profile.len();
    if n == 0 {
        return Vec::new();
    }
    let stops = max_stops.clamp(2, n);
    (0..stops)
        .map(|i| {
            let t = i as f64 / (stops - 1) as f64;
            let idx = ((t * (n - 1) as f64).round() as usize).min(n - 1);
            (t, profile[idx])
        })
        .collect()
}

fn merge_run(seg: &mut Segmentation, run: Run, max_stops: usize) {
    let stops = resample_stops(&run.profile, max_stops);

    let (width, height) = (seg.width as i32, seg.height as i32);
    let (mask, x1, y1, x2, y2) = match run.axis {
        Axis::Y => {
            let h = (run.end - run.start).max(1) as usize;
            let mut img = BinaryImage::new_w_h(width.max(0) as usize, h);
            for y in 0..h {
                for x in 0..width.max(0) as usize {
                    img.set_pixel(x, y, true);
                }
            }
            let mask = RegionMask::new(img, PointI32 { x: 0, y: run.start });
            let mid_x = width as f64 / 2.0;
            (mask, mid_x, run.start as f64, mid_x, run.end as f64)
        }
        Axis::X => {
            let w = (run.end - run.start).max(1) as usize;
            let mut img = BinaryImage::new_w_h(w, height.max(0) as usize);
            for y in 0..height.max(0) as usize {
                for x in 0..w {
                    img.set_pixel(x, y, true);
                }
            }
            let mask = RegionMask::new(img, PointI32 { x: run.start, y: 0 });
            let mid_y = height as f64 / 2.0;
            (mask, run.start as f64, mid_y, run.end as f64, mid_y)
        }
    };

    let gradient_layer = Layer {
        paint: Paint::Linear {
            x1,
            y1,
            x2,
            y2,
            stops,
        },
        mask,
    };

    // Replace every layer whose bounding box falls within [start, end) along
    // the run's axis with the one gradient layer, inserted at the first
    // removed layer's position. Partial overlap (a layer straddling the
    // boundary) is left untouched rather than guessed at — documented
    // scope limit above.
    let insert_at = seg
        .layers
        .iter()
        .position(|l| layer_within(l, run.axis, run.start, run.end))
        .unwrap_or(seg.layers.len());

    let mut new_layers = Vec::with_capacity(seg.layers.len());
    let mut inserted = false;
    for layer in seg.layers.drain(..) {
        if layer_within(&layer, run.axis, run.start, run.end) {
            if !inserted {
                new_layers.push(gradient_layer.clone());
                inserted = true;
            }
            continue;
        }
        new_layers.push(layer);
    }
    if !inserted {
        new_layers.insert(insert_at.min(new_layers.len()), gradient_layer);
    }
    seg.layers = new_layers;
}

fn layer_within(layer: &Layer, axis: Axis, start: i32, end: i32) -> bool {
    let (lo, hi) = match axis {
        Axis::Y => (
            layer.mask.offset.y,
            layer.mask.offset.y + layer.mask.height() as i32,
        ),
        Axis::X => (
            layer.mask.offset.x,
            layer.mask.offset.x + layer.mask.width() as i32,
        ),
    };
    lo >= start && hi <= end
}

/// Perpendicular distance from `p` to the segment `a`-`b` in OKLab space.
fn oklab_point_to_segment_dist(p: [f64; 3], a: [f64; 3], b: [f64; 3]) -> f64 {
    let d = [b[0] - a[0], b[1] - a[1], b[2] - a[2]];
    let len_sq = d[0] * d[0] + d[1] * d[1] + d[2] * d[2];
    if len_sq < 1e-12 {
        let e = [p[0] - a[0], p[1] - a[1], p[2] - a[2]];
        return (e[0] * e[0] + e[1] * e[1] + e[2] * e[2]).sqrt();
    }
    let ap = [p[0] - a[0], p[1] - a[1], p[2] - a[2]];
    let t = ((ap[0] * d[0] + ap[1] * d[1] + ap[2] * d[2]) / len_sq).clamp(0.0, 1.0);
    let proj = [a[0] + t * d[0], a[1] + t * d[1], a[2] + t * d[2]];
    let e = [p[0] - proj[0], p[1] - proj[1], p[2] - proj[2]];
    (e[0] * e[0] + e[1] * e[1] + e[2] * e[2]).sqrt()
}

fn srgb_to_linear(c: u8) -> f64 {
    let c = c as f64 / 255.0;
    if c <= 0.04045 {
        c / 12.92
    } else {
        ((c + 0.055) / 1.055).powf(2.4)
    }
}

/// sRGB -> OKLab (Björn Ottosson, https://bottosson.github.io/posts/oklab/),
/// the same well-known formula vtracer's own (private, unreachable from
/// outside the crate) `colorfit::oklab` module implements — reproduced here
/// since a plugin can't import it. Verified against the reference's known
/// fixed points: pure white -> L=1, a=b=0; pure black -> L=a=b=0 (see tests).
fn to_oklab(c: &Color) -> [f64; 3] {
    let (r, g, b) = (
        srgb_to_linear(c.r),
        srgb_to_linear(c.g),
        srgb_to_linear(c.b),
    );

    let l = 0.4122214708 * r + 0.5363325363 * g + 0.0514459929 * b;
    let m = 0.2119034982 * r + 0.6806995451 * g + 0.1073969566 * b;
    let s = 0.0883024619 * r + 0.2817188376 * g + 0.6299787005 * b;

    let (l_, m_, s_) = (l.cbrt(), m.cbrt(), s.cbrt());

    [
        0.2104542553 * l_ + 0.7936177850 * m_ - 0.0040720468 * s_,
        1.9779984951 * l_ - 2.4285922050 * m_ + 0.4505937099 * s_,
        0.0259040371 * l_ + 0.7827717662 * m_ - 0.8086757660 * s_,
    ]
}

#[cfg(test)]
mod tests {
    use super::*;
    use vtracer::ColorImage;

    #[test]
    fn oklab_maps_known_fixed_points() {
        let white = to_oklab(&Color::new(255, 255, 255));
        assert!((white[0] - 1.0).abs() < 1e-3, "L={}", white[0]);
        assert!(white[1].abs() < 1e-3 && white[2].abs() < 1e-3, "{white:?}");

        let black = to_oklab(&Color::new(0, 0, 0));
        assert!(
            black[0].abs() < 1e-3 && black[1].abs() < 1e-3 && black[2].abs() < 1e-3,
            "{black:?}"
        );
    }

    fn full_width_band(w: usize, y0: i32, y1: i32) -> RegionMask {
        let mut img = BinaryImage::new_w_h(w, (y1 - y0) as usize);
        for y in 0..(y1 - y0) as usize {
            for x in 0..w {
                img.set_pixel(x, y, true);
            }
        }
        RegionMask::new(img, PointI32 { x: 0, y: y0 })
    }

    #[test]
    fn merges_a_clean_vertical_gradient_profile() {
        let (w, h) = (20usize, 40i32);
        let mut seg = Segmentation::new(w as u32, h as u32);
        // 4 non-overlapping bands, painted bottom-to-top: an RGB-linear
        // ramp between two moderate-saturation colors (not pure primaries —
        // see the comment on `max_deviation` for why that distinction
        // matters: a pure-primary ramp like red->blue bows through OKLab by
        // as much as an unrelated color jump does).
        let colors = [
            Color::new(250, 90, 90),
            Color::new(197, 73, 113),
            Color::new(143, 57, 137),
            Color::new(90, 40, 160),
        ];
        for (i, &c) in colors.iter().enumerate() {
            let y0 = i as i32 * 10;
            seg.layers.push(Layer {
                paint: Paint::Solid(c),
                mask: full_width_band(w, y0, y0 + 10),
            });
        }

        GradientFitter::default().fit(&mut seg);

        assert_eq!(
            seg.layers.len(),
            1,
            "all four bands should merge into one layer"
        );
        match &seg.layers[0].paint {
            Paint::Linear { stops, .. } => assert!(stops.len() >= 2, "{stops:?}"),
            other => panic!("expected Linear, got {other:?}"),
        }
    }

    #[test]
    fn leaves_non_collinear_colors_alone() {
        let (w, h) = (20usize, 30i32);
        let mut seg = Segmentation::new(w as u32, h as u32);
        let colors = [
            Color::new(255, 0, 0),
            Color::new(0, 255, 0),
            Color::new(0, 0, 255),
        ];
        for (i, &c) in colors.iter().enumerate() {
            let y0 = i as i32 * 10;
            seg.layers.push(Layer {
                paint: Paint::Solid(c),
                mask: full_width_band(w, y0, y0 + 10),
            });
        }

        GradientFitter::default().fit(&mut seg);

        assert_eq!(
            seg.layers.len(),
            3,
            "non-collinear colors must not be merged"
        );
    }

    #[test]
    fn pipeline_with_gradient_fitter_builds_and_traces() {
        let (w, h) = (40usize, 40usize);
        let mut img = ColorImage {
            pixels: vec![0u8; w * h * 4],
            width: w,
            height: h,
        };
        let (from, to) = ((250.0, 90.0, 90.0), (90.0, 40.0, 160.0));
        for y in 0..h {
            let t = y as f64 / (h - 1) as f64;
            let lerp = |a: f64, b: f64| (a + (b - a) * t) as u8;
            let c = Color::new(lerp(from.0, to.0), lerp(from.1, to.1), lerp(from.2, to.2));
            for x in 0..w {
                img.set_pixel(x, y, &c);
            }
        }
        let mut pipeline = vtracer::Config {
            color_precision: 8,
            ..vtracer::Config::default()
        }
        .build()
        .unwrap();
        pipeline
            .color_fitters
            .push(Box::new(GradientFitter::default()));
        let svg = pipeline.to_svg(&img).unwrap();
        assert!(svg.contains("linearGradient"), "{svg}");
    }
}
