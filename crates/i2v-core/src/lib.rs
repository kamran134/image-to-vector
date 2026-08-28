//! Extensions to VTracer 1.0 that read the alpha channel it ignores.
//!
//! Verified against `vtracer` 1.0.0-alpha.3 sources: every built-in
//! [`vtracer::frontend`] type reads only RGB. The only place alpha is
//! touched at all is the private `frontend::keying` module, and only as an
//! `a == 0` heuristic gated on the image being "substantially" transparent
//! (see [`should_key_image`'s 20%-of-sampled-rows threshold][keying], not
//! reachable from outside the crate). The antialiased edge band
//! (`0 < a < 255`) — where a translucent pixel's stored RGB is often
//! contaminated by whatever was under it at authoring time — is unhandled
//! unconditionally. That contamination is what gets clustered into the
//! colored "fringe" around a trimmed PNG.
//!
//! [keying]: https://github.com/visioncortex/vtracer/blob/1.0.0-alpha.3/crates/vtracer/src/frontend/keying.rs

pub mod gradient;
pub mod metrics;
pub mod profile;
pub mod regularize;
pub mod supersample;

use vtracer::frontend::Frontend;
use vtracer::ir::Segmentation;
use vtracer::{Color, ColorImage, Error, PointI32};

/// Replace the RGB of every partially-transparent pixel (`0 < a < 255`) with
/// color propagated inward from the nearest fully-opaque pixel, in place.
/// Fully-transparent pixels (`a == 0`) are left untouched — invisible, their
/// color can't leak into a cluster's mean.
///
/// This is the fix for the fringe itself: applied before segmentation, a
/// translucent edge pixel clusters with the shape's real color instead of
/// with whatever color noise it happened to store.
pub fn defringe(img: &mut ColorImage) {
    let (w, h) = (img.width, img.height);
    if w == 0 || h == 0 {
        return;
    }

    let mut fixed = vec![false; w * h];
    for y in 0..h {
        for x in 0..w {
            if img.get_pixel(x, y).a == 255 {
                fixed[y * w + x] = true;
            }
        }
    }

    // Each pass propagates color one pixel further from the opaque interior.
    // A fringe is a handful of pixels wide, never the whole canvas, so a
    // small bounded cap keeps this from becoming quadratic on huge inputs
    // that happen to have no opaque pixel at all (a wholly-translucent image
    // just stops propagating and exits early via the `!changed` break).
    let max_passes = w.max(h).min(64);
    for _ in 0..max_passes {
        let snapshot = fixed.clone();
        let mut changed = false;
        for y in 0..h {
            for x in 0..w {
                let i = y * w + x;
                if snapshot[i] {
                    continue;
                }
                if img.get_pixel(x, y).a == 0 {
                    continue;
                }
                let mut sum = [0u32; 3];
                let mut n = 0u32;
                for (nx, ny) in neighbors4(x, y, w, h) {
                    if snapshot[ny * w + nx] {
                        let c = img.get_pixel(nx, ny);
                        sum[0] += c.r as u32;
                        sum[1] += c.g as u32;
                        sum[2] += c.b as u32;
                        n += 1;
                    }
                }
                if n > 0 {
                    let a = img.get_pixel(x, y).a;
                    img.set_pixel(
                        x,
                        y,
                        &Color::new_rgba(
                            (sum[0] / n) as u8,
                            (sum[1] / n) as u8,
                            (sum[2] / n) as u8,
                            a,
                        ),
                    );
                    fixed[i] = true;
                    changed = true;
                }
            }
        }
        if !changed {
            break;
        }
    }
}

fn neighbors4(x: usize, y: usize, w: usize, h: usize) -> impl Iterator<Item = (usize, usize)> {
    let mut v = Vec::with_capacity(4);
    if x > 0 {
        v.push((x - 1, y));
    }
    if x + 1 < w {
        v.push((x + 1, y));
    }
    if y > 0 {
        v.push((x, y - 1));
    }
    if y + 1 < h {
        v.push((x, y + 1));
    }
    v.into_iter()
}

/// A [`Frontend`] that makes VTracer alpha-aware.
///
/// 1. runs [`defringe`] so the translucent edge band no longer carries
///    contaminated color;
/// 2. delegates region-forming on the defringed RGB to `inner` (typically
///    [`vtracer::frontend::ColorClusterFrontend`]);
/// 3. clips every resulting region to a binary coverage mask built from the
///    alpha channel at `alpha_threshold`, so background pixels — including
///    ones `inner` would otherwise have clustered as a real color, since it
///    never looks at alpha — cannot survive as a spurious region.
///
/// Contour precision is native-resolution: one input pixel is one mask
/// pixel. `vtracer::ir::RegionMask`'s `BinaryImage` is a fully public,
/// freely constructible bitmap (`BinaryImage::new_w_h` + `set_pixel`,
/// confirmed by reading `visioncortex` 0.9.3 sources), so building the
/// coverage mask at an N× supersampled resolution — with a matching
/// downscaling `CurvePass` to bring fitted geometry back to canvas
/// coordinates — is a confirmed-feasible follow-up for genuine sub-pixel
/// contours. Not implemented yet: today's clipping only removes the fringe,
/// it does not sharpen the edge beyond the pixel grid.
pub struct AlphaFrontend {
    pub inner: Box<dyn Frontend>,
    /// Alpha at/above this value is inside the shape. `0..=255`.
    pub alpha_threshold: u8,
}

impl Frontend for AlphaFrontend {
    fn segment(&self, img: &ColorImage) -> Result<Segmentation, Error> {
        let mut defringed = img.clone();
        defringe(&mut defringed);

        let mut seg = self.inner.segment(&defringed)?;
        clip_to_alpha(&mut seg, img, self.alpha_threshold);
        Ok(seg)
    }
}

/// Zero out every mask pixel whose source-image alpha falls below
/// `threshold`, then drop layers left with no foreground pixels at all.
pub(crate) fn clip_to_alpha(seg: &mut Segmentation, img: &ColorImage, threshold: u8) {
    seg.layers.retain_mut(|layer| {
        let mask = &mut layer.mask;
        let (mw, mh) = (mask.width(), mask.height());
        for my in 0..mh {
            for mx in 0..mw {
                if !mask.image.get_pixel(mx, my) {
                    continue;
                }
                let cx = mask.offset.x + mx as i32;
                let cy = mask.offset.y + my as i32;
                let inside = cx >= 0
                    && cy >= 0
                    && (cx as usize) < img.width
                    && (cy as usize) < img.height
                    && img.get_pixel(cx as usize, cy as usize).a >= threshold;
                if !inside {
                    mask.image.set_pixel(mx, my, false);
                }
            }
        }
        mask.area() > 0
    });
}

/// Convenience: an [`AlphaFrontend`] wrapping the stock color-cluster
/// frontend with VTracer's own defaults (mirrors
/// `vtracer::Config::default()`'s `ColorClusterFrontend` construction).
pub fn default_alpha_frontend(alpha_threshold: u8) -> AlphaFrontend {
    use vtracer::frontend::ColorClusterFrontend;
    AlphaFrontend {
        inner: Box::new(ColorClusterFrontend {
            color_precision_loss: 8 - 6, // vtracer::Config::default().color_precision == 6
            layer_difference: 16,
            good_min_area: 16, // vtracer::Config::default().filter_speckle(4).pow(2)
        }),
        alpha_threshold,
    }
}

/// Proves the extension point claimed in `docs/SPEC.md` §2/§10.3: a
/// [`vtracer::Pipeline`] can be built by hand — with a custom [`Frontend`] —
/// without forking the crate. `vtracer::Pipeline`'s doc comment says as much
/// ("construct it directly for full control"); this exercises it.
pub fn alpha_pipeline(
    cfg: &vtracer::Config,
    alpha_threshold: u8,
) -> Result<vtracer::Pipeline, Error> {
    let mut pipeline = cfg.build()?;
    pipeline.frontend = Box::new(AlphaFrontend {
        inner: pipeline.frontend,
        alpha_threshold,
    });
    Ok(pipeline)
}

#[allow(dead_code)]
fn point(x: i32, y: i32) -> PointI32 {
    PointI32 { x, y }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn solid(w: usize, h: usize, c: Color) -> ColorImage {
        let mut img = ColorImage {
            pixels: vec![0u8; w * h * 4],
            width: w,
            height: h,
        };
        for y in 0..h {
            for x in 0..w {
                img.set_pixel(x, y, &c);
            }
        }
        img
    }

    #[test]
    fn defringe_recovers_true_color_under_a_contaminated_edge() {
        // A 3x1 red shape; the middle pixel is 50% alpha but stores a wrong,
        // contaminated color (as if blended against a blue background at
        // authoring time). defringe must pull it back toward the true
        // neighboring red, not leave the blue-tinted stored value.
        let red = Color::new(255, 0, 0);
        let mut img = solid(3, 1, red);
        img.set_pixel(1, 0, &Color::new_rgba(0, 0, 255, 128));

        defringe(&mut img);

        let fixed = img.get_pixel(1, 0);
        assert_eq!((fixed.r, fixed.g, fixed.b), (255, 0, 0));
        assert_eq!(fixed.a, 128, "alpha itself must be untouched");
    }

    #[test]
    fn defringe_leaves_fully_transparent_pixels_alone() {
        let red = Color::new(255, 0, 0);
        let mut img = solid(3, 1, red);
        img.set_pixel(1, 0, &Color::new_rgba(0, 0, 255, 0));

        defringe(&mut img);

        let untouched = img.get_pixel(1, 0);
        assert_eq!(
            (untouched.r, untouched.g, untouched.b, untouched.a),
            (0, 0, 255, 0)
        );
    }

    #[test]
    fn alpha_frontend_drops_a_fully_transparent_region() {
        // Vanilla ColorClusterFrontend ignores alpha entirely, so a
        // fully-transparent corner still clusters as an ordinary color
        // region. AlphaFrontend must clip it away.
        let mut img = solid(4, 4, Color::new(255, 0, 0));
        for y in 0..2 {
            for x in 0..2 {
                img.set_pixel(x, y, &Color::new_rgba(0, 255, 0, 0));
            }
        }

        let frontend = default_alpha_frontend(128);
        let seg = frontend.segment(&img).unwrap();

        for layer in &seg.layers {
            let (mw, mh) = (layer.mask.width(), layer.mask.height());
            for my in 0..mh {
                for mx in 0..mw {
                    if !layer.mask.image.get_pixel(mx, my) {
                        continue;
                    }
                    let cx = layer.mask.offset.x + mx as i32;
                    let cy = layer.mask.offset.y + my as i32;
                    assert!(
                        !(0..2).contains(&cx) || !(0..2).contains(&cy),
                        "transparent corner pixel ({cx},{cy}) survived clipping"
                    );
                }
            }
        }
    }

    #[test]
    fn alpha_pipeline_builds_and_traces_without_forking_vtracer() {
        let img = solid(4, 4, Color::new(255, 0, 0));
        let pipeline = alpha_pipeline(&vtracer::Config::default(), 128).unwrap();
        let svg = pipeline.to_svg(&img).unwrap();
        assert!(svg.contains("<svg"));
    }
}
