# Land Survey (`opencad.landsurvey`)

An external [Open CAD Studio](https://github.com/HakanSeven12/OpenCADStudio)
add-on: survey points, PNEZD & LandXML import, TIN surfaces, earthwork volumes,
COGO, and coordinate transforms. Domain math lives in the host-free `landsurvey`
engine crate (`crates/landsurvey/`), which is `std`/WASM-capable and has no CAD
dependency; a headless `landsurvey-cli` exercises the same engine and dumps DXF.

## Commands

| Command | Source | Description |
|---------|--------|-------------|
| `LS_PNEZD <path> [pnezd\|penzd] [detail] [preview]` | ribbon file dialog | Import a points file as labeled `Point` entities. Reads PNEZD or **PENZD** column order; delimiter (comma / tab / semicolon / spaces) auto-detected, with CSV-style quoted fields and decimal commas in semicolon files; UTF-8, UTF-16 and Windows-1252 files and LF / CRLF / CR line endings all read; lines with non-numeric or non-finite (`NaN`, `inf`) coordinates are skipped and counted; alphanumeric point names kept. Each point's number is labeled (`detail` adds an elevation + description line) and grouped onto a per-feature-code layer `LS-PT-<CODE>`. `preview` reports count / order / sample rows without drawing. World map: X=Easting, Y=Northing, Z=Elevation. |
| `LS_SURFACE <path>` | ribbon file dialog | Build a TIN from a PNEZD or **LandXML** surface, draw it on `LS-TIN-<NAME>`, and retain it by name for later commands. |
| `LS_LANDXML <path> [points] [parcels] [features] [surfaces] [alignments] [preview]` | ribbon file dialog | Import LandXML. **`<CgPoints>`** become Land Survey points, the same as `LS_PNEZD`: `LS-PT-<CODE>` layers, `LANDSURVEY_POINT` XDATA and labels. `pntRef` points are resolved. **`<Parcels>`** become closed LWPolylines on `LS-PARCEL`, with arcs as bulges and a `LANDSURVEY_PARCEL [name, desc]` tag. Each lot is labelled on `LS-PARCEL-LABEL` with its name and area (m², or sq ft and acres) at the file's `<Center>`, or at the area centroid when there is none. Every lot's area is recomputed from its lines and arcs and checked against the file's `area`. **`<PlanFeatures>`** become LWPolylines on `LS-FEATURE`, tagged `LANDSURVEY_FEATURE`. **TIN `<Surface>`s** become `Mesh` entities on `LS-TIN-<NAME>`, retained by name for LS_VOLUME / LS_DATUM; invisible `<F i="1">` faces are skipped. **`<Alignments>`** draw as centrelines on `LS-ALIGN`, with clothoid spirals evaluated exactly and densified, and station ticks and labels on `LS-ALIGN-LABEL` at every geometry point and at a round interval. Their **profiles** draw as a 10:1 profile view below the plan: grid on `LS-PROF-GRID`, the PVI line on `LS-PROF-PVI`, finished grade with parabolic vertical curves on `LS-PROF-FG`, and ground profiles on `LS-PROF-EG`. Their **cross sections** draw as 5:1 section views in rows below that, on `LS-XSEC` / `LS-XSEC-EG` / `LS-XSEC-LABEL`. `<P>`/`<CgPoint>` text is `northing easting [elev]`, mapped to X=E, Y=N, Z=Z, and units are kept as-is. Unsymmetrical or circular vertical curves are drawn as symmetric parabolas, with a warning. Naming kinds imports only those; `preview` only reports. Also handles the host's `LANDXMLIMPORT` verb (OpenCADStudio #157). |
| `LS_LANDXMLOUT <path> [points] [parcels] [features] [surfaces] [alignments] [metric\|imperial]` | ribbon (prints usage) / command line | Export LandXML 1.2 from what is drawn now. **Points:** Land Survey points; the code is the description's first word. **Parcels:** tagged parcels, plus any closed LWPolyline on a layer containing PARCEL, LOT, PROPERTY or BOUNDARY, named `Parcel <n>`. Bulges become `<Curve>`s, the area is computed from the boundary, and `<Center>` is where the lot's name label now sits, or the area centroid. **Features:** tagged plan features. **Surfaces:** every named TIN. **Alignments:** each centreline, with lines, arcs and `<Spiral>`s with a computed PI; spirals off a tangent also get tangent lengths. Its profiles and cross sections are read back from their views, so edits to the drawn PVI line or sections export. Units default to those of the last `LS_LANDXML` import, else metric. |
| `LS_VOLUME [<top> <bottom>] [grid] [draw]` | ribbon / command line | Earthwork cut/fill/net between two surfaces (by name — defaults to `top`/`bottom` — or file paths). Exact TIN overlay; optional grid method and drawn TINs + cut/fill line + label. |
| `LS_DATUM <surface> <elev>` | command line | Cut/fill of one surface vs a horizontal datum, with the datum contour and a label. |
| `LS_RTS <baseN> <baseE> <rot> <scale> [<toN> <toE>]` | command line | Rotate / Translate / Scale every entity about a base point. |
| `LS_HELMERT <pairs> [apply\|stages\|anim\|teach]` | ribbon file dialog / command line | Least-squares 2-D conformal fit from control pairs (`srcN,srcE,dstN,dstE`); prints a 7-step report. `stages` draws the annotated transform stages; `apply` transforms the drawing; `anim`/`teach` export an animated-SVG explainer (`teach` amplifies near-grid fits). |
| `LS_RESECT <shots>` | ribbon file dialog / command line | Free-station resection: solve the occupied (unknown) point from shots to known control (`knownN, knownE, direction_deg[, distance][, name]`). Combined (direction + distance, least-squares, EDM scale check) with ≥2 distance shots, else angle-only three-point (Tienstra, danger-circle guard) for 3 angle shots. Draws the station, rays to each known point, and a label on `LS-RESECT-STATION/KNOWN/RAYS/LABEL`. |
| `LS_IMPORTPLAN <path>` | ribbon file dialog | Import recognized plan geometry (the `plan2cad` / `plat2json` JSON) — lines / arcs / circles / texts / polylines onto their named layers. Ordered chains (`polylines`, alias `plines`) become LWPolylines (closed when the last point repeats the first); a vertex may carry a bulge (`[x, y, bulge]`, `tan(sweep/4)`, CCW positive) making its segment an arc. Flattened `lines`/`arcs` entries that restate a chain's edges or arc segments are skipped as duplicates. |
| `LS_INVERSE <N1> <E1> <N2> <E2> [draw] [anim]` | command line | Distance + azimuth + quadrant bearing between two coordinates. `draw` draws the line on `LS-INVERSE` with bearing/distance labels (midpoint, perpendicular, rotated to the line and flipped to stay readable) in the current MicroSurvey label style — e.g. `N88°16'15"E` / `36.00` on `BEARING1` / `DISTANCE1`; `anim` exports an animated-SVG explainer. |
| `LS_LABELSTYLE [LOAD <config.json\|job.MSJ>] [BEARING <n>] [DISTANCE <n>] [SCALE <n>\|OFF] [METRIC\|IMPERIAL] [RESET]` | command line | Show or set the MicroSurvey (MsAnnotate) bearing/distance label styles: load the 10+10 styles from a MicroSurvey job's `config.json`, pick the current style by number, and set a plot scale (1:n) so text is sized from its leroy (60 = 0.06" on paper). Without a scale, labels are sized to the line. See `docs/msannotate-line-labels.md`. |
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
