# Building for a published host

The plugin uses the exact compiler recorded in `rust-toolchain.toml`, not
floating stable. `host-build.json` records the matching host commit, release
tag, successful release run, full compiler version, and Windows asset checksum.
For v2026.36, weekly release run 34041622155 reports Rust 1.98.1
(`48a229cea 2026-09-01`) on Windows, Linux and macOS. Plugin 0.4.17
targets API 5 and the host's acadrust 0.5.4 build.

`build_metadata.py` rejects a stale host pin or mismatched compiler and generates
`dist/plugin.toml` with the resolved acadrust source from Cargo.lock, compiler
version, host tag and host commit. The checked-in plugin.toml remains a template.
Each release binary also has a `.build.json` sidecar recording its native target
platform and lockfile checksum; the shared manifest stays identical across platforms.
Never install that template instead of the generated release manifest.

```
cargo build --release --locked
python .github/scripts/build_metadata.py --output dist/plugin.toml
```

CI and release builds use the repository toolchain and the committed lockfile.
The release collects all platform artifacts and requires matching manifests
and a passing Windows smoke test before uploading any release assets.

## Published Windows host test

The Windows CI job downloads the exact portable host named in host-build.json,
verifies its SHA-256, and uses temporary plugin and configuration directories.
It checks point import, inverse drawing, DWG save/reopen in a fresh host process,
survey XDATA in the resulting DXF, and a 10-by-10 pad raised 2 units (cut 200,
fill 0, net 200). Logs are uploaded even when assertions fail. Each host process
has a 120-second timeout. No installed Studio plugins or settings are modified.

After staging the DLL as `dist/opencad.landsurvey-windows-x86_64.dll`:

```
python .github/scripts/windows_smoke.py --host path/to/published-portable.exe
```

## Updating the host pin

The re-pin workflow runs `record_host_build.py --tag <release-tag>` after
rewriting dependencies. This reads the successful release run's compiler output,
checks for one unambiguous compiler version, records the checksummed Windows
asset, and refreshes the toolchain. Missing logs, ambiguous successful runs,
mixed compilers or missing asset checksums stop the update; they never fall back
to today's stable compiler. This deliberately requires review when upstream
changes its release format or historical logs have expired.

The host record and toolchain travel with the dependency pin in the re-pin commit.
The current compatibility target is v2026.36, not whatever happens to be on Studio
main. Passing this test does not claim compatibility with later host releases.
