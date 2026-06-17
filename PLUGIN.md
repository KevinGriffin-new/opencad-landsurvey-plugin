# Land Survey (`opencad.landsurvey`)

An external [Open CAD Studio](https://github.com/HakanSeven12/OpenCADStudio)
add-on: survey points, PNEZD import, recognized-plan import, and COGO geometry.
Domain math lives in the host-free `landsurvey` engine crate
(`crates/landsurvey/`), which is `std`/WASM-capable and has no CAD dependency.

## Commands

| Command | Source | Description |
|---------|--------|-------------|
| `LS_PNEZD <path>` | ribbon file dialog | Import a PNEZD CSV (`P,N,E,Z,Desc`) as `Point` entities. World map: X=Easting, Y=Northing, Z=Elevation. |
| `LS_IMPORTPLAN <path>` | ribbon file dialog | Import recognized plan geometry (the `plan2cad` JSON) — lines / arcs / circles / texts onto their named layers. |
| `LS_INVERSE <N1> <E1> <N2> <E2>` | command line | Distance + azimuth + quadrant bearing between two coordinates. |
| `LS_LIST` | ribbon / command line | Count entities tagged with the `LANDSURVEY_POINT` record. |
| `LS_HELLO` | command line | Print the available commands. |

The two import commands fire `ModuleEvent::PluginFileDialog`, so the host opens
a native file picker and dispatches `"<command> <path>"` back with the path's
original case preserved.

## XDATA

Domain data is stored on entities as XDATA (registered APPIDs, so it
round-trips through DWG/DXF) rather than in a side database.

| Application | Attached to | Record values |
|-------------|-------------|---------------|
| `LANDSURVEY_POINT` | each imported PNEZD `Point` | `[String(point_number), String(description)]` |
| `LANDSURVEY_PLAN` | each entity from an imported plan | `[String(source_filename)]` |

## Build & install

```
cargo build --release        # → target/release/{lib,}opencad_landsurvey_plugin.{so,dll,dylib}
cargo test -p landsurvey     # exercise the host-free engine
```

In OCS: **Plugin Manager → Add repository →** `KevinGriffin-new/opencad-landsurvey-plugin`,
pick a compatible release, **Install**, then restart. The **Land Survey** tab
appears. See the OpenCADStudio `docs/plugin-architecture.md` for the loading and
ABI model.
