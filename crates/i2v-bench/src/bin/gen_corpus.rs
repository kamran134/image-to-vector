//! Regenerates `corpus/` from scratch: `cargo run -p i2v-bench --bin gen_corpus`.
//!
//! No stock photo library is available in this environment, so the corpus
//! is procedurally generated rather than curated from real assets. It's
//! honest about that limitation (see docs/SPEC.md §6) but still exercises
//! the six failure modes the spec cares about: real alpha antialiasing,
//! crisp flat shapes, binary-ish line art, banded gradients, blocky pixel
//! art, and smooth photo-like noise. Deterministic (fixed seed per image),
//! so the corpus is reproducible and diffable across runs.

use std::path::Path;

use image::{Rgba, RgbaImage};

/// Small deterministic PRNG (xorshift32) — no need for a `rand` dependency
/// just to scatter noise pixels reproducibly.
struct Rng(u32);
impl Rng {
    fn new(seed: u32) -> Self {
        Rng(seed | 1)
    }
    fn next_u32(&mut self) -> u32 {
        let mut x = self.0;
        x ^= x << 13;
        x ^= x >> 17;
        x ^= x << 5;
        self.0 = x;
        x
    }
    fn next_f32(&mut self) -> f32 {
        (self.next_u32() % 10_000) as f32 / 10_000.0
    }
}

fn lerp_u8(a: u8, b: u8, t: f32) -> u8 {
    (a as f32 * (1.0 - t) + b as f32 * t).round() as u8
}

fn blend(a: Rgba<u8>, b: Rgba<u8>, t: f32) -> Rgba<u8> {
    Rgba([
        lerp_u8(a.0[0], b.0[0], t),
        lerp_u8(a.0[1], b.0[1], t),
        lerp_u8(a.0[2], b.0[2], t),
        lerp_u8(a.0[3], b.0[3], t),
    ])
}

/// A rounded badge with a translucent, antialiased-edge silhouette on a
/// transparent canvas — the case Module A targets directly.
fn transparent_logo(size: u32, fg: Rgba<u8>, bg_hint: Rgba<u8>) -> RgbaImage {
    let mut img = RgbaImage::from_pixel(size, size, Rgba([0, 0, 0, 0]));
    let r = size as f32 * 0.42;
    let (cx, cy) = (size as f32 / 2.0, size as f32 / 2.0);
    for y in 0..size {
        for x in 0..size {
            let d = (((x as f32 - cx).powi(2) + (y as f32 - cy).powi(2)).sqrt() - r) / 1.5;
            let coverage = (0.5 - d).clamp(0.0, 1.0);
            if coverage <= 0.0 {
                continue;
            }
            // The antialiased rim carries `bg_hint`'s color bleeding in, as
            // a real "trim on a colored canvas" export would leave behind.
            let color = if coverage >= 1.0 {
                fg
            } else {
                blend(bg_hint, fg, coverage)
            };
            img.put_pixel(
                x,
                y,
                Rgba([color.0[0], color.0[1], color.0[2], (coverage * 255.0) as u8]),
            );
        }
    }
    img
}

/// Crisp flat-color shapes, real alpha but a hard edge (no antialiasing) —
/// the case an icon exporter with "snap to pixel" produces.
fn flat_icon(size: u32, colors: [Rgba<u8>; 2]) -> RgbaImage {
    let mut img = RgbaImage::from_pixel(size, size, Rgba([0, 0, 0, 0]));
    let pad = size / 6;
    for y in pad..size - pad {
        for x in pad..size - pad {
            let inner = x > size / 2;
            let c = if inner { colors[1] } else { colors[0] };
            img.put_pixel(x, y, Rgba([c.0[0], c.0[1], c.0[2], 255]));
        }
    }
    img
}

/// Thin dark strokes on a light background with antialiased edges — a
/// scanned-line-art stand-in.
fn lineart(size: u32, seed: u32) -> RgbaImage {
    let mut img = RgbaImage::from_pixel(size, size, Rgba([250, 250, 248, 255]));
    let mut rng = Rng::new(seed);
    for _ in 0..(size / 8).max(3) {
        let x0 = (rng.next_f32() * size as f32) as i32;
        let y0 = (rng.next_f32() * size as f32) as i32;
        let x1 = (rng.next_f32() * size as f32) as i32;
        let y1 = (rng.next_f32() * size as f32) as i32;
        draw_antialiased_line(&mut img, x0, y0, x1, y1, Rgba([20, 20, 25, 255]));
    }
    img
}

fn draw_antialiased_line(img: &mut RgbaImage, x0: i32, y0: i32, x1: i32, y1: i32, color: Rgba<u8>) {
    let steps = ((x1 - x0).abs()).max((y1 - y0).abs()).max(1);
    for i in 0..=steps {
        let t = i as f32 / steps as f32;
        let x = x0 as f32 + (x1 - x0) as f32 * t;
        let y = y0 as f32 + (y1 - y0) as f32 * t;
        for (dx, dy, cov) in [(0.0, 0.0, 1.0f32), (1.0, 0.0, 0.35), (0.0, 1.0, 0.35)] {
            let (px, py) = (x + dx, y + dy);
            if px < 0.0 || py < 0.0 || px >= img.width() as f32 || py >= img.height() as f32 {
                continue;
            }
            let existing = *img.get_pixel(px as u32, py as u32);
            img.put_pixel(px as u32, py as u32, blend(existing, color, cov));
        }
    }
}

/// Opaque banded gradient — no gradient support exists yet (Module B), so
/// this is what VTracer currently turns into a dozen flat layers.
fn gradient_logo(w: u32, h: u32, from: Rgba<u8>, to: Rgba<u8>) -> RgbaImage {
    let mut img = RgbaImage::new(w, h);
    for y in 0..h {
        let t = y as f32 / (h - 1).max(1) as f32;
        let c = blend(from, to, t);
        for x in 0..w {
            img.put_pixel(x, y, Rgba([c.0[0], c.0[1], c.0[2], 255]));
        }
    }
    img
}

/// Small blocky sprite, nearest-neighbor edges only — pixel art must not be
/// smoothed away by curve fitting.
fn pixel_art(cells: u32, cell_px: u32, seed: u32, palette: &[Rgba<u8>]) -> RgbaImage {
    let size = cells * cell_px;
    let mut img = RgbaImage::from_pixel(size, size, Rgba([0, 0, 0, 0]));
    let mut rng = Rng::new(seed);
    for cy in 0..cells {
        for cx in 0..cells {
            if rng.next_f32() < 0.25 {
                continue; // leave transparent, sprites aren't solid rectangles
            }
            let c = palette[(rng.next_u32() as usize) % palette.len()];
            for y in 0..cell_px {
                for x in 0..cell_px {
                    img.put_pixel(cx * cell_px + x, cy * cell_px + y, c);
                }
            }
        }
    }
    img
}

/// Smooth radial gradient plus per-pixel noise, quantization-friendly but
/// not flat — a synthetic stand-in for a photo posterized to a small
/// palette, since no real photo is available in this environment.
fn photo_poster(size: u32, seed: u32) -> RgbaImage {
    let mut img = RgbaImage::new(size, size);
    let mut rng = Rng::new(seed);
    let (cx, cy) = (size as f32 / 2.0, size as f32 / 2.0);
    let max_d = (cx * cx + cy * cy).sqrt();
    for y in 0..size {
        for x in 0..size {
            let d = (((x as f32 - cx).powi(2) + (y as f32 - cy).powi(2)).sqrt() / max_d)
                .clamp(0.0, 1.0);
            let base = blend(Rgba([250, 210, 120, 255]), Rgba([40, 60, 120, 255]), d);
            let noise = (rng.next_f32() - 0.5) * 24.0;
            let jitter = |c: u8| (c as f32 + noise).clamp(0.0, 255.0) as u8;
            img.put_pixel(
                x,
                y,
                Rgba([jitter(base.0[0]), jitter(base.0[1]), jitter(base.0[2]), 255]),
            );
        }
    }
    img
}

fn main() {
    let corpus = Path::new(env!("CARGO_MANIFEST_DIR")).join("corpus");
    std::fs::create_dir_all(&corpus).unwrap();

    let images: Vec<(&str, RgbaImage)> = vec![
        (
            "transparent-logo-01.png",
            transparent_logo(96, Rgba([200, 30, 30, 255]), Rgba([30, 30, 200, 255])),
        ),
        (
            "transparent-logo-02.png",
            transparent_logo(128, Rgba([40, 160, 90, 255]), Rgba([250, 250, 250, 255])),
        ),
        (
            "transparent-logo-03.png",
            transparent_logo(64, Rgba([230, 140, 20, 255]), Rgba([20, 20, 20, 255])),
        ),
        (
            "flat-icon-01.png",
            flat_icon(64, [Rgba([220, 60, 60, 255]), Rgba([60, 90, 220, 255])]),
        ),
        (
            "flat-icon-02.png",
            flat_icon(96, [Rgba([20, 160, 120, 255]), Rgba([240, 200, 40, 255])]),
        ),
        ("lineart-01.png", lineart(96, 1)),
        ("lineart-02.png", lineart(128, 2)),
        (
            "gradient-logo-01.png",
            gradient_logo(96, 96, Rgba([250, 90, 90, 255]), Rgba([90, 40, 160, 255])),
        ),
        (
            "gradient-logo-02.png",
            gradient_logo(
                64,
                120,
                Rgba([40, 200, 200, 255]),
                Rgba([250, 250, 60, 255]),
            ),
        ),
        (
            "pixel-art-01.png",
            pixel_art(
                12,
                8,
                3,
                &[
                    Rgba([230, 60, 60, 255]),
                    Rgba([60, 200, 90, 255]),
                    Rgba([40, 60, 220, 255]),
                ],
            ),
        ),
        (
            "pixel-art-02.png",
            pixel_art(
                16,
                6,
                4,
                &[
                    Rgba([250, 220, 40, 255]),
                    Rgba([30, 30, 30, 255]),
                    Rgba([240, 240, 240, 255]),
                ],
            ),
        ),
        ("photo-poster-01.png", photo_poster(120, 5)),
        ("photo-poster-02.png", photo_poster(96, 6)),
    ];

    for (name, img) in &images {
        img.save(corpus.join(name)).unwrap();
    }
    println!("wrote {} images to {}", images.len(), corpus.display());
}
