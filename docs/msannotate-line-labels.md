# MicroSurvey (MsAnnotate) line-label style

`LS_INVERSE … draw` labels a line's bearing and distance the way MicroSurvey
CAD 2027 does. The engine side is `crates/landsurvey/src/label_style.rs`, and
`LS_LABELSTYLE` chooses and loads the styles.

## Where the style lives

MicroSurvey CAD 2027 keeps MsAnnotate settings **per job**, in
`Documents/MicroSurvey/Jobs/<job>.MSJ/config.json`. Older tooling looked in
`%APPDATA%/MicroSurvey/MSCAD/<version>/config.json`, which no longer holds them.
The file has `bearingstyles[10]`, `distancestyles[10]` and `curvestyles[10]`,
plus a `…styleindex` key for each that selects the current style.
`LabelStyles::from_msj_config` reads the bearing and distance sets. Keys it
doesn't use are ignored.

The shipped defaults are `BEARING1` (colour 1, leroy 60, rounding 1",
textoffset 0.5) and `DISTANCE1` (colour 3, leroy 60, 2 decimal places,
textoffset 0.5). Both use `MSURVEY.SHX` and sit on a layer named after the style.

## How the text is built

These rules were recovered by static analysis of MSCAD 2027 binaries (REA +
Ghidra 12.1.4). No MicroSurvey code was copied; only the observable format
rules were reimplemented.

| Rule | Source | Status |
|---|---|---|
| The distance is CAD `rtos(value, LUNITS, decimalplaces)`, with feet ×12 when LUNITS is architectural or engineering | `config.dll!FormatDistanceString` | observed |
| Bearing = `N` + quadrantspacing×space + DMS + quadrantspacing×space + `E` (`N%s%s%sE`, likewise S/E, S/W, N/W) | `mscad.dll` quadrant formatter called by `FormatBearing` | observed |
| DMS = degrees, `°`, spacesbetweendms×space, `%02` minutes, `'`, spaces, `%02` seconds, `"` | `mscad.dll` DMS builder | observed |
| Degrees are padded to width 2: zeros with `useleadingzero`, otherwise a **space** (`N 5°…`) | same (`%*.0f` vs `%0*.0f`) | observed |
| The angle is rounded to a multiple of `rounding` seconds, halves away from zero, before the DMS split (so carries propagate) | same | observed |
| `rounding == 60` shows degrees and minutes only. `rounding > 60` falls back to 1" | `FormatBearing` precision select | observed |
| A bearing of exactly 90° reads `N90°…E` / `N90°…W`, never `S90°` | quadrant formatter (`"90"` check) | observed |
| `textoffset` is copied into the placement config (slot 0) | `labeling.dll!GetLabelPlaceDistCfg` / `GetLabelPlaceDirCfg` | observed |
| `textoffset` is the clearance from the line **in text heights** | — | inferred: the placement consumer sits behind `mscad.dll`'s dynamic `DllFunc` dispatch table and wasn't traced |
| Leroy is plotted height in thousandths of an inch (60 ≈ 1.5 mm) | MSCAD dialog wording, `cnf_get_*_leroy` | inferred |
| Distance `scalefactor` multiplies the labelled value, and prefix/suffix/foot mark wrap it | `DistanceStyle::GetFootSymbol`, `SetScaleFactor` | inferred |

Not modelled yet: reference-north rotation (`usereferencenorth`), south
azimuth, non-DMS angle units (decimal degrees, grads, radians), and
`obliqueangle` / `width` on the text entity.

## Verifying against MicroSurvey

The rules above come from static analysis and have not been checked against
labels MicroSurvey actually drew. To confirm them, label a few lines in a
throwaway MSCAD job with MsAnnotate and compare those TEXT entities with
`LS_INVERSE … draw` using `LS_LABELSTYLE LOAD <that job>`. Use one line per
quadrant, one due east, one that rounds across a minute, and one with a
1-digit degree.
