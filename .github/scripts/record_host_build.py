"""Record compiler provenance from the published host release run."""
import argparse
from datetime import datetime, timedelta
import json
from pathlib import Path
import re
import subprocess

REPO = "HakanSeven12/OpenCADStudio"
RELEASE_WORKFLOWS = ("release", "weekly release")
# How long before the tag exists a release run may have started. A scheduled
# Weekly release starts on main, then its prepare job commits and tags the
# release, so the run predates the tag by seconds; a wide margin costs nothing.
RUN_LEAD = timedelta(minutes=30)


def compiler_from_log(log):
    # Only standalone rustc output, not rustup's 'updated ... (from ...)' lines.
    versions = set(re.findall(r"\t(?:\d{4}-\S+ )?rustc (\d+\.\d+\.\d+ \([0-9a-f]+ \d{4}-\d{2}-\d{2}\))\s*$", log, re.M))
    if len(versions) != 1:
        raise ValueError(f"Expected one compiler across release platforms, found {sorted(versions)}")
    return "rustc " + versions.pop()


def gh(*args):
    return subprocess.check_output(["gh", *args], text=True, encoding="utf-8")


def parse_time(text):
    return datetime.fromisoformat(text.replace("Z", "+00:00"))


def windows_asset(release):
    assets = [a for a in release["assets"] if a["name"].endswith("-windows-x86_64-portable.exe")]
    if len(assets) != 1 or not re.fullmatch(r"sha256:[0-9a-f]{64}", assets[0].get("digest") or ""):
        raise ValueError("Expected one checksummed Windows portable release asset")
    return assets[0]


def select_release_run(runs, jobs_by_run, asset_uploaded_at):
    """Pick the run that published the Windows asset we are about to record.

    Runs are filed under the commit they STARTED on, which for a scheduled
    Weekly release is the parent of the release commit, and a retry may start
    on any later main commit — so the tagged commit identifies nothing. The
    workflow re-uploads every asset with --clobber on retries, so the bytes on
    the release come from whichever native build finished last. The only fact
    that ties a run to the published asset is that the asset's upload time
    falls inside that run's lifetime; require exactly one such run, and require
    its native jobs to have passed even when an unrelated job (the web bundle)
    sank the run's overall conclusion.
    """
    uploaded = parse_time(asset_uploaded_at)
    candidates = []
    for run in runs:
        if run["name"].lower() not in RELEASE_WORKFLOWS:
            continue
        if not parse_time(run["created_at"]) <= uploaded <= parse_time(run["updated_at"]):
            continue
        jobs = {j["name"]: j["conclusion"] for j in jobs_by_run[run["id"]]}
        for job in ("native / build-windows", "native / verify"):
            if jobs.get(job) != "success":
                raise ValueError(f"Release run {run['id']} uploaded the Windows asset but its '{job}' job "
                                 f"concluded {jobs.get(job)!r}; verify artifact provenance manually")
        candidates.append(run)
    if len(candidates) != 1:
        raise ValueError(f"Expected one release run to have uploaded the published Windows asset, found "
                         f"{[r['id'] for r in candidates]}; verify artifact provenance manually")
    return candidates[0]


def write_host_record(root, info):
    """Write both provenance files with LF endings on every platform; the
    release build byte-compares what it derives from them."""
    (root / "host-build.json").write_text(
        json.dumps(info, indent=2) + "\n", encoding="utf-8", newline="\n")
    channel = info["rustc_version"].split()[1]
    (root / "rust-toolchain.toml").write_text(
        '[toolchain]\nchannel = "' + channel + '"\nprofile = "minimal"\n',
        encoding="utf-8", newline="\n")


def record(tag):
    commit = json.loads(gh("api", f"repos/{REPO}/commits/{tag}"))["sha"]
    release = json.loads(gh("api", f"repos/{REPO}/releases/tags/{tag}"))
    asset = windows_asset(release)
    since = (parse_time(release["created_at"]) - RUN_LEAD).strftime("%Y-%m-%dT%H:%M:%SZ")
    # Ask each release workflow for its own runs: the repository-wide listing
    # is dominated by issue-triggered runs and a single page of it can miss a
    # week-old release run entirely.
    workflows = json.loads(gh("api", f"repos/{REPO}/actions/workflows?per_page=100"))["workflows"]
    runs = []
    for workflow in workflows:
        if workflow["name"].lower() in RELEASE_WORKFLOWS:
            runs += json.loads(gh("api", "-X", "GET", f"repos/{REPO}/actions/workflows/{workflow['id']}/runs",
                "-f", "branch=main", "-f", f"created=>={since}", "-f", "per_page=100"))["workflow_runs"]
    jobs_by_run = {run["id"]: json.loads(gh("api", f"repos/{REPO}/actions/runs/{run['id']}/jobs?per_page=100"))["jobs"]
                   for run in runs}
    run = select_release_run(runs, jobs_by_run, asset["updated_at"])["id"]
    compiler = compiler_from_log(gh("run", "view", str(run), "--repo", REPO, "--log"))
    root = Path(__file__).resolve().parents[2]
    info = dict(tag=tag, commit=commit, rustc_version=compiler, release_run=run,
        windows_asset=asset["name"], windows_sha256=asset["digest"].split(":")[1])
    write_host_record(root, info)


if __name__ == "__main__":
    p = argparse.ArgumentParser()
    p.add_argument("--tag", required=True)
    record(p.parse_args().tag)
