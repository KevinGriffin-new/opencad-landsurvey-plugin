"""Exercise the published Windows host with an isolated plugin installation."""
import argparse
import hashlib
import json
import os
from pathlib import Path
import shutil
import subprocess
import tempfile


def check_replies(stdout):
    replies = [json.loads(line) for line in stdout.splitlines() if line.startswith("{")]
    assert replies and all(r.get("ok") is True for r in replies), replies
    imported = next(r for r in replies if r.get("cmd", "").startswith("LS_PNEZD"))
    assert imported.get("added") == 6, imported
    entities = next(r for r in replies if r.get("total", 0) > 0 and "by_type" in r)
    assert entities["by_type"].get("Point") == 3, entities
    assert entities["by_type"].get("Text") == 3, entities
    inverse = next(r for r in replies if r.get("cmd", "").startswith("LS_INVERSE"))
    assert inverse.get("added") == 3, inverse


def run(exe, package, output, host_record=None):
    root = Path(__file__).resolve().parents[2]
    host = json.loads((host_record or root / "host-build.json").read_text())
    if hashlib.sha256(exe.read_bytes()).hexdigest() != host["windows_sha256"]:
        raise ValueError("Published Windows host checksum mismatch")
    output.mkdir(parents=True, exist_ok=True)
    with tempfile.TemporaryDirectory(prefix="ocs-smoke-") as tmp:
        stage = Path(tmp)
        plugin = stage / "plugins" / "opencad.landsurvey"
        plugin.mkdir(parents=True)
        shutil.copy2(package / "plugin.toml", plugin)
        shutil.copy2(package / "opencad.landsurvey-windows-x86_64.dll", plugin)
        (stage / "points.csv").write_text("1,5000,5000,100,IPF\n2,5100,5050,101.25,IPF\n3,5200,4980,99.8,MON\n")
        for name, z in (("top", 2), ("bottom", 0)):
            (stage / f"{name}.csv").write_text(
                f"1,0,0,{z},PAD\n2,0,10,{z},PAD\n3,10,0,{z},PAD\n4,10,10,{z},PAD\n")
        commands = [{"op": "new"}, {"op": "run", "cmd": "LS_PNEZD points.csv"},
                    {"op": "entities"},
                    {"op": "run", "cmd": "LS_INVERSE 5000 5000 5100 5050 draw"},
                    {"op": "save", "path": "survey.dwg"},
                    {"op": "new"},
                    {"op": "run", "cmd": "LS_VOLUME top.csv bottom.csv draw"},
                    {"op": "save", "path": "volume.dxf"}]
        env = dict(os.environ, OCS_PLUGINS_DIR=str(stage / "plugins"),
                   APPDATA=str(stage / "config"), LOCALAPPDATA=str(stage / "local"))
        env.pop("OCS_PLUGIN_MAX_API_VERSION", None)
        result = subprocess.run([str(exe.resolve()), "--serve"], cwd=stage, env=env,
            input="".join(json.dumps(c) + "\n" for c in commands),
            text=True, encoding="utf-8", errors="replace", capture_output=True,
            timeout=120, creationflags=getattr(subprocess, "CREATE_NO_WINDOW", 0))
        (output / "stdout.log").write_text(result.stdout, encoding="utf-8")
        (output / "stderr.log").write_text(result.stderr, encoding="utf-8")
        result.check_returncode()
        check_replies(result.stdout)
        # Close the writing host before reopening: NEW leaves its old drawing
        # tab (and Windows file lock) alive. A fresh process also tests persistence
        # without depending on the plugin's in-memory metadata cache.
        reopen = [{"op": "open", "path": "survey.dwg"}, {"op": "entities"},
                  {"op": "save", "path": "roundtrip.dxf"}]
        restored = subprocess.run([str(exe.resolve()), "--serve"], cwd=stage, env=env,
            input="".join(json.dumps(c) + "\n" for c in reopen),
            text=True, encoding="utf-8", errors="replace", capture_output=True,
            timeout=120, creationflags=getattr(subprocess, "CREATE_NO_WINDOW", 0))
        (output / "reopen-stdout.log").write_text(restored.stdout, encoding="utf-8")
        (output / "reopen-stderr.log").write_text(restored.stderr, encoding="utf-8")
        restored.check_returncode()
        replies = [json.loads(line) for line in restored.stdout.splitlines() if line.startswith("{")]
        assert replies and all(r.get("ok") is True for r in replies), replies
        assert any(r.get("by_type", {}).get("Point") == 3 for r in replies), "Survey points did not survive save/reopen"
        drawing = (stage / "roundtrip.dxf").read_text(errors="replace")
        assert "LANDSURVEY_POINT" in drawing and "IPF" in drawing and "MON" in drawing
        volume = (stage / "volume.dxf").read_text(errors="replace")
        assert "CUT 200.00  FILL 0.00  NET 200.00" in volume, "Known pad volume is incorrect"
        print("Published Windows host: import, inverse, DWG metadata roundtrip and known volume passed")


if __name__ == "__main__":
    p = argparse.ArgumentParser()
    p.add_argument("--host", type=Path, required=True)
    p.add_argument("--package", type=Path, default=Path("dist"))
    p.add_argument("--output", type=Path, default=Path(".compat-host/logs"))
    p.add_argument("--host-record", type=Path, help="Checksummed release record for testing another host version")
    a = p.parse_args()
    run(a.host, a.package, a.output, a.host_record)
