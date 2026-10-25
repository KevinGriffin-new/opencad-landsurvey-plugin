//! Headless COGO + PNEZD survey geometry for Open CAD Studio.
//!
//! Layer-C engine (see `docs/plugin-architecture.md`): depends only on `std`
//! and `serde` — no `iced`, `acadrust`, or host types — so it builds unchanged
//! for CLI/WASM. Its in-app consumer is the `opencad-landsurvey-plugin` cdylib,
//! which wraps these functions in ribbon/command/XDATA glue.

pub mod alignment;
pub mod cogo;
pub mod contours;
pub mod dxf;
pub mod finish;
pub mod label_style;
pub mod landxml;
pub mod library;
pub mod plan;
pub mod pnezd;
pub mod resection;
pub mod rw5;
pub mod sdr;
pub mod surface;
pub mod symbols;
pub mod text;
pub mod transform;
pub mod viz;
