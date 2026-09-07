"""Generate release metadata from the locked graph and verified host build."""
import argparse
import hashlib
import json
from pathlib import Path
import subprocess
import tomllib

from repin_gates import current_host_pin, locked_package


def validate(root, rustc):
    host = json.loads((root / "host-build.json").read_text())
    if current_host_pin((root / "Cargo.toml").read_text()) != host["commit"]:
        raise ValueError("Host pin changed: verify the new published compiler and update host-build.json and rust-toolchain.toml before releasing")
    if rustc.split() != host["rustc_version"].split():
        raise ValueError(f"Compiler mismatch: expected {host['rustc_version']}, got {rustc}")
    toolchain = tomllib.loads((root / "rust-toolchain.toml").read_text())
    if toolchain["toolchain"]["channel"] != rustc.split()[1]:
        raise ValueError("rust-toolchain.toml disagrees with verified host compiler")
    return host


def generate(root, rustc):
    host = validate(root, rustc)
    source = locked_package((root / "Cargo.lock").read_text(), "acadrust")["source"]
    template = (root / "plugin.toml").read_text()
    # Metadata must belong to [opencad], regardless of later template sections.
    lines = ['rustc_version = ' + json.dumps(rustc),
             'acadrust_source = ' + json.dumps(source),
             'host_release = ' + json.dumps(host["tag"]),
             'host_commit = ' + json.dumps(host["commit"])]
    if any(key in tomllib.loads(template)["opencad"] for key in
           ("rustc_version", "acadrust_source", "host_release", "host_commit")):
        raise ValueError("Generated metadata must not be hardcoded in plugin.toml")
    return template.replace("[opencad]", "[opencad]\n" + "\n".join(lines), 1)


if __name__ == "__main__":
    parser = argparse.ArgumentParser()
    parser.add_argument("--output", type=Path)
    parser.add_argument("--build-info", type=Path)
    args = parser.parse_args()
    root = Path(__file__).resolve().parents[2]
    rustc = subprocess.check_output(["rustc", "--version"], text=True).strip()
    manifest = generate(root, rustc)
    if args.output:
        args.output.parent.mkdir(parents=True, exist_ok=True)
        args.output.write_text(manifest, encoding="utf-8", newline="\n")
    else:
        print("Host pin and compiler verified")
    if args.build_info:
        # These workflows build natively, without --target. Record that platform
        # separately because the host expects one shared plugin.toml per release.
        verbose = subprocess.check_output(["rustc", "-vV"], text=True)
        target = next(line.removeprefix("host: ") for line in verbose.splitlines()
                      if line.startswith("host: "))
        info = dict(tomllib.loads(manifest)["opencad"], target=target,
                    plugin_version=tomllib.loads(manifest)["plugin"]["version"],
                    cargo_lock_sha256=hashlib.sha256((root / "Cargo.lock").read_bytes()).hexdigest())
        args.build_info.parent.mkdir(parents=True, exist_ok=True)
        args.build_info.write_text(json.dumps(info, indent=2) + "\n", encoding="utf-8", newline="\n")
