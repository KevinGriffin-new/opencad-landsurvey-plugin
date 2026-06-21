# Land Survey (`opencad.landsurvey`)

An external [Open CAD Studio](https://github.com/HakanSeven12/OpenCADStudio)
add-on: survey points, PNEZD & LandXML import, TIN surfaces, earthwork volumes,
COGO, and coordinate transforms. Domain math lives in the host-free `landsurvey`
engine crate (`crates/landsurvey/`), which is `std`/WASM-capable and has no CAD
dependency; a headless `landsurvey-cli` exercises the same engine and dumps DXF.

## Commands

| Command | Source | Description |
|---------|--------|-------------|
| `LS_PNEZD <path>` | ribbon file dialog | Import a PNEZD CSV (`P,N,E,Z,Desc`) as `Point` entities. World map: X=Easting, Y=Northing, Z=Elevation. |
| `LS_SURFACE <path>` | ribbon file dialog | Build a TIN from a PNEZD or **LandXML** surface, draw it on `LS-TIN-<NAME>`, and retain it by name for later commands. |
| `LS_VOLUME [<top> <bottom>] [grid] [draw]` | ribbon / command line | Earthwork cut/fill/net between two surfaces (by name — defaults to `top`/`bottom` — or file paths). Exact TIN overlay; optional grid method and drawn TINs + cut/fill line + label. |
| `LS_DATUM <surface> <elev>` | command line | Cut/fill of one surface vs a horizontal datum, with the datum contour and a label. |
| `LS_RTS <baseN> <baseE> <rot> <scale> [<toN> <toE>]` | command line | Rotate / Translate / Scale every entity about a base point. |
| `LS_HELMERT <pairs> [apply\|stages]` | ribbon file dialog / command line | Least-squares 2-D conformal fit from control pairs (`srcN,srcE,dstN,dstE`); prints a 7-step report. `stages` draws the annotated transform stages; `apply` transforms the drawing. |
| `LS_IMPORTPLAN <path>` | ribbon file dialog | Import recognized plan geometry (the `plan2cad` JSON) — lines / arcs / circles / texts onto their named layers. |
| `LS_INVERSE <N1> <E1> <N2> <E2>` | command line | Distance + azimuth + quadrant bearing between two coordinates. |
| `LS_LIST` | ribbon / command line | Count entities tagged with the `LANDSURVEY_POINT` record. |
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
