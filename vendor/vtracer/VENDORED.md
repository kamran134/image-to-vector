# Vendored fork of `vtracer`

Source: https://github.com/visioncortex/vtracer, `crates/vtracer` at tag
`1.0.0-alpha.3`, commit `58221025d5cfc6abbe12745942ae867b57ad3117`.

## Why vendored instead of a plain crates.io dependency

Module B (gradients, `docs/SPEC.md` §4) needs a `Paint::Linear` variant.
`Paint` is defined in this crate; adding a variant to a foreign enum isn't
possible from outside it. Everything else in this project (modules A and C)
is a plugin against `vtracer`'s public trait extension points — this is the
one piece that genuinely requires patching `vtracer` itself, so it's
vendored rather than pulled from crates.io.

## What's changed vs. upstream

Tracked in git history from this file's initial commit onward — diff
`vendor/vtracer` against a fresh checkout of the tag above to see exactly
what's patched. Summary kept current here:

- `src/lib.rs`: one crate-level `#![allow(...)]` for clippy findings that
  predate this fork (verified: still present against unmodified upstream
  1.0.0-alpha.3 under clippy 1.94). Not a restyle — see the comment above
  the attribute for why it exists and why it's one line, not scattered
  `#[allow]`s at each site. Functional patches (`Paint::Linear` onward) get
  their own entries here as they land.

## What's deliberately not vendored

- `tests/spline_fit.rs`, and the `snap_leaves_no_debris` test from
  `tests/watershed.rs`: both load
  `docs/assets/samples/Cityscape Sunset_DFM3-01.jpg`, a ~250KB stock photo
  living outside `crates/vtracer` in the upstream repo, with no license or
  attribution found alongside it. Not vendored rather than guessed at.
  Everything else in both files (17/18 watershed tests, all of them a real
  regression check) is kept.
- `Cargo.lock`, CI config, the wasm/Node/Python bindings, the desktop app,
  docs — none of it is needed to build/test this one library crate.

## Verification before any patch

`cargo test` inside `vendor/vtracer` (standalone, before wiring into this
workspace): 80 passed, 0 failed — the full upstream suite minus the two
tests above. This is the baseline every subsequent patch is checked against.

## Upgrading

If upstream ships a fixed release, re-vendor from that tag and re-apply the
patches described above (small enough to redo by hand — see git history for
the exact diff). Do not merge/rebase this directory against upstream's git
history; it isn't a git subtree, just a copy of `src/`, `tests/` (minus the
two exclusions), and `LICENSE`.

## License

MIT (`LICENSE` in this directory, copied from the upstream repository root).
