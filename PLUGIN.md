# Land Survey (`opencad.landsurvey`)

An external [Open CAD Studio](https://github.com/HakanSeven12/OpenCADStudio)
add-on: survey points, PNEZD & LandXML import, TIN surfaces, earthwork volumes,
COGO, and coordinate transforms. Domain math lives in the host-free `landsurvey`
engine crate (`crates/landsurvey/`), which is `std`/WASM-capable and has no CAD
dependency; a headless `landsurvey-cli` exercises the same engine and dumps DXF.

## Commands

| Command | Source | Description |
|---------|--------|-------------|
| `LS_PNEZD <path> [pnezd\|penzd] [detail] [preview]` | ribbon file dialog | Import a points file as labeled `Point` entities. Reads PNEZD or **PENZD** column order; delimiter (comma / tab / spaces) auto-detected; alphanumeric point names kept. Each point's number is labeled (`detail` adds an elevation + description line) and grouped onto a per-feature-code layer `LS-PT-<CODE>`. `preview` reports count / order / sample rows without drawing. World map: X=Easting, Y=Northing, Z=Elevation. |
| `LS_SURFACE <path>` | ribbon file dialog | Build a TIN from a PNEZD or **LandXML** surface, draw it on `LS-TIN-<NAME>`, and retain it by name for later commands. |
| `LS_LANDXML <path>` | ribbon file dialog | Import LandXML TIN surface(s) as **`Mesh`** entities on `LS-TIN-<NAME>` (retained by name for LS_VOLUME / LS_DATUM). `<P>` = `northing easting [elev]` → X=E/Y=N/Z=Z; units as-is; invisible `<F i="1">` faces skipped. Also handles the host's `LANDXMLIMPORT` verb so the Insert-tab button can dispatch LandXML import here (OpenCADStudio #157). |
| `LS_VOLUME [<top> <bottom>] [grid] [draw]` | ribbon / command line | Earthwork cut/fill/net between two surfaces (by name — defaults to `top`/`bottom` — or file paths). Exact TIN overlay; optional grid method and drawn TINs + cut/fill line + label. |
| `LS_DATUM <surface> <elev>` | command line | Cut/fill of one surface vs a horizontal datum, with the datum contour and a label. |
| `LS_RTS <baseN> <baseE> <rot> <scale> [<toN> <toE>]` | command line | Rotate / Translate / Scale every entity about a base point. |
| `LS_HELMERT <pairs> [apply\|stages\|anim\|teach]` | ribbon file dialog / command line | Least-squares 2-D conformal fit from control pairs (`srcN,srcE,dstN,dstE`); prints a 7-step report. `stages` draws the annotated transform stages; `apply` transforms the drawing; `anim`/`teach` export an animated-SVG explainer (`teach` amplifies near-grid fits). |
| `LS_RESECT <shots>` | ribbon file dialog / command line | Free-station resection: solve the occupied (unknown) point from shots to known control (`knownN, knownE, direction_deg[, distance][, name]`). Combined (direction + distance, least-squares, EDM scale check) with ≥2 distance shots, else angle-only three-point (Tienstra, danger-circle guard) for 3 angle shots. Draws the station, rays to each known point, and a label on `LS-RESECT-STATION/KNOWN/RAYS/LABEL`. |
| `LS_IMPORTPLAN <path>` | ribbon file dialog | Import recognized plan geometry (the `plan2cad` JSON) — lines / arcs / circles / texts onto their named layers. |
| `LS_INVERSE <N1> <E1> <N2> <E2> [draw] [anim]` | command line | Distance + azimuth + quadrant bearing between two coordinates. `draw` draws the line with bearing/distance labels (midpoint, perpendicular, rotated to the line and flipped to stay readable) on `LS-INVERSE`; `anim` exports an animated-SVG explainer. |
| `LS_LIST` | ribbon / command line | List each Land Survey point — number, easting / northing / elevation, description — read back from the geometry + `LANDSURVEY_POINT` record. |
| `LS_AUTOLABEL [ON\|OFF]` | command line | Toggle whether `LS_PNEZD` draws point-number labels on import (default ON); no argument reports the current state. |
| `LS_HELLO` | command line | Print the available commands. |

The file-dialog commands fire `ModuleEvent::PluginFileDialog`, so the host opens
a native file picker and dispatches `"<command> <path>"` back with the path's
original case preserved.

## XDATA

Domain data is stored on entities as XDATA (registered APPIDs, so it
round-trips through DWG/DXF) rather than in a side database.

| Application | Attached to | Record values |
|-------------|-------------|---------------|
| `LANDSURVEY_POINT` | each imported PNEZD `Point` | `[String(point_number), String(description)]` |
| `LANDSURVEY_PLAN` | each entity from an imported plan | `[String(source_filename)]` |
| `LANDSURVEY_SURFACE` | TIN edges, cut/fill lines, result labels | `[String(surface_name), String(kind)]` (kind: `TIN` / `CUTFILL` / `CONTOUR` / `LABEL`) |

## Build & install

```
cargo build --release        # → target/release/{lib,}opencad_landsurvey_plugin.{so,dll,dylib}
cargo test -p landsurvey     # exercise the host-free engine
```

In OCS: **Plugin Manager → Add repository →** `KevinGriffin-new/opencad-landsurvey-plugin`,
pick a compatible release, **Install**, then restart. The **Land Survey** tab
appears. See the OpenCADStudio `docs/plugin-architecture.md` for the loading and
ABI model.
