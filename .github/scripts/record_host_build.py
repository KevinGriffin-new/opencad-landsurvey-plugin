"""Record compiler provenance from a successful published host release run."""
import argparse
import json
from pathlib import Path
import re
import subprocess

REPO = "HakanSeven12/OpenCADStudio"


def compiler_from_log(log):
    # Only standalone rustc output, not rustup's 'updated ... (from ...)' lines.
    versions = set(re.findall(r"\t(?:\d{4}-\S+ )?rustc (\d+\.\d+\.\d+ \([0-9a-f]+ \d{4}-\d{2}-\d{2}\))\s*$", log, re.M))
    if len(versions) != 1:
        raise ValueError(f"Expected one compiler across release platforms, found {sorted(versions)}")
    return "rustc " + versions.pop()


def gh(*args):
    return subprocess.check_output(["gh", *args], text=True, encoding="utf-8")


def record(tag):
    commit = json.loads(gh("api", f"repos/{REPO}/commits/{tag}"))["sha"]
    runs = json.loads(gh("run", "list", "--repo", REPO,
        "--commit", commit, "--json", "databaseId,conclusion,workflowName", "--limit", "100"))
    good = [r for r in runs if r["conclusion"] == "success"
            and r["workflowName"].lower() in ("release", "weekly release")]
    if len(good) != 1:
        raise ValueError("Release run is missing or ambiguous; verify artifact provenance manually")
    run = good[0]["databaseId"]
    compiler = compiler_from_log(gh("run", "view", str(run), "--repo", REPO, "--log"))
    release = json.loads(gh("release", "view", tag, "--repo", REPO, "--json", "assets"))
    assets = [a for a in release["assets"] if a["name"].endswith("-windows-x86_64-portable.exe")]
    if len(assets) != 1 or not re.fullmatch(r"sha256:[0-9a-f]{64}", assets[0].get("digest", "")):
        raise ValueError("Expected one checksummed Windows portable release asset")
    asset = assets[0]
    root = Path(__file__).resolve().parents[2]
    info = dict(tag=tag, commit=commit, rustc_version=compiler, release_run=run,
        windows_asset=asset["name"], windows_sha256=asset["digest"].split(":")[1])
    (root / "host-build.json").write_text(json.dumps(info, indent=2) + "\n")
    (root / "rust-toolchain.toml").write_text(
        '[toolchain]\nchannel = "' + compiler.split()[1] + '"\nprofile = "minimal"\n')


if __name__ == "__main__":
    p = argparse.ArgumentParser()
    p.add_argument("--tag", required=True)
    record(p.parse_args().tag)
