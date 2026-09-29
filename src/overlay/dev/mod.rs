//! Dev tools for tuning and checking the overlay's look:
//! `simple-ptt --debug` (live tuner) and `simple-ptt --overlay-snapshot <dir>`
//! (captures of every overlay state). The `OverlayWindow` methods only they
//! call (`pin_to_top`, `glass_tuning`, `set_glass_tuning`,
//! `halo_is_progressive`, `glass_internals_now`) are not for product code.
//! Tuned values become the `GlassTuning` defaults in `glass.rs`.

mod curve_editor;
pub mod snapshot;
pub mod tuner;
