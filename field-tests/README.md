# Open CAD Studio field tests

`test_no_app.py` is a black-box regression suite for the published Open CAD
Studio host. It stages the Windows plugin in an isolated temporary profile and
drives the host's `--serve` JSON interface. It does not use the user's normal
plugin directory and does not open the MN-metalica installation.

The suite covers:

- host/plugin handshake and `LS_HELLO`;
- Brazilian semicolon CSV, decimal comma, thousands separators, labels and
  feature-code layers;
- `LS_AUTOLABEL` off/on;
- inverse bearing DMS carry and the absence of `60"` seconds;
- angle-only resection outside the control triangle;
- explicit danger-circle rejection without geometry mutation;
- a known cut/fill volume;
- DWG save/reopen and `LANDSURVEY_POINT` XDATA persistence.

Run it with a staged package containing `plugin.toml` and
`opencad.landsurvey-windows-x86_64.dll`:

```powershell
python field-tests/test_no_app.py `
  --host C:\path\OpenCADStudio-v2026.36-windows-x86_64-portable.exe `
  --package dist `
  --host-record host-build.json `
  --output field-test-results
```

The expected result is `FIELD TESTS PASSED: 26 checks`. Logs and
`summary.json` are written under the selected output directory.
