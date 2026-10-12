# Building for a published host

The plugin uses the exact compiler recorded in `rust-toolchain.toml`, not
floating stable. `host-build.json` records the matching host commit, release
tag, successful release run, full compiler version, and Windows asset checksum.
For v2026.41, release run 38160100031 reports Rust 1.99.0
(`b940084d7 2026-09-28`). Plugin 0.4.20 targets API 7 and the host's
opencadcodec 0.6.0 build (`8189f83`). Earlier: v2026.36 was Rust 1.98.1
(`48a229cea 2026-09-01`, run 34041622155), API 5 and acadrust 0.5.4.

The host compares the compiler string and the codec source hash exactly at
install and at load, so a plugin built for one host release is refused by any
other. Since v2026.39 the codec is the `opencadcodec` package (imported as
`codec`); the generated manifest still uses the host's key name
`acadrust_source` for it.

`build_metadata.py` rejects a stale host pin or mismatched compiler and generates
`dist/plugin.toml` with the resolved codec source from Cargo.lock, compiler
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
rewriting dependencies. It finds the release run that published the Windows
portable asset — the run whose lifetime spans the asset's upload time, since a
scheduled Weekly release starts on main *before* it commits and tags the
release, and a retry re-uploads with `--clobber` from whatever commit main is
on — requires that run's `native / build-windows` and `native / verify` jobs to
have passed (an unrelated job such as the web bundle may have failed), reads its
compiler output, checks for one unambiguous compiler version, records the
checksummed asset, and refreshes the toolchain. Missing logs, zero or several
candidate runs, failed native jobs, mixed compilers or missing asset checksums
stop the update; they never fall back to today's stable compiler. This
deliberately requires review when upstream changes its release format or
historical logs have expired.

The host record and toolchain travel with the dependency pin in the re-pin commit.
The current compatibility target is v2026.41, not whatever happens to be on Studio
main. Passing this test does not claim compatibility with later host releases.
