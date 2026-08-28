//! # vtracer
//!
//! A vectorization *framework*: raster images become vector graphics through a
//! pipeline of pluggable stages.
//!
//! ```text
//! Frontend ─▶ ColorFitter* ─▶ Compositing ─▶ CurveFitter ─▶ CurvePass* ─▶ VectorDoc
//!                                                                            │
//!                                                        OptimizerPass* ─────┤
//!                                                                            ▼
//!                                                                        SvgWriter ─▶ SVG
//! ```
//!
//! The crate is wasm-safe: it performs no file or image I/O (that lives in the
//! `vtracer-cli` wrapper). Everything here compiles to
//! `wasm32-unknown-unknown`.
//!
//! ## Quick start
//!
//! ```no_run
//! use vtracer::{Config, ColorImage};
//!
//! # fn load() -> ColorImage { todo!() }
//! let img: ColorImage = load();
//! let svg = Config::default().build().unwrap().to_svg(&img).unwrap();
//! ```
//!
//! For finer control, assemble a [`Pipeline`] directly from the stage traits
//! in [`frontend`], [`colorfit`], [`fitter`], [`simplify`], [`compose`],
//! [`optimize`], and [`svg`].

// Vendored (see VENDORED.md): this crate is patched only where the fork
// needed it functionally (Paint::Linear onward), never restyled to match
// this workspace's own clippy bar. Cargo lints local `path` dependencies'
// source the same as workspace members — with no way to scope that off per
// dependency — so upstream's pre-existing clippy findings (current as of
// vtracer 1.0.0-alpha.3, clippy 1.94) would otherwise fail i2v-core's own
// `-D warnings` CI gate for code this fork isn't touching. Kept as one
// explicit, auditable line rather than a dozen scattered `#[allow]`s so a
// future re-vendor only has to check this still matches, not hunt for them.
#![allow(
    clippy::collapsible_if,
    clippy::default_constructed_unit_structs,
    clippy::derivable_impls,
    clippy::manual_is_multiple_of,
    clippy::needless_range_loop,
    clippy::needless_update,
    clippy::unnecessary_map_or,
    clippy::approx_constant
)]

pub mod colorfit;
pub mod compose;
pub mod config;
pub mod error;
pub mod fitter;
pub mod frontend;
pub mod ir;
pub mod mosaic;
pub mod optimize;
pub mod pipeline;
pub mod progress;
pub mod session;
pub mod simplify;
pub mod svg;

pub use config::{Clustering, Config, FitMode, Hierarchical, Preset, SegmentKey};
pub use error::Error;
pub use frontend::Threshold;
pub use ir::{Segmentation, VectorDoc};
pub use pipeline::Pipeline;
pub use progress::{CancelToken, Phase, Progress};
pub use session::Session;

// Re-export the visioncortex value types callers need at the boundary.
pub use visioncortex::{Color, ColorImage, PointF64, PointI32};
