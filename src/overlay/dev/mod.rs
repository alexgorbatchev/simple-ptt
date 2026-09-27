//! Dev tools for tuning and checking the overlay's look on this branch:
//! `simple-ptt --debug` (live tuner) and `simple-ptt --overlay-snapshot <dir>`
//! (captures of every overlay state). They and the `OverlayWindow` methods
//! only they call (`pin_to_top`, `glass_tuning`, `set_glass_tuning`,
//! `halo_is_progressive`, `glass_internals_now`) are removed before merging to
//! main, once the tuned values are the `GlassTuning` defaults.

mod curve_editor;
pub mod snapshot;
pub mod tuner;
