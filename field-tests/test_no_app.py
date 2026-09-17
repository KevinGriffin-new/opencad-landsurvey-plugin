"""Black-box regression suite for the published Open CAD Studio host.

The test deliberately drives the real host in ``--serve`` mode.  It stages the
plugin in a temporary, isolated directory, so it cannot load another local
plugin installation or modify the user's normal Open CAD Studio profile.

Usage (from the repository root)::

    python field-tests/test_no_app.py --host path/to/OpenCADStudio.exe \
        --package dist --output field-test-results
"""

from __future__ import annotations

import argparse
import hashlib
import json
import os
from pathlib import Path
import shutil
import subprocess
import tempfile
from typing import Any, Iterable


ROOT = Path(__file__).resolve().parents[1]
FIXTURES = Path(__file__).resolve().parent / "fixtures"


class Checks:
    def __init__(self) -> None:
        self.total = 0
        self.passed = 0
        self.failed: list[str] = []

    def check(self, name: str, condition: bool, detail: str = "") -> None:
        self.total += 1
        if condition:
            self.passed += 1
            print(f"PASS {self.total:02d} {name}")
        else:
            message = f"{name}: {detail}" if detail else name
            self.failed.append(message)
            print(f"FAIL {self.total:02d} {message}")

    def finish(self) -> None:
        if self.failed:
            raise AssertionError(
                f"{self.passed} checks passed; {len(self.failed)} failed:\n"
                + "\n".join(f"- {item}" for item in self.failed)
            )
        print(f"FIELD TESTS PASSED: {self.passed} checks")


def json_replies(stdout: str) -> list[dict[str, Any]]:
    replies: list[dict[str, Any]] = []
    for line in stdout.splitlines():
        if line.startswith("{"):
            replies.append(json.loads(line))
    return replies


def reply(replies: Iterable[dict[str, Any]], predicate, name: str) -> dict[str, Any]:
    for item in replies:
        if predicate(item):
            return item
    raise AssertionError(f"missing reply: {name}; got {replies}")


def entities(replies: Iterable[dict[str, Any]]) -> dict[str, Any]:
    return reply(
        replies,
        lambda r: isinstance(r.get("entities"), list) and "by_type" not in r,
        "query",
    )


def entity_texts(query: dict[str, Any]) -> list[str]:
    return [
        str(e.get("value", ""))
        for e in query.get("entities", [])
        if e.get("type") == "Text"
    ]


def entity_counts(replies: Iterable[dict[str, Any]]) -> dict[str, Any]:
    return reply(
        replies,
        lambda r: "by_type" in r and r.get("total", 0) > 0,
        "entities summary",
    )


def run_host(
    host: Path,
    stage: Path,
    commands: list[dict[str, str]],
    log_dir: Path,
    label: str,
) -> tuple[list[dict[str, Any]], str, str]:
    env = dict(
        os.environ,
        OCS_PLUGINS_DIR=str(stage / "plugins"),
        APPDATA=str(stage / "config"),
        LOCALAPPDATA=str(stage / "local"),
    )
    env.pop("OCS_PLUGIN_MAX_API_VERSION", None)
    result = subprocess.run(
        [str(host.resolve()), "--serve"],
        cwd=stage,
        env=env,
        input="".join(json.dumps(command) + "\n" for command in commands),
        text=True,
        encoding="utf-8",
        errors="replace",
        capture_output=True,
        timeout=120,
        creationflags=getattr(subprocess, "CREATE_NO_WINDOW", 0),
    )
    (log_dir / f"{label}-stdout.log").write_text(result.stdout, encoding="utf-8")
    (log_dir / f"{label}-stderr.log").write_text(result.stderr, encoding="utf-8")
    if result.returncode:
        raise RuntimeError(
            f"{label}: host exited {result.returncode}\n"
            f"stdout:\n{result.stdout}\nstderr:\n{result.stderr}"
        )
    return json_replies(result.stdout), result.stdout, result.stderr


def stage_package(package: Path, stage: Path) -> None:
    plugin = stage / "plugins" / "opencad.landsurvey"
    plugin.mkdir(parents=True)
    for name in ("plugin.toml", "opencad.landsurvey-windows-x86_64.dll"):
        source = package / name
        if not source.is_file():
            raise FileNotFoundError(f"missing staged package file: {source}")
        shutil.copy2(source, plugin / name)
    for fixture in FIXTURES.glob("*.csv"):
        shutil.copy2(fixture, stage / fixture.name)


def copy_and_check_fixture(stage: Path, name: str) -> Path:
    path = stage / name
    if not path.is_file():
        raise FileNotFoundError(path)
    return path


def run_suite(host: Path, package: Path, output: Path, expected_host: Path | None) -> None:
    output.mkdir(parents=True, exist_ok=True)
    if expected_host:
        record = json.loads(expected_host.read_text(encoding="utf-8"))
        actual = hashlib.sha256(host.read_bytes()).hexdigest()
        if actual != record["windows_sha256"]:
            raise ValueError(
                f"host checksum mismatch: {actual} != {record['windows_sha256']}"
            )

    checks = Checks()
    with tempfile.TemporaryDirectory(prefix="ocs-land-survey-field-") as temp:
        stage = Path(temp)
        stage_package(package, stage)
        points_br = copy_and_check_fixture(stage, "pnezd-br.csv")
        points = copy_and_check_fixture(stage, "points-comma.csv")
        top = copy_and_check_fixture(stage, "top.csv")
        bottom = copy_and_check_fixture(stage, "bottom.csv")
        outside = copy_and_check_fixture(stage, "resection-outside.csv")
        danger = copy_and_check_fixture(stage, "resection-danger.csv")

        core, _, core_err = run_host(
            host,
            stage,
            [
                {"op": "new"},
                {"op": "run", "cmd": "LS_HELLO"},
                {"op": "run", "cmd": f"LS_PNEZD {points_br.name} preview"},
                {"op": "entities"},
                {"op": "run", "cmd": f"LS_PNEZD {points_br.name}"},
                {"op": "entities"},
                {"op": "layers"},
                {"op": "query"},
                {"op": "save", "path": "survey.dwg"},
            ],
            output,
            "01-core",
        )
        ready = reply(core, lambda r: r.get("ready") is True, "ready")
        checks.check("host ready banner", ready.get("version") == "2026.36")
        checks.check("plugin loaded by host", "Loaded plugin: Land Survey" in core_err, core_err)
        checks.check(
            "LS_HELLO recognized",
            any(r.get("cmd") == "LS_HELLO" and r.get("ok") for r in core),
        )
        preview_entities = reply(
            core,
            lambda r: "by_type" in r and r.get("total") == 0,
            "preview leaves drawing unchanged",
        )
        checks.check("PNEZD preview is non-mutating", preview_entities["total"] == 0)
        imported = reply(
            core,
            lambda r: r.get("cmd", "").startswith("LS_PNEZD")
            and "preview" not in r.get("cmd", "").lower(),
            "PNEZD import",
        )
        checks.check("Brazilian semicolon PNEZD imports 3 points + labels", imported.get("added") == 6, str(imported))
        summary = entity_counts(core)
        checks.check("PNEZD creates 3 Point entities", summary.get("by_type", {}).get("Point") == 3, str(summary))
        checks.check("PNEZD creates 3 Text labels", summary.get("by_type", {}).get("Text") == 3, str(summary))
        layers = reply(core, lambda r: "layers" in r, "layers")
        checks.check("feature-code layer is LS-PT-MARCO", "LS-PT-MARCO" in {x.get("name") for x in layers.get("layers", [])}, str(layers))
        query = entities(core)
        point_coords = [
            tuple(e.get("location", [])[:2])
            for e in query.get("entities", [])
            if e.get("type") == "Point"
        ]
        checks.check("Brazilian decimal values reach CAD coordinates", (333000.0, 7395000.0) in point_coords, str(point_coords))

        labels, _, _ = run_host(
            host,
            stage,
            [
                {"op": "new"},
                {"op": "run", "cmd": f"LS_AUTOLABEL OFF"},
                {"op": "run", "cmd": f"LS_PNEZD {points.name}"},
                {"op": "entities"},
                {"op": "run", "cmd": "LS_AUTOLABEL ON"},
                {"op": "run", "cmd": f"LS_PNEZD {points.name}"},
                {"op": "entities"},
            ],
            output,
            "02-label-toggle",
        )
        checks.check("LS_AUTOLABEL OFF recognized", any("LS_AUTOLABEL" in r.get("cmd", "") and r.get("ok") for r in labels))
        off_import = reply(labels, lambda r: r.get("cmd", "").startswith("LS_PNEZD"), "labels off import")
        checks.check("labels OFF imports only points", off_import.get("added") == 3, str(off_import))
        off_summary = entity_counts(labels)
        checks.check("labels OFF leaves Text count at zero", off_summary.get("by_type", {}).get("Text", 0) == 0, str(off_summary))
        on_summary = reply(labels[::-1], lambda r: "by_type" in r, "labels on summary")
        checks.check("labels ON restores point labels", on_summary.get("by_type", {}).get("Text") == 3, str(on_summary))

        inverse, _, _ = run_host(
            host,
            stage,
            [
                {"op": "new"},
                {"op": "run", "cmd": "LS_INVERSE 1000 1000 1086.602540378444 1050 draw"},
                {"op": "query"},
                {"op": "run", "cmd": "LS_INVERSE 0 0 0 -100 draw"},
                {"op": "query"},
            ],
            output,
            "03-inverse",
        )
        first_inverse = reply(inverse, lambda r: r.get("cmd", "").startswith("LS_INVERSE"), "inverse")
        checks.check("inverse draws line and two labels", first_inverse.get("added") == 3, str(first_inverse))
        inverse_query = entities(inverse)
        texts = entity_texts(inverse_query)
        checks.check("DMS rounds across minute boundary", any("30" in value and "00'00.00\"" in value for value in texts), str(texts))
        checks.check("DMS never emits 60 seconds", all("60\"" not in value for value in texts), str(texts))
        reverse_query = entities(inverse[::-1])
        checks.check("inverse quadrant formatting includes west bearing", any("W" in value for value in entity_texts(reverse_query)), str(entity_texts(reverse_query)))

        resect, _, _ = run_host(
            host,
            stage,
            [
                {"op": "new"},
                {"op": "run", "cmd": f"LS_RESECT {outside.name}"},
                {"op": "query"},
                {"op": "run", "cmd": f"LS_RESECT {danger.name}"},
                {"op": "query"},
            ],
            output,
            "04-resection",
        )
        outside_reply = reply(resect, lambda r: r.get("cmd", "").startswith("LS_RESECT"), "outside resection")
        checks.check("outside-triangle resection draws station and rays", outside_reply.get("added") == 8, str(outside_reply))
        outside_query = entities(resect)
        outside_texts = entity_texts(outside_query)
        checks.check("outside-triangle station is E500 N1300", any("STA E500.000 N1300.000" in value for value in outside_texts), str(outside_texts))
        ray_count = sum(
            1
            for item in outside_query.get("entities", [])
            if item.get("type") == "Line" and item.get("layer") == "LS-RESECT-RAYS"
        )
        checks.check("outside-triangle geometry has 3 rays", ray_count == 3, str(outside_query))
        danger_reply = reply(resect, lambda r: r.get("ok") is False, "danger-circle rejection")
        checks.check("danger circle is rejected explicitly", "danger circle" in danger_reply.get("error", "").lower(), str(danger_reply))
        after_danger = entities(resect)
        checks.check("danger rejection does not add geometry", after_danger.get("count") == outside_query.get("count"), str(after_danger))

        volume, _, _ = run_host(
            host,
            stage,
            [
                {"op": "new"},
                {"op": "run", "cmd": f"LS_VOLUME {top.name} {bottom.name} draw"},
                {"op": "query"},
                {"op": "save", "path": "survey-volume.dwg"},
            ],
            output,
            "05-volume",
        )
        volume_query = entities(volume)
        volume_texts = entity_texts(volume_query)
        checks.check("known 10x10x2 volume is drawn", any("CUT 200.00" in value and "NET 200.00" in value for value in volume_texts), str(volume_texts))
        checks.check("save creates the DWG artifact", (stage / "survey-volume.dwg").is_file())

        reopened, _, _ = run_host(
            host,
            stage,
            [{"op": "open", "path": "survey.dwg"}, {"op": "entities"}, {"op": "save", "path": "roundtrip.dxf"}],
            output,
            "06-roundtrip",
        )
        reopened_summary = entity_counts(reopened)
        checks.check("DWG reopen restores imported points", reopened_summary.get("by_type", {}).get("Point") == 3, str(reopened_summary))
        roundtrip = stage / "roundtrip.dxf"
        checks.check("LANDSURVEY_POINT survives save/reopen", roundtrip.is_file() and "LANDSURVEY_POINT" in roundtrip.read_text(errors="replace"))

    checks.finish()
    (output / "summary.json").write_text(json.dumps({"checks": checks.passed, "status": "passed"}, indent=2) + "\n", encoding="utf-8")


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--host", type=Path, required=True)
    parser.add_argument("--package", type=Path, default=Path("dist"))
    parser.add_argument("--output", type=Path, default=Path("field-test-results"))
    parser.add_argument("--host-record", type=Path, help="Optional host-build.json for SHA-256 verification")
    args = parser.parse_args()
    run_suite(args.host, args.package, args.output, args.host_record)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
