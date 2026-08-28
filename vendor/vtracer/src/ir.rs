//! Core intermediate representation shared by the pipeline stages.
//!
//! Two IRs flow through the pipeline:
//!
//! * [`Segmentation`] — the frontend output: ordered paint layers over a
//!   raster canvas (painter's algorithm, bottom to top). This is what the
//!   [`crate::colorfit`] stages rewrite.
//! * [`VectorDoc`] — the output document: resolved shapes with fitted paths.
//!   This is what the [`crate::optimize`] passes and the [`crate::svg`] writer
//!   operate on.

mod region;
mod vector;

pub use region::{Layer, RegionMask, Segmentation};
pub use vector::{MultiPath, PathCmd, Shape, SubPath, VectorDoc};

use visioncortex::Color;

/// The final appearance of a region.
///
/// `Linear` is an i2v fork addition (`docs/SPEC.md` §4, see VENDORED.md) —
/// upstream 1.0.0-alpha.3 only had `Solid`; this enum lived in this crate,
/// so a plugin outside it could never add a paint kind of its own. `stops`
/// are `(offset, color)` pairs with `offset` in `0.0..=1.0`, in userspace
/// coordinates spanning `(x1,y1)` to `(x2,y2)` — the same absolute document
/// space every path's own coordinates already live in.
#[derive(Debug, Clone, PartialEq)]
pub enum Paint {
    Solid(Color),
    Linear {
        x1: f64,
        y1: f64,
        x2: f64,
        y2: f64,
        stops: Vec<(f64, Color)>,
    },
}

impl Paint {
    /// A representative solid color — the mean of the raster region this
    /// paint replaced, weighted by area, for `Solid`; the first stop's
    /// color for `Linear`. Existing callers (palette snapping, mosaic
    /// merge-color averaging) treat this as "close enough to summarize the
    /// paint," never as an exact value to redisplay — a gradient collapsed
    /// to one color is expected to look different from the original.
    pub fn color(&self) -> Color {
        match self {
            Paint::Solid(c) => *c,
            Paint::Linear { stops, .. } => stops.first().map(|(_, c)| *c).unwrap_or(Color::new(0, 0, 0)),
        }
    }
}
