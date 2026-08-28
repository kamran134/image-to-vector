//! Module A v2: sub-pixel contour precision via supersampling.
//!
//! [`AlphaFrontend`](crate::AlphaFrontend) (v1) clips at native resolution —
//! one input pixel is one mask pixel, so the traced contour is locked to the
//! source pixel grid, same as vanilla VTracer. This module builds the
//! coverage mask at `N`× the source resolution instead, with the alpha
//! channel read by bilinear interpolation rather than nearest-neighbor, so
//! the 50%-coverage crossing — the true sub-pixel edge position — lands
//! wherever it actually falls, not wherever the nearest whole pixel happens
//! to be.
//!
//! The mechanism: [`Frontend::segment`] runs entirely in supersampled pixel
//! space, and a paired [`DownscalePass`] (a [`CurvePass`]) divides every
//! fitted coordinate by `N` before it reaches the SVG writer. `RegionMask`'s
//! `BinaryImage` has no tie to any particular resolution — it's a plain
//! bitmap you size yourself (confirmed in `docs/SPEC.md` §0.2.2) — so
//! nothing here needs upstream changes.

use vtracer::frontend::Frontend;
use vtracer::ir::Segmentation;
use vtracer::simplify::CurvePass;
use vtracer::{ColorImage, Error};

use crate::{clip_to_alpha, defringe};

/// Scales every fitted coordinate by `1 / factor`, undoing the resolution
/// increase [`SupersampledAlphaFrontend`] introduced before curve fitting.
/// Must run first in the pipeline's `curve_passes` — anything after it
/// (`--simplify`'s tolerance, corner thresholds) is specified in final
/// output units, not supersampled ones.
pub struct DownscalePass {
    pub factor: f64,
}

impl CurvePass for DownscalePass {
    fn open(&self, geom: vtracer::fitter::FittedGeom) -> vtracer::fitter::FittedGeom {
        self.scale(geom)
    }
    fn ring(&self, geom: vtracer::fitter::FittedGeom) -> vtracer::fitter::FittedGeom {
        self.scale(geom)
    }
}

impl DownscalePass {
    fn scale(&self, geom: vtracer::fitter::FittedGeom) -> vtracer::fitter::FittedGeom {
        use vtracer::fitter::FittedGeom;
        use vtracer::PointF64;
        let p = |q: PointF64| PointF64 {
            x: q.x / self.factor,
            y: q.y / self.factor,
        };
        match geom {
            FittedGeom::Polyline(pts) => FittedGeom::Polyline(pts.into_iter().map(p).collect()),
            FittedGeom::Beziers(chain) => {
                FittedGeom::Beziers(chain.into_iter().map(|c| c.map(p)).collect())
            }
        }
    }
}

/// Bilinear-sample the alpha channel of `img` at continuous source
/// coordinates `(sx, sy)`. Outside the image bounds reads as fully
/// transparent — a shape's silhouette ends at the canvas edge, it doesn't
/// extend into it.
fn sample_alpha_bilinear(img: &ColorImage, sx: f64, sy: f64) -> f64 {
    let get = |x: i32, y: i32| -> f64 {
        if x < 0 || y < 0 || x as usize >= img.width || y as usize >= img.height {
            0.0
        } else {
            img.get_pixel(x as usize, y as usize).a as f64
        }
    };

    let x0 = sx.floor();
    let y0 = sy.floor();
    let (fx, fy) = (sx - x0, sy - y0);
    let (x0, y0) = (x0 as i32, y0 as i32);

    let a00 = get(x0, y0);
    let a10 = get(x0 + 1, y0);
    let a01 = get(x0, y0 + 1);
    let a11 = get(x0 + 1, y0 + 1);

    a00 * (1.0 - fx) * (1.0 - fy) + a10 * fx * (1.0 - fy) + a01 * (1.0 - fx) * fy + a11 * fx * fy
}

/// [`AlphaFrontend`](crate::AlphaFrontend) at `factor`× the source
/// resolution, for sub-pixel contour precision. `factor` of `1` degenerates
/// to native resolution (equivalent to v1, modulo the bilinear vs.
/// nearest-neighbor alpha read). Pair with [`DownscalePass`] — see
/// [`supersampled_alpha_pipeline`], which wires both up correctly.
pub struct SupersampledAlphaFrontend {
    pub inner: Box<dyn Frontend>,
    pub alpha_threshold: u8,
    pub factor: u32,
}

impl Frontend for SupersampledAlphaFrontend {
    fn segment(&self, img: &ColorImage) -> Result<Segmentation, Error> {
        let n = self.factor.max(1);
        if n == 1 {
            let mut defringed = img.clone();
            defringe(&mut defringed);
            let mut seg = self.inner.segment(&defringed)?;
            clip_to_alpha(&mut seg, img, self.alpha_threshold);
            return Ok(seg);
        }

        let mut defringed = img.clone();
        defringe(&mut defringed);

        let (sw, sh) = (img.width * n as usize, img.height * n as usize);
        let mut upsampled = ColorImage {
            pixels: vec![0u8; sw * sh * 4],
            width: sw,
            height: sh,
        };
        // RGB: nearest-neighbor from the defringed source. Bilinear here
        // would blend in fresh mixed colors right where defringe just
        // removed contamination, working against it.
        for y in 0..sh {
            let src_y = (y / n as usize).min(img.height - 1);
            for x in 0..sw {
                let src_x = (x / n as usize).min(img.width - 1);
                upsampled.set_pixel(x, y, &defringed.get_pixel(src_x, src_y));
            }
        }

        let mut seg = self.inner.segment(&upsampled)?;

        // Coverage mask: bilinear alpha from the *original* (non-defringed —
        // defringe never touches alpha) image, at supersampled resolution.
        let threshold = self.alpha_threshold as f64;
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
                    // Map a supersampled pixel's center back to source
                    // pixel-center coordinates.
                    let sx = (cx as f64 + 0.5) / n as f64 - 0.5;
                    let sy = (cy as f64 + 0.5) / n as f64 - 0.5;
                    let inside = sample_alpha_bilinear(img, sx, sy) >= threshold;
                    if !inside {
                        mask.image.set_pixel(mx, my, false);
                    }
                }
            }
            mask.area() > 0
        });

        // The pipeline's viewBox comes from these two fields (see
        // docs/SPEC.md §0.2.2) — declare the *original* canvas size. Fitted
        // geometry is still in supersampled coordinates at this point;
        // pairing this frontend with `DownscalePass` in `curve_passes`
        // brings it back in range before the writer ever sees it.
        seg.width = img.width as u32;
        seg.height = img.height as u32;

        Ok(seg)
    }
}

/// Builds a [`vtracer::Pipeline`] with [`SupersampledAlphaFrontend`] as its
/// frontend and [`DownscalePass`] prepended to `curve_passes` — the two
/// halves of Module A v2 have to be installed together, this is the
/// guaranteed-consistent way to do it.
///
/// For `Clustering::ColorCluster` (the default), the inner frontend is
/// rebuilt rather than reused from `cfg.build()`, with its area-based
/// speckle filter (`good_min_area`) scaled by `factor²`. That threshold is
/// an absolute pixel area; left as-is at `factor`× linear resolution, it
/// stops filtering almost anything (measured, not assumed: on a real 1100×
/// 537 logo this took 189 colors to 1924 before the scaling was added — a
/// speck the filter used to reject was 16px² in native terms, and only
/// 1px² at 4× before the fix). Other clustering modes fall back to the
/// frontend `cfg.build()` produced, unscaled — a known gap, not silently
/// "handled".
pub fn supersampled_alpha_pipeline(
    cfg: &vtracer::Config,
    alpha_threshold: u8,
    factor: u32,
) -> Result<vtracer::Pipeline, Error> {
    let mut pipeline = cfg.build()?;
    let n = factor.max(1);

    let inner: Box<dyn Frontend> = if n > 1 && cfg.clustering == vtracer::Clustering::ColorCluster {
        use vtracer::frontend::ColorClusterFrontend;
        Box::new(ColorClusterFrontend {
            color_precision_loss: 8 - cfg.color_precision,
            layer_difference: cfg.layer_difference,
            good_min_area: cfg.filter_speckle * cfg.filter_speckle * (n * n) as usize,
        })
    } else {
        pipeline.frontend
    };

    pipeline.frontend = Box::new(SupersampledAlphaFrontend {
        inner,
        alpha_threshold,
        factor: n,
    });
    pipeline
        .curve_passes
        .insert(0, Box::new(DownscalePass { factor: n as f64 }));
    Ok(pipeline)
}

#[cfg(test)]
mod tests {
    use super::*;
    use vtracer::Color;

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
    fn sample_alpha_bilinear_interpolates_between_pixels() {
        let mut img = solid(2, 1, Color::new_rgba(255, 255, 255, 0));
        img.set_pixel(1, 0, &Color::new_rgba(255, 255, 255, 200));
        // Halfway between a=0 and a=200 should read close to 100, not
        // snapped to either whole-pixel value.
        let mid = sample_alpha_bilinear(&img, 0.5, 0.0);
        assert!((mid - 100.0).abs() < 1.0, "got {mid}");
    }

    #[test]
    fn supersampled_pipeline_traces_at_original_canvas_size() {
        // A circle drawn directly at 4x supersampled resolution: if the
        // downscale pass is wired correctly, the traced SVG's viewBox and
        // coordinates come out at native size, not 4x it.
        let img = solid(20, 20, Color::new(200, 30, 30));
        let pipeline = supersampled_alpha_pipeline(&vtracer::Config::default(), 128, 4).unwrap();
        let svg = pipeline.to_svg(&img).unwrap();
        assert!(
            svg.contains("width=\"20\"") || svg.contains("viewBox=\"0 0 20 20\""),
            "{svg}"
        );
    }

    #[test]
    fn factor_one_matches_native_resolution_alpha_frontend() {
        let mut img = solid(10, 10, Color::new(200, 30, 30));
        for x in 0..3 {
            img.set_pixel(x, 0, &Color::new_rgba(200, 30, 30, 0));
        }
        let a = supersampled_alpha_pipeline(&vtracer::Config::default(), 128, 1)
            .unwrap()
            .to_svg(&img)
            .unwrap();
        let b = crate::alpha_pipeline(&vtracer::Config::default(), 128)
            .unwrap()
            .to_svg(&img)
            .unwrap();
        assert_eq!(a, b);
    }
}
