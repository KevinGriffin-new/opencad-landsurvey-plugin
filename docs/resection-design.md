# Resection / free-station — design + implementation notes

> **Status: implemented.** Engine `crates/landsurvey/src/resection.rs`
> (`resection_combined`, `resection_three_point`, `parse_resection_shots`);
> CLI `landsurvey-cli resect <shots.csv>`; in-app command `LS_RESECT <shots.csv>`.
> Demo seed: `docs/examples/resection-demo.csv`. The sections below are the
> original design sketch, kept as the rationale; the orientation-sign and
> scale-check caveats are pinned by unit tests.

Occupy an **unknown** point P, shoot **known** control points, solve P's
coordinates + instrument orientation. Two flavors:

| Flavor | Measured | Method |
|---|---|---|
| **Combined** (modern TS) | direction **+ distance** to ≥2 known pts | least-squares similarity fit |
| **Angle-only** (classic) | directions only to ≥3 known pts | Tienstra / three-point |

## Key insight: combined resection ≡ Helmert (reuse the engine)
Each shot, in the instrument frame, is a local coordinate:
```
local_i = ( d_i · sin r_i , d_i · cos r_i )     // r = circle reading, d = horiz dist
```
The map instrument-frame → world is rotation (orientation) + translation
(station), scale ≈ 1 — a 2-D conformal. So `transform::helmert_fit(pairs)` with
`pairs = (local_i, known_i)` gives:
- `station   = (fit.c, fit.d)`        — instrument at local origin → its world position
- `orientation = fit.rotation_deg`    — add to circle readings to get grid azimuths
- `scale    = fit.scale`              — free EDM scale check (~1.000; deviation = distance blunder)
- `residuals/RMS = fit_residuals(...)` — resection quality

→ almost no new math; it's `helmert_fit` + a polar→cartesian pre-step.

## Engine sketch — `crates/landsurvey/src/resection.rs` (std-only)
```rust
pub struct ResectionShot {
    pub known: (f64, f64),     // (E, N) of the known point
    pub direction_deg: f64,    // horizontal circle reading
    pub distance: Option<f64>, // horiz distance (None = angle-only)
    pub name: String,
}
pub struct ResectionResult {
    pub station: (f64, f64),
    pub orientation_deg: f64,
    pub scale: f64,                    // EDM scale check
    pub residuals: Vec<(String, f64)>,
    pub rms: f64,
    pub method: ResectionMethod,       // Combined | AngleOnly
}

/// Combined (angle+distance), ≥2 shots with distances → reuse helmert_fit.
pub fn resection_combined(shots: &[ResectionShot]) -> Result<ResectionResult, &'static str>;

/// Angle-only three-point (Tienstra). Exactly 3 known points.
///   W_A = 1/(cot A − cot α), …   (A = triangle interior angle, α = ∠BPC)
///   P = (W_A·A + W_B·B + W_C·C) / (W_A+W_B+W_C)
pub fn resection_three_point(a,b,c, ang_at_p:[f64;3]) -> Result<(f64,f64), &'static str>;
```
*(General angle-only ≥3 directions, no distances → small 3-unknown Gauss-Newton on E,N,θ; v2.)*

## Commands
- **`LS_RESECT <shots.csv>`** (ribbon file picker, like `LS_SURFACE`).
  CSV: `knownN, knownE, direction_deg, distance, name` (blank distance → angle-only).
- Prints: station E/N, orientation, scale check, per-shot residuals + RMS.
- `draw`: Point at the station, ray to each known point (labeled w/ residual),
  label block — layers `LS-RESECT-STATION / RAYS / RESID / LABEL`.
- `anim` (free, via `viz`): known points fixed, rays swing in, station converges.

## Caveats to bake in
- **Danger circle** (angle-only): if P is on the circumcircle of A/B/C, the
  solution is indeterminate (Tienstra weights blow up) — detect & warn.
- **Combined** needs ≥2 distance shots; report if fewer.
- **Scale guard**: flag if `scale` strays from 1 beyond a tolerance (distance/EDM blunder).
- **Orientation sign**: nail reading→azimuth vs `cogo::inverse` (CW-from-N) with a known test.

## Validation
Golden-capture a resection in MicroSurvey / Civil 3D (same shots); assert clone
station/orientation/residuals match (the perishable-output capture pattern).
