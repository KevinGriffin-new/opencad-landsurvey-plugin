# opencad-landsurvey-plugin

A **Land Survey** add-on for [Open CAD Studio](https://github.com/HakanSeven12/OpenCADStudio),
distributed as a prebuilt dynamic library (`cdylib`) via GitHub Releases.

It depends only on `ocs_plugin_api` (the host's stable contract crate) and
`acadrust` (the host's entity model) — never on the OpenCADStudio binary — and
exports the two C symbols the host loader expects (via
`ocs_plugin_api::export_plugin!`). It adds a **Land Survey** ribbon tab with
PNEZD import, recognized-plan import, a COGO inverse, and a point list.

See [`PLUGIN.md`](PLUGIN.md) for the command and XDATA reference.

## Layout

```
opencad-landsurvey-plugin/
  Cargo.toml            # workspace root: the cdylib package + acadrust patch
  plugin.toml           # shipped beside the binary; mirrors the in-code MANIFEST
  src/
    lib.rs              # manifest, BuiltinPlugin entry point, export_plugin!
    ribbon.rs           # the "Land Survey" CadModule tab
    dispatch.rs         # LS_* command routing → acadrust entities + XDATA
    state.rs            # per-tab plugin state
  crates/
    landsurvey/         # Layer-C engine: COGO + PNEZD + plan parsing (std + serde)
```

The split mirrors the three-layer model in OpenCADStudio's
`docs/plugin-architecture.md`: the host (Layer A) loads this cdylib (Layer B),
which calls the host-free engine crate (Layer C). The engine carries the unit
tests and can be reused from a CLI or WASM build.

## Releases

Pushing a `v*` tag runs `.github/workflows/release.yml`, which builds the cdylib
on Linux/Windows/macOS and uploads each binary plus `plugin.toml` as release
assets. The host picks the asset matching the user's OS/arch and reads
`plugin.toml` for the API-version compatibility check.

> Approach B: the binary must be built against the same toolchain and
> `ocs_plugin_api` / `acadrust` versions as the host. The
> `ocs_plugin_api_version` symbol gates the API version at load time.

## License

GPL-3.0-only, matching Open CAD Studio.
