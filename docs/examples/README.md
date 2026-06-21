# Demo seeds — reproducible explainer SVGs

The animated SVG explainers are **data-driven**: each one renders from real
input, so the clip shows the actual result (scale, rotation, residuals,
distance, bearing, solved station…). The seed inputs live here so any explainer
can be regenerated from the repo alone — no external scratch data.

Build the headless CLI once, then run a command below. Each writes a
self-contained, looping SVG you can open in any browser.

```sh
cargo build -p landsurvey-cli
CLI=./target/debug/landsurvey-cli
```

| Explainer | Seed | Command |
|-----------|------|---------|
| **Helmert** | [`helmert-pairs-demo.csv`](helmert-pairs-demo.csv) | `$CLI helmert docs/examples/helmert-pairs-demo.csv --anim helmert.svg --teach` |
| **RTS** | [`rts-points-demo.csv`](rts-points-demo.csv) | `$CLI rts docs/examples/rts-points-demo.csv --base 1000,1000 --to 5000,4000 --rot 18 --scale 1.25 --anim rts.svg --teach` |
| **Inverse** | *(inline coords)* | `$CLI inverse 1000 1000 1080 1100 --anim inverse.svg` |
| **Resection** | [`resection-demo.csv`](resection-demo.csv) | `$CLI resect docs/examples/resection-demo.csv --anim resection.svg` |

Notes
- `--teach` amplifies a near-grid transform (small rotation/scale) so the
  *operation* reads on screen, with an on-screen note; drop it for a faithful
  clip. It has no effect on Inverse / Resection.
- In the app, the explainers come from an `anim` / `teach` keyword on
  `LS_HELMERT`, `LS_INVERSE`, and `LS_RESECT` (the SVG is written to a temp
  folder and its path printed). RTS animation is currently CLI-only.
- The resection seed recovers station **E 5000.000, N 4000.000**, orientation
  **+20.000°**, scale **1.000000** (truth noted in the CSV header).
