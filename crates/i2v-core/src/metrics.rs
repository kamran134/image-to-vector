//! Rasterize an SVG back to RGBA and score it against the source image, so
//! "smaller SVG" and "fewer colors" claims can be checked against whether
//! the result still looks right — the gap flagged as open in `i2v-bench`
//! before this module existed.

use image::RgbaImage;
use resvg::tiny_skia;

/// Render `svg` onto a `width`x`height` transparent canvas, scaled to fill
/// it regardless of the SVG's own declared size (VTracer's output viewBox
/// always matches the source image, so in practice this is a 1:1 render,
/// but staying explicit avoids a silent mismatch if that ever changes).
pub fn render_svg(svg: &str, width: u32, height: u32) -> Result<RgbaImage, String> {
    let tree = usvg::Tree::from_str(svg, &usvg::Options::default()).map_err(|e| e.to_string())?;
    let size = tree.size();
    if size.width() <= 0.0 || size.height() <= 0.0 {
        return Err("SVG has zero size".into());
    }

    let mut pixmap = tiny_skia::Pixmap::new(width, height).ok_or("zero-size pixmap")?;
    let transform = tiny_skia::Transform::from_scale(
        width as f32 / size.width(),
        height as f32 / size.height(),
    );
    resvg::render(&tree, transform, &mut pixmap.as_mut());

    let mut out = RgbaImage::new(width, height);
    for (i, p) in pixmap.pixels().iter().enumerate() {
        let c = p.demultiply();
        out.put_pixel(
            i as u32 % width,
            i as u32 / width,
            image::Rgba([c.red(), c.green(), c.blue(), c.alpha()]),
        );
    }
    Ok(out)
}

/// Per-channel RGBA absolute error between two same-size images: `(mean,
/// p99)`, both on the 0..255 scale. Panics if the images differ in size —
/// callers always render to the source's own dimensions, so a mismatch is a
/// bug, not an input to handle gracefully.
pub fn rgba_error(a: &RgbaImage, b: &RgbaImage) -> (f64, f64) {
    assert_eq!(
        a.dimensions(),
        b.dimensions(),
        "compared images must be the same size"
    );

    let mut diffs: Vec<u8> = Vec::with_capacity((a.width() * a.height() * 4) as usize);
    for (pa, pb) in a.pixels().zip(b.pixels()) {
        for c in 0..4 {
            diffs.push(pa.0[c].abs_diff(pb.0[c]));
        }
    }

    let mean = diffs.iter().map(|&d| d as f64).sum::<f64>() / diffs.len() as f64;

    diffs.sort_unstable();
    let p99_idx = ((diffs.len() as f64) * 0.99) as usize;
    let p99 = diffs[p99_idx.min(diffs.len() - 1)] as f64;

    (mean, p99)
}

/// Structural similarity (Wang et al., 2004) on luma, over non-overlapping
/// 8x8 blocks — a standard simplification of the full 11x11 Gaussian-window
/// version: cheaper, and enough to catch "the shape moved/vanished" without
/// needing a Gaussian kernel. Alpha is folded in by compositing over black
/// first, so a region that VTracer dropped (transparent where the source
/// wasn't) shows up as a luma difference instead of being invisible to a
/// color-only metric.
pub fn ssim(a: &RgbaImage, b: &RgbaImage) -> f64 {
    assert_eq!(
        a.dimensions(),
        b.dimensions(),
        "compared images must be the same size"
    );

    const WIN: u32 = 8;
    const L: f64 = 255.0;
    const C1: f64 = (0.01 * L) * (0.01 * L);
    const C2: f64 = (0.03 * L) * (0.03 * L);

    let luma = |img: &RgbaImage, x: u32, y: u32| -> f64 {
        let p = img.get_pixel(x, y).0;
        let a = p[3] as f64 / 255.0;
        // composite over black: transparent counts as dark, not "equal to
        // whatever the other image has there".
        0.299 * p[0] as f64 * a + 0.587 * p[1] as f64 * a + 0.114 * p[2] as f64 * a
    };

    let (w, h) = a.dimensions();
    let mut total = 0.0;
    let mut windows = 0u64;

    let mut y = 0;
    while y < h {
        let mut x = 0;
        while x < w {
            let x_end = (x + WIN).min(w);
            let y_end = (y + WIN).min(h);
            let n = ((x_end - x) * (y_end - y)) as f64;

            let (mut sum_a, mut sum_b) = (0.0, 0.0);
            for yy in y..y_end {
                for xx in x..x_end {
                    sum_a += luma(a, xx, yy);
                    sum_b += luma(b, xx, yy);
                }
            }
            let (mean_a, mean_b) = (sum_a / n, sum_b / n);

            let (mut var_a, mut var_b, mut covar) = (0.0, 0.0, 0.0);
            for yy in y..y_end {
                for xx in x..x_end {
                    let da = luma(a, xx, yy) - mean_a;
                    let db = luma(b, xx, yy) - mean_b;
                    var_a += da * da;
                    var_b += db * db;
                    covar += da * db;
                }
            }
            var_a /= n;
            var_b /= n;
            covar /= n;

            let window_ssim = ((2.0 * mean_a * mean_b + C1) * (2.0 * covar + C2))
                / ((mean_a * mean_a + mean_b * mean_b + C1) * (var_a + var_b + C2));

            total += window_ssim;
            windows += 1;
            x += WIN;
        }
        y += WIN;
    }

    total / windows as f64
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::Rgba;

    fn solid(w: u32, h: u32, c: Rgba<u8>) -> RgbaImage {
        RgbaImage::from_pixel(w, h, c)
    }

    #[test]
    fn identical_images_have_zero_error_and_ssim_one() {
        let img = solid(16, 16, Rgba([200, 30, 30, 255]));
        let (mean, p99) = rgba_error(&img, &img);
        assert_eq!(mean, 0.0);
        assert_eq!(p99, 0.0);
        assert!((ssim(&img, &img) - 1.0).abs() < 1e-9);
    }

    #[test]
    fn opposite_images_have_high_error_and_low_ssim() {
        let black = solid(16, 16, Rgba([0, 0, 0, 255]));
        let white = solid(16, 16, Rgba([255, 255, 255, 255]));
        let (mean, _) = rgba_error(&black, &white);
        // RGB channels each contribute 255, alpha is identical (both opaque)
        // and contributes 0, so the 4-channel mean caps out at 191.25.
        assert!(
            mean > 150.0,
            "mean error {mean} should be large for black vs white"
        );
        assert!(ssim(&black, &white) < 0.5);
    }

    #[test]
    fn render_svg_reproduces_a_solid_rect() {
        let svg = r##"<svg xmlns="http://www.w3.org/2000/svg" width="10" height="10" viewBox="0 0 10 10"><rect width="10" height="10" fill="#C81E1E"/></svg>"##;
        let img = render_svg(svg, 10, 10).unwrap();
        let center = img.get_pixel(5, 5);
        assert_eq!(
            (center[0], center[1], center[2], center[3]),
            (200, 30, 30, 255)
        );
    }

    #[test]
    fn render_svg_respects_transparency() {
        let svg = r#"<svg xmlns="http://www.w3.org/2000/svg" width="4" height="4" viewBox="0 0 4 4"></svg>"#;
        let img = render_svg(svg, 4, 4).unwrap();
        assert_eq!(img.get_pixel(0, 0)[3], 0);
    }
}
