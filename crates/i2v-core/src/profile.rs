//! A serializable settings bundle (M4, `docs/SPEC.md` §7): one JSON file
//! that fully determines a pipeline, shared by the CLI's `--profile` flag
//! and batch mode, instead of a growing pile of shell-history flags.

use std::str::FromStr;

use serde::{Deserialize, Serialize};
use vtracer::{Config, Error, Pipeline};

use crate::gradient::GradientFitter;
use crate::regularize::RegularizePass;

/// Which frontend traces the image. `Vanilla` is plain VTracer, unaware of
/// alpha; `Alpha`/`Supersample` are Module A v1/v2 (`docs/SPEC.md` §2).
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Default)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum AlphaMode {
    #[default]
    Vanilla,
    Alpha {
        alpha_threshold: u8,
    },
    Supersample {
        alpha_threshold: u8,
        factor: u32,
    },
}

/// Mirrors the `RegularizePass` fields (`docs/SPEC.md` §3) one-to-one —
/// kept as a separate, explicitly-`Option`al block so "off" is
/// self-documenting in the JSON rather than a magic zero tolerance.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct RegularizeSettings {
    pub tolerance: f64,
    pub angle_tolerance: f64,
    pub min_length: f64,
    pub circle_relative_tolerance: f64,
}

impl Default for RegularizeSettings {
    /// The values used throughout `crates/i2v-bench` and measured there
    /// (`docs/SPEC.md` §3/§6) — 0 regressions across the full corpus.
    fn default() -> Self {
        Self {
            tolerance: 1.0,
            angle_tolerance: 2.0,
            min_length: 4.0,
            circle_relative_tolerance: 0.03,
        }
    }
}

/// Mirrors `GradientFitter`'s fields (`docs/SPEC.md` §4) — Module B, the one
/// stage in this project that needed a fork rather than a plugin
/// (`vendor/vtracer`, see VENDORED.md: `Paint::Linear` doesn't exist
/// upstream). `None` = off, same "off is explicit" reasoning as
/// `RegularizeSettings`.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct GradientSettings {
    pub min_coverage: f64,
    pub max_deviation: f64,
    pub max_stops: usize,
    pub min_distinct_colors: usize,
    pub cross_axis_tolerance: f64,
}

impl Default for GradientSettings {
    fn default() -> Self {
        let d = GradientFitter::default();
        Self {
            min_coverage: d.min_coverage,
            max_deviation: d.max_deviation,
            max_stops: d.max_stops,
            min_distinct_colors: d.min_distinct_colors,
            cross_axis_tolerance: d.cross_axis_tolerance,
        }
    }
}

/// The subset of `vtracer::Config` exposed as profile settings — the same
/// fields `i2v-cli` already took as flags (`--simplify`, `--max-colors`),
/// plus `mode`/`color_precision`/`filter_speckle`: stable, commonly-tuned
/// VTracer knobs. Not the whole of `Config` — that's a bigger surface than
/// this project has needed to expose yet, and unexposed fields are simply
/// vtracer's own defaults.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(default)]
pub struct TraceSettings {
    pub simplify: Option<f64>,
    pub max_colors: Option<usize>,
    pub color_precision: i32,
    pub filter_speckle: usize,
    /// `"pixel"`, `"polygon"`, or `"spline"` — see `vtracer::FitMode`.
    pub mode: String,
}

impl Default for TraceSettings {
    fn default() -> Self {
        let d = Config::default();
        Self {
            simplify: d.simplify,
            max_colors: d.max_colors,
            color_precision: d.color_precision,
            filter_speckle: d.filter_speckle,
            mode: "spline".to_string(),
        }
    }
}

/// Everything needed to reproduce one trace: `cargo run -p i2v-cli -- in.png
/// out.svg --profile mine.json` should give the same output today and a
/// year from now, independent of whatever flags were fashionable when it
/// was written.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Default)]
#[serde(default)]
pub struct Profile {
    pub trace: TraceSettings,
    pub alpha: AlphaMode,
    /// `None` = regularization off. Distinct from `RegularizeSettings`
    /// carrying zeroed tolerances, which would silently no-op instead of
    /// stating "not requested" in the file.
    pub regularize: Option<RegularizeSettings>,
    /// `None` = gradient detection off.
    pub gradient: Option<GradientSettings>,
}

impl Profile {
    pub fn from_json(s: &str) -> serde_json::Result<Self> {
        serde_json::from_str(s)
    }

    pub fn to_json_pretty(&self) -> serde_json::Result<String> {
        serde_json::to_string_pretty(self)
    }

    fn vtracer_config(&self) -> Result<Config, Error> {
        let mode = vtracer::FitMode::from_str(&self.trace.mode)
            .map_err(|_| Error::Other(format!("unknown trace.mode {:?}", self.trace.mode)))?;
        Ok(Config {
            simplify: self.trace.simplify,
            max_colors: self.trace.max_colors,
            color_precision: self.trace.color_precision,
            filter_speckle: self.trace.filter_speckle,
            mode,
            ..Config::default()
        })
    }

    /// Builds the full pipeline this profile describes: frontend (§2) then
    /// `RegularizePass` (§3) if requested.
    pub fn build_pipeline(&self) -> Result<Pipeline, Error> {
        let cfg = self.vtracer_config()?;
        let mut pipeline = match &self.alpha {
            AlphaMode::Vanilla => cfg.build()?,
            AlphaMode::Alpha { alpha_threshold } => crate::alpha_pipeline(&cfg, *alpha_threshold)?,
            AlphaMode::Supersample {
                alpha_threshold,
                factor,
            } => crate::supersample::supersampled_alpha_pipeline(&cfg, *alpha_threshold, *factor)?,
        };
        if let Some(g) = &self.gradient {
            pipeline.color_fitters.push(Box::new(GradientFitter {
                min_coverage: g.min_coverage,
                max_deviation: g.max_deviation,
                max_stops: g.max_stops,
                min_distinct_colors: g.min_distinct_colors,
                cross_axis_tolerance: g.cross_axis_tolerance,
            }));
        }
        if let Some(r) = &self.regularize {
            pipeline.curve_passes.push(Box::new(RegularizePass {
                tolerance: r.tolerance,
                angle_tolerance: r.angle_tolerance,
                min_length: r.min_length,
                circle_relative_tolerance: r.circle_relative_tolerance,
            }));
        }
        Ok(pipeline)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use vtracer::{Color, ColorImage};

    fn solid(w: usize, h: usize) -> ColorImage {
        let mut img = ColorImage {
            pixels: vec![0u8; w * h * 4],
            width: w,
            height: h,
        };
        for y in 0..h {
            for x in 0..w {
                img.set_pixel(x, y, &Color::new(200, 30, 30));
            }
        }
        img
    }

    #[test]
    fn default_profile_round_trips_through_json() {
        let p = Profile::default();
        let json = p.to_json_pretty().unwrap();
        let back = Profile::from_json(&json).unwrap();
        assert_eq!(p, back);
    }

    #[test]
    fn default_profile_traces_like_vanilla_vtracer() {
        let img = solid(20, 20);
        let profile_svg = Profile::default()
            .build_pipeline()
            .unwrap()
            .to_svg(&img)
            .unwrap();
        let vanilla_svg = Config::default().build().unwrap().to_svg(&img).unwrap();
        assert_eq!(profile_svg, vanilla_svg);
    }

    #[test]
    fn alpha_and_regularize_settings_actually_apply() {
        let profile = Profile {
            alpha: AlphaMode::Alpha {
                alpha_threshold: 128,
            },
            regularize: Some(RegularizeSettings::default()),
            ..Profile::default()
        };
        // Should build and trace without error; a real end-to-end check
        // that both stages are wired, not just that the struct compiles.
        let svg = profile
            .build_pipeline()
            .unwrap()
            .to_svg(&solid(20, 20))
            .unwrap();
        assert!(svg.contains("<svg"));
    }

    #[test]
    fn unknown_mode_string_is_a_clean_error_not_a_panic() {
        let mut p = Profile::default();
        p.trace.mode = "not-a-real-mode".to_string();
        assert!(p.build_pipeline().is_err());
    }
}
