//! Module C: geometric regularization (`docs/SPEC.md` §3).
//!
//! `RegularizePass` implements `vtracer::simplify::CurvePass` and runs two
//! independent sub-passes over each fitted contour:
//!
//! 1. [`circle_fit`] — a closed ring whose points all lie within `tolerance`
//!    of one circle is replaced by a canonical 4-arc Bézier circle.
//! 2. [`axis_snap`] — any segment that's already a straight line (or close
//!    enough to one) and within `angle_tolerance` degrees of horizontal or
//!    vertical is snapped exactly onto that axis, with the snap propagated
//!    through shared corner points so the contour stays a closed loop
//!    (rings) or keeps its two endpoints exactly fixed (open chains — see
//!    below).
//!
//! **Scope correction found while implementing, not assumed up front:**
//! whole-shape mirror symmetry (docs/SPEC.md §3 point 3, in the original
//! plan) turns out not to be buildable as a `CurvePass` at all — the trait
//! hands you one contour at a time (`fn ring(&self, geom: FittedGeom) ->
//! FittedGeom`), with no way to see or compare sibling contours belonging to
//! the same shape. Symmetry needs a whole-document pass (an
//! `OptimizerPass` over the assembled `VectorDoc`, a different and larger
//! piece of work) or a redesign of what a `CurvePass` receives. Left out of
//! this module rather than faked; docs/SPEC.md records this as a plan
//! correction.
//!
//! Radius-consistency across a shape (§3 point 4) has the same problem for
//! the same reason and is left out for the same reason.

use vtracer::fitter::FittedGeom;
use vtracer::simplify::CurvePass;
use vtracer::PointF64;

/// Geometric regularization: circle snapping + axis-aligned line snapping.
/// Off unless both tolerances are set above zero — the `logo` preset (not
/// yet implemented, see docs/SPEC.md §5) is meant to turn this on.
pub struct RegularizePass {
    /// Max distance (px) a point may sit from a fitted circle/axis-snapped
    /// line for the snap to apply.
    pub tolerance: f64,
    /// Max angle (degrees) from horizontal/vertical for a straight segment
    /// to be snapped onto that axis.
    pub angle_tolerance: f64,
    /// Minimum chord length (px) for a segment to be considered for axis
    /// snapping. Load-bearing, not a nicety: any short arc of *any* smooth
    /// curve looks nearly straight over a couple of pixels — a font glyph's
    /// curves are made of dozens of such short cubics. Snapping those
    /// individually toward whichever axis they happen to lean on introduces
    /// visible facets in what should stay a smooth curve; measured on the
    /// benchmark corpus (docs/SPEC.md §6), not assumed — an earlier version
    /// without this gate regressed 8 of 14 corpus files.
    pub min_length: f64,
    /// Max circle-fit deviation as a *fraction of the fitted radius*, on top
    /// of `tolerance`'s absolute cap — whichever is stricter wins. `tolerance`
    /// alone isn't enough: measured on the benchmark corpus, an 8px pixel-art
    /// block's 4 corners fit a circle of r≈3.5 within 0.8px, comfortably
    /// inside a 1px absolute tolerance, while being 23% of the radius off —
    /// obviously not a circle. An absolute px budget doesn't shrink with the
    /// shape; this does. `0.05` (5%) is a reasonable default.
    pub circle_relative_tolerance: f64,
}

impl CurvePass for RegularizePass {
    fn ring(&self, geom: FittedGeom) -> FittedGeom {
        if let Some(circle) = circle_fit(&geom, self.tolerance, self.circle_relative_tolerance) {
            return circle;
        }
        axis_snap(
            geom,
            self.angle_tolerance,
            self.tolerance,
            self.min_length,
            true,
        )
    }

    fn open(&self, geom: FittedGeom) -> FittedGeom {
        // Circle-fitting an open chain would need the replacement arc to
        // land exactly on the two pinned endpoints (mosaic junction nodes) —
        // a real constraint, not implemented here; only axis snapping runs,
        // which already respects pinning (see `axis_snap`'s `closed: false`
        // path).
        axis_snap(
            geom,
            self.angle_tolerance,
            self.tolerance,
            self.min_length,
            false,
        )
    }
}

fn sample_points(geom: &FittedGeom, samples_per_curve: usize) -> Vec<PointF64> {
    match geom {
        FittedGeom::Polyline(pts) => pts.clone(),
        FittedGeom::Beziers(chain) => {
            let mut out = Vec::with_capacity(chain.len() * samples_per_curve + 1);
            for c in chain {
                for i in 0..samples_per_curve {
                    let t = i as f64 / samples_per_curve as f64;
                    out.push(cubic_at(c, t));
                }
            }
            if let Some(last) = chain.last() {
                out.push(last[3]);
            }
            out
        }
    }
}

fn cubic_at(c: &[PointF64; 4], t: f64) -> PointF64 {
    let mt = 1.0 - t;
    let w0 = mt * mt * mt;
    let w1 = 3.0 * mt * mt * t;
    let w2 = 3.0 * mt * t * t;
    let w3 = t * t * t;
    PointF64 {
        x: w0 * c[0].x + w1 * c[1].x + w2 * c[2].x + w3 * c[3].x,
        y: w0 * c[0].y + w1 * c[1].y + w2 * c[2].y + w3 * c[3].y,
    }
}

/// Kåsa algebraic circle fit through `pts`, or `None` if fewer than 3
/// points. Minimizes algebraic (not geometric) residual — adequate here
/// since the result is only accepted after an independent geometric max-
/// deviation check in [`circle_fit`].
fn fit_circle(pts: &[PointF64]) -> Option<(PointF64, f64)> {
    let n = pts.len() as f64;
    if pts.len() < 3 {
        return None;
    }
    let (mut sx, mut sy, mut sxx, mut syy, mut sxy, mut sxz, mut syz, mut sz) =
        (0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0);
    for p in pts {
        let z = p.x * p.x + p.y * p.y;
        sx += p.x;
        sy += p.y;
        sxx += p.x * p.x;
        syy += p.y * p.y;
        sxy += p.x * p.y;
        sxz += p.x * z;
        syz += p.y * z;
        sz += z;
    }
    // Normal equations for x^2+y^2 + A x + B y + C = 0, least squares.
    let (a11, a12, a13) = (sxx, sxy, sx);
    let (a21, a22, a23) = (sxy, syy, sy);
    let (a31, a32, a33) = (sx, sy, n);
    let (b1, b2, b3) = (-sxz, -syz, -sz);

    let det = a11 * (a22 * a33 - a23 * a32) - a12 * (a21 * a33 - a23 * a31)
        + a13 * (a21 * a32 - a22 * a31);
    if det.abs() < 1e-9 {
        return None;
    }
    let det_a =
        b1 * (a22 * a33 - a23 * a32) - a12 * (b2 * a33 - a23 * b3) + a13 * (b2 * a32 - a22 * b3);
    let det_b =
        a11 * (b2 * a33 - a23 * b3) - b1 * (a21 * a33 - a23 * a31) + a13 * (a21 * b3 - b2 * a31);
    let (a, b) = (det_a / det, det_b / det);

    let center = PointF64 {
        x: -a / 2.0,
        y: -b / 2.0,
    };
    // r^2 = cx^2 + cy^2 - C; C from the third normal equation
    // a·sx + b·sy + n·C = -sz  =>  C = (-sz - a*sx - b*sy)/n.
    let c = (-sz - a * sx - b * sy) / n;
    let r2 = center.x * center.x + center.y * center.y - c;
    if r2 <= 0.0 {
        return None;
    }
    Some((center, r2.sqrt()))
}

/// If every sampled point of `geom` (must be a closed ring) lies within
/// `min(tolerance, r * relative_tolerance)` of one fitted circle, return a
/// canonical 4-arc replacement. `None` if it doesn't fit, or the geometry is
/// too small to check (fewer than 8 sampled points — not enough signal
/// either way).
fn circle_fit(geom: &FittedGeom, tolerance: f64, relative_tolerance: f64) -> Option<FittedGeom> {
    if tolerance <= 0.0 {
        return None;
    }
    let pts = sample_points(geom, 6);
    if pts.len() < 8 {
        return None;
    }
    let (center, r) = fit_circle(&pts)?;
    if r <= 0.0 {
        return None;
    }
    let max_dev = pts
        .iter()
        .map(|p| (((p.x - center.x).powi(2) + (p.y - center.y).powi(2)).sqrt() - r).abs())
        .fold(0.0_f64, f64::max);
    let budget = tolerance.min(r * relative_tolerance.max(0.0));
    if max_dev > budget {
        return None;
    }
    Some(circle_beziers(center, r))
}

/// The standard 4-cubic circle approximation (magic number `k` minimizes
/// radial error to ~0.03% — good enough given `tolerance` already bounds
/// acceptance in px).
fn circle_beziers(center: PointF64, r: f64) -> FittedGeom {
    const K: f64 = 0.5522847498307936;
    let pt = |x: f64, y: f64| PointF64 {
        x: center.x + x,
        y: center.y + y,
    };
    let chain = vec![
        [pt(r, 0.0), pt(r, r * K), pt(r * K, r), pt(0.0, r)],
        [pt(0.0, r), pt(-r * K, r), pt(-r, r * K), pt(-r, 0.0)],
        [pt(-r, 0.0), pt(-r, -r * K), pt(-r * K, -r), pt(0.0, -r)],
        [pt(0.0, -r), pt(r * K, -r), pt(r, -r * K), pt(r, 0.0)],
    ];
    FittedGeom::Beziers(chain)
}

/// True if `p1`/`p2` (a cubic's control points) sit within `tol` of the
/// straight line `p0`-`p3` — i.e. the cubic is a line in every way but
/// representation.
fn cubic_is_straight(c: &[PointF64; 4], tol: f64) -> bool {
    point_to_segment_dist(c[1], c[0], c[3]) <= tol && point_to_segment_dist(c[2], c[0], c[3]) <= tol
}

fn point_to_segment_dist(p: PointF64, a: PointF64, b: PointF64) -> f64 {
    let (dx, dy) = (b.x - a.x, b.y - a.y);
    let len_sq = dx * dx + dy * dy;
    if len_sq < 1e-12 {
        return ((p.x - a.x).powi(2) + (p.y - a.y).powi(2)).sqrt();
    }
    let t = (((p.x - a.x) * dx + (p.y - a.y) * dy) / len_sq).clamp(0.0, 1.0);
    let proj = PointF64 {
        x: a.x + t * dx,
        y: a.y + t * dy,
    };
    ((p.x - proj.x).powi(2) + (p.y - proj.y).powi(2)).sqrt()
}

/// Which axis (if either) the direction `a -> b` is within `angle_tol`
/// degrees of.
enum Axis {
    Horizontal,
    Vertical,
}

fn near_axis(a: PointF64, b: PointF64, angle_tol_deg: f64) -> Option<Axis> {
    let (dx, dy) = (b.x - a.x, b.y - a.y);
    let len = (dx * dx + dy * dy).sqrt();
    if len < 1e-9 {
        return None;
    }
    let angle_from_horizontal = (dy.abs() / len).asin().to_degrees();
    let angle_from_vertical = (dx.abs() / len).asin().to_degrees();
    if angle_from_horizontal <= angle_tol_deg {
        Some(Axis::Horizontal)
    } else if angle_from_vertical <= angle_tol_deg {
        Some(Axis::Vertical)
    } else {
        None
    }
}

/// One node of the corner-point representation used to snap shared corners
/// consistently across neighbouring segments — see module docs.
struct Chain {
    /// Corner positions; mutated in place by `axis_snap` as it snaps.
    points: Vec<PointF64>,
    /// `points` before any snapping — kept so `rebuild` can compute how far
    /// each corner moved, and shift an untouched segment's original control
    /// points by that delta instead of discarding them.
    orig_points: Vec<PointF64>,
    /// Original `[c1, c2]` per segment, for `Beziers` input — needed so a
    /// segment that never gets snapped keeps its real (possibly curved)
    /// shape in `rebuild` instead of being silently flattened to a line.
    /// Empty for `Polyline` input, which has no curvature to preserve.
    controls: Vec<[PointF64; 2]>,
    /// `straight[i]` connects `points[i]` to `points[i+1]` (open) or
    /// `points[(i+1) % n]` (closed): true when the segment is a snap
    /// *candidate* (straight enough, long enough) — not that it was
    /// actually snapped, which additionally needs a near-axis direction
    /// (tracked separately in `axis_snap` as it runs).
    straight: Vec<bool>,
    closed: bool,
}

fn chord_len(a: PointF64, b: PointF64) -> f64 {
    ((a.x - b.x).powi(2) + (a.y - b.y).powi(2)).sqrt()
}

fn to_chain(geom: &FittedGeom, straightness_tol: f64, min_length: f64) -> Chain {
    match geom {
        FittedGeom::Polyline(pts) => {
            let closed = pts.len() > 1 && pts.first() == pts.last();
            let points = if closed {
                pts[..pts.len() - 1].to_vec()
            } else {
                pts.clone()
            };
            let n = points.len();
            let n_segments = if closed { n } else { n.saturating_sub(1) };
            let straight = (0..n_segments)
                .map(|i| chord_len(points[i], points[(i + 1) % n.max(1)]) >= min_length)
                .collect();
            Chain {
                orig_points: points.clone(),
                points,
                controls: Vec::new(),
                straight,
                closed,
            }
        }
        FittedGeom::Beziers(chain) => {
            let closed = chain.len() > 1 && chain.last().unwrap()[3] == chain[0][0];
            let mut points: Vec<PointF64> = chain.iter().map(|c| c[0]).collect();
            if !closed {
                if let Some(last) = chain.last() {
                    points.push(last[3]);
                }
            }
            let straight = chain
                .iter()
                .map(|c| {
                    cubic_is_straight(c, straightness_tol) && chord_len(c[0], c[3]) >= min_length
                })
                .collect();
            let controls = chain.iter().map(|c| [c[1], c[2]]).collect();
            Chain {
                orig_points: points.clone(),
                points,
                controls,
                straight,
                closed,
            }
        }
    }
}

/// Snap straight, near-axis segments onto that axis, propagating each snap
/// to the shared corner so the contour stays connected. `closed_input` false
/// (an open chain) keeps `points[0]` and `points[last]` exactly as given —
/// mosaic junction nodes must not move; true (a ring) allows every point to
/// move, since staying closed is the only invariant a ring has to keep.
fn axis_snap(
    geom: FittedGeom,
    angle_tol_deg: f64,
    dist_tol: f64,
    min_length: f64,
    closed_input: bool,
) -> FittedGeom {
    if angle_tol_deg <= 0.0 || dist_tol <= 0.0 {
        return geom;
    }
    let was_beziers = matches!(geom, FittedGeom::Beziers(_));
    let mut chain = to_chain(&geom, dist_tol, min_length);
    if chain.points.len() < 2 {
        return geom;
    }
    let n = chain.points.len();
    let n_segments = chain.straight.len();
    // Open chains pin both endpoints (mosaic junction nodes); closed rings
    // pin nothing — any point may move, since "closed" is the only
    // invariant to preserve.
    let pin_first = !closed_input;
    let pin_last = !closed_input;
    let mut snapped = vec![false; n_segments];

    // `i` indexes `chain.straight`/`snapped` while `j = (i+1) % n` indexes
    // the shared corner in `chain.points` — an `enumerate()` rewrite would
    // only cover one of those.
    #[allow(clippy::needless_range_loop)]
    for i in 0..n_segments {
        if !chain.straight[i] {
            continue;
        }
        let j = (i + 1) % n;
        let a_pinned = pin_first && i == 0;
        let b_pinned = pin_last && j == n - 1;
        let a = chain.points[i];
        let b = chain.points[j];
        match near_axis(a, b, angle_tol_deg) {
            Some(Axis::Horizontal) => {
                let y = if a_pinned {
                    a.y
                } else if b_pinned {
                    b.y
                } else {
                    (a.y + b.y) / 2.0
                };
                if (a.y - y).abs() <= dist_tol && (b.y - y).abs() <= dist_tol {
                    if !a_pinned {
                        chain.points[i].y = y;
                    }
                    if !b_pinned {
                        chain.points[j].y = y;
                    }
                    snapped[i] = true;
                }
            }
            Some(Axis::Vertical) => {
                let x = if a_pinned {
                    a.x
                } else if b_pinned {
                    b.x
                } else {
                    (a.x + b.x) / 2.0
                };
                if (a.x - x).abs() <= dist_tol && (b.x - x).abs() <= dist_tol {
                    if !a_pinned {
                        chain.points[i].x = x;
                    }
                    if !b_pinned {
                        chain.points[j].x = x;
                    }
                    snapped[i] = true;
                }
            }
            None => {}
        }
    }

    rebuild(&chain, &snapped, was_beziers)
}

fn rebuild(chain: &Chain, snapped: &[bool], as_beziers: bool) -> FittedGeom {
    let n = chain.points.len();
    if !as_beziers {
        let mut pts = chain.points.clone();
        if chain.closed {
            pts.push(chain.points[0]);
        }
        return FittedGeom::Polyline(pts);
    }
    let n_segments = chain.straight.len();
    let mut out = Vec::with_capacity(n_segments);
    #[allow(clippy::needless_range_loop)]
    for i in 0..n_segments {
        let j = (i + 1) % n;
        let (a, b) = (chain.points[i], chain.points[j]);
        if snapped[i] {
            let c1 = PointF64 {
                x: a.x + (b.x - a.x) / 3.0,
                y: a.y + (b.y - a.y) / 3.0,
            };
            let c2 = PointF64 {
                x: a.x + (b.x - a.x) * 2.0 / 3.0,
                y: a.y + (b.y - a.y) * 2.0 / 3.0,
            };
            out.push([a, c1, c2, b]);
        } else {
            // Untouched segment: keep its real (possibly curved) shape.
            // Only its endpoints may have moved, if a *neighbouring*
            // segment got snapped and shared this corner — shift each
            // control point by exactly how far its own endpoint moved, so
            // the curve stays continuous without being re-fit or flattened.
            let (orig_a, orig_b) = (chain.orig_points[i], chain.orig_points[j]);
            let [oc1, oc2] = chain.controls[i];
            let da = PointF64 {
                x: a.x - orig_a.x,
                y: a.y - orig_a.y,
            };
            let db = PointF64 {
                x: b.x - orig_b.x,
                y: b.y - orig_b.y,
            };
            let c1 = PointF64 {
                x: oc1.x + da.x,
                y: oc1.y + da.y,
            };
            let c2 = PointF64 {
                x: oc2.x + db.x,
                y: oc2.y + db.y,
            };
            out.push([a, c1, c2, b]);
        }
    }
    FittedGeom::Beziers(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn poly(pts: &[(f64, f64)]) -> FittedGeom {
        FittedGeom::Polyline(pts.iter().map(|&(x, y)| PointF64 { x, y }).collect())
    }

    #[test]
    fn circle_fit_replaces_a_close_ring_with_a_canonical_circle() {
        let mut pts = Vec::new();
        for i in 0..32 {
            let a = i as f64 / 32.0 * std::f64::consts::TAU;
            pts.push((10.0 + 5.0 * a.cos(), 10.0 + 5.0 * a.sin()));
        }
        pts.push(pts[0]);
        let geom = poly(&pts);

        let out = circle_fit(&geom, 0.5, 0.05).expect("should fit a circle");
        match out {
            FittedGeom::Beziers(chain) => assert_eq!(chain.len(), 4),
            _ => panic!("expected Beziers"),
        }
    }

    #[test]
    fn circle_fit_rejects_a_large_square() {
        let square = poly(&[
            (0.0, 0.0),
            (10.0, 0.0),
            (10.0, 10.0),
            (0.0, 10.0),
            (0.0, 0.0),
        ]);
        assert!(circle_fit(&square, 0.5, 0.05).is_none());
    }

    #[test]
    fn circle_fit_rejects_a_small_square_even_within_absolute_tolerance() {
        // Regression test: an 8px pixel-art block's corners fit a circle of
        // r~3.5 within 0.8px under a purely-absolute tolerance — comfortably
        // inside 1.0px, while being ~23% of the radius off. Measured on the
        // real benchmark corpus (docs/SPEC.md §6), not a hypothetical.
        // `circle_fit` doesn't resample a Polyline (only Beziers get the
        // interpolated samples a real VTracer spline fit would produce), so
        // the perimeter needs enough of its own points to clear the >=8
        // minimum and reflect real sampling density — a few points per edge,
        // not just the 4 corners.
        let mut edge_pts = Vec::new();
        for &(x0, y0, x1, y1) in &[
            (0.0, 0.0, 8.0, 0.0),
            (8.0, 0.0, 8.0, 8.0),
            (8.0, 8.0, 0.0, 8.0),
            (0.0, 8.0, 0.0, 0.0),
        ] {
            for i in 0..4 {
                let t = i as f64 / 4.0;
                edge_pts.push((x0 + (x1 - x0) * t, y0 + (y1 - y0) * t));
            }
        }
        edge_pts.push(edge_pts[0]);
        let square = poly(&edge_pts);

        assert!(
            circle_fit(&square, 1.0, 0.05).is_none(),
            "an 8px square must not pass as a circle even under a 1px absolute budget"
        );
    }

    #[test]
    fn axis_snap_straightens_a_nearly_axis_aligned_square() {
        // Slightly wobbly square — corners off-axis by well under a degree.
        let wobbly = poly(&[
            (0.0, 0.0),
            (10.0, 0.05),
            (10.05, 10.0),
            (-0.05, 10.0),
            (0.0, 0.0),
        ]);
        let out = axis_snap(wobbly, 2.0, 0.5, 0.0, true);
        match out {
            FittedGeom::Polyline(pts) => {
                // All four corners should share exact x or y with their neighbours.
                assert_eq!(
                    pts[0].y, pts[1].y,
                    "bottom edge should be exactly horizontal"
                );
                assert_eq!(pts[1].x, pts[2].x, "right edge should be exactly vertical");
                assert_eq!(pts[2].y, pts[3].y, "top edge should be exactly horizontal");
                assert_eq!(pts[3].x, pts[4].x, "left edge should be exactly vertical");
            }
            _ => panic!("expected Polyline"),
        }
    }

    #[test]
    fn axis_snap_pins_open_chain_endpoints() {
        let pts = poly(&[(0.0, 0.0), (5.0, 0.02), (10.0, 0.0)]);
        let out = axis_snap(pts, 2.0, 0.5, 0.0, false);
        match out {
            FittedGeom::Polyline(p) => {
                assert_eq!(p[0], PointF64 { x: 0.0, y: 0.0 });
                assert_eq!(p[2], PointF64 { x: 10.0, y: 0.0 });
            }
            _ => panic!("expected Polyline"),
        }
    }

    #[test]
    fn regularize_pass_pipeline_builds_and_traces() {
        use vtracer::Color;
        let mut img = vtracer::ColorImage {
            pixels: vec![0u8; 20 * 20 * 4],
            width: 20,
            height: 20,
        };
        for y in 0..20 {
            for x in 0..20 {
                img.set_pixel(x, y, &Color::new(200, 30, 30));
            }
        }
        let mut pipeline = vtracer::Config::default().build().unwrap();
        pipeline.curve_passes.push(Box::new(RegularizePass {
            tolerance: 1.0,
            angle_tolerance: 3.0,
            min_length: 4.0,
            circle_relative_tolerance: 0.05,
        }));
        let svg = pipeline.to_svg(&img).unwrap();
        assert!(svg.contains("<svg"));
    }
}
