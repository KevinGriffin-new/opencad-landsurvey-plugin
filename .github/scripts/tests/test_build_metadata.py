import json
from pathlib import Path
import tomllib

import pytest
from build_metadata import generate, validate
from record_host_build import compiler_from_log, select_release_run, write_host_record
from windows_smoke import check_replies
import repin_gates as gates

ROOT = Path(__file__).resolve().parents[3]


def test_2026_36_dependency_override_and_api():
    fixture = Path(__file__).parent / "fixtures" / "host" / "v2026.36"
    read = lambda name: (fixture / name).read_text(encoding="utf-8")
    plan = gates.host_acadrust_plan(read("ocs_plugin_api-Cargo.toml"), read("Cargo.toml"), read("Cargo.lock"))
    assert plan["locked_rev"] == "5b56571a190e7a17c8f12d36390d2938b0fb72f7"
    assert plan["rev"] == "c91a1c1de89a0fbc006dc360ddb0251ac8c207d1"
    result = gates.gate_api_version(read("manifest.rs"), (ROOT / "plugin.toml").read_text(), "v2026.36")
    assert result["target"] == 5
    report = gates.gate_acadrust(read("Cargo.lock"), (ROOT / "Cargo.toml").read_text(),
                                (ROOT / "Cargo.lock").read_text(), "v2026.36")
    assert report["host_version"] == "0.5.4"


def test_generated_manifest_uses_resolved_source():
    host = json.loads((ROOT / "host-build.json").read_text())
    result = tomllib.loads(generate(ROOT, host["rustc_version"]))
    source = next(p["source"] for p in tomllib.loads((ROOT / "Cargo.lock").read_text())["package"] if p["name"] == "acadrust")
    assert result["opencad"]["acadrust_source"] == source
    assert result["opencad"]["rustc_version"] == host["rustc_version"]


def test_wrong_compiler_rejected():
    with pytest.raises(ValueError, match="Compiler mismatch"):
        validate(ROOT, "rustc 0.0.0 (bad 2000-01-01)")


def test_stale_host_record_rejected(tmp_path):
    for name in ("host-build.json", "Cargo.toml", "rust-toolchain.toml"):
        (tmp_path / name).write_text((ROOT / name).read_text())
    host = json.loads((tmp_path / "host-build.json").read_text())
    host["commit"] = "0" * 40
    (tmp_path / "host-build.json").write_text(json.dumps(host))
    with pytest.raises(ValueError, match="Host pin changed"):
        validate(tmp_path, host["rustc_version"])


def test_release_log_rejects_mixed_compilers():
    log = "windows\tstep\t2026-08-27T16:59:40.8233967Z rustc 1.98.0 (88d9e12ae 2026-08-18)\n"
    assert compiler_from_log(log) == "rustc 1.98.0 (88d9e12ae 2026-08-18)"
    with pytest.raises(ValueError):
        compiler_from_log(log + log.replace("1.98.0", "1.97.0"))


def test_smoke_rejects_plugin_that_did_not_load():
    with pytest.raises(AssertionError):
        check_replies(json.dumps({"cmd": "LS_PNEZD points.csv", "added": 0}))


def test_host_record_is_written_with_lf_endings(tmp_path):
    host = json.loads((ROOT / "host-build.json").read_text())
    write_host_record(tmp_path, host)
    for name in ("host-build.json", "rust-toolchain.toml"):
        raw = (tmp_path / name).read_bytes()
        assert b"\r" not in raw, f"{name} carries CR line endings"
    assert json.loads((tmp_path / "host-build.json").read_text()) == host
    toolchain = tomllib.loads((tmp_path / "rust-toolchain.toml").read_text())
    assert toolchain["toolchain"]["channel"] == host["rustc_version"].split()[1]


def _run(id, name, created, updated):
    return {"id": id, "name": name, "created_at": created, "updated_at": updated}


def _jobs(**conclusions):
    return [{"name": k.replace("__", " / ").replace("_", "-"), "conclusion": v} for k, v in conclusions.items()]


def test_release_run_is_the_one_that_uploaded_the_asset():
    # v2026.36: a manual dispatch tagged the release, then the scheduled run
    # rebuilt on the already-released commit and re-uploaded with --clobber.
    # The published bytes came from the second run.
    runs = [_run(34033142114, "Weekly release", "2026-09-06T12:27:17Z", "2026-09-06T12:52:00Z"),
            _run(34041622155, "Weekly release", "2026-09-06T15:13:22Z", "2026-09-06T15:38:00Z"),
            _run(34045281214, "Welcome new issues", "2026-09-06T15:13:22Z", "2026-09-06T15:38:00Z")]
    ok = _jobs(native__build_windows="success", native__verify="success")
    jobs = {34033142114: ok, 34041622155: ok}
    assert select_release_run(runs, jobs, "2026-09-06T15:35:51Z")["id"] == 34041622155


def test_release_run_found_when_scheduled_run_predates_its_own_tag():
    # v2026.37: one scheduled run started on the parent commit, created the
    # release commit + tag, published every native asset, then failed only on
    # the web bundle. No run is filed under the tagged commit at all.
    runs = [_run(34767008500, "Weekly release", "2026-09-13T15:54:50Z", "2026-09-13T16:14:13Z")]
    jobs = {34767008500: _jobs(native__build_windows="success", native__verify="success", web__build="failure")}
    assert select_release_run(runs, jobs, "2026-09-13T16:13:52Z")["id"] == 34767008500


def test_release_run_rejected_when_native_jobs_failed():
    runs = [_run(1, "Weekly release", "2026-09-13T15:54:50Z", "2026-09-13T16:14:13Z")]
    jobs = {1: _jobs(native__build_windows="success", native__verify="failure")}
    with pytest.raises(ValueError, match="native / verify"):
        select_release_run(runs, jobs, "2026-09-13T16:13:52Z")


def test_release_run_missing_when_no_run_spans_the_upload():
    runs = [_run(1, "Weekly release", "2026-09-13T15:54:50Z", "2026-09-13T16:14:13Z")]
    jobs = {1: _jobs(native__build_windows="success", native__verify="success")}
    with pytest.raises(ValueError, match=r"found \[\]"):
        select_release_run(runs, jobs, "2026-09-13T17:00:00Z")
