#!/usr/bin/env python3
"""Parsers and gates for the nightly upstream re-pin.

This logic lives in a file rather than in a heredoc inside repin.yml so that it
can be imported, run locally, and unit-tested. Every previous break of these
gates was an upstream respelling that only a red nightly run could find:

    <=v0.8.2  acadrust = "0.4"        + [patch] git = ".../acadrust", branch = "main"
      v0.8.7  (same)                  + [patch] moved to .../acadifc
      v0.8.8  acadrust = { version = "0.4", features = [...] }, [patch] rev-pinned,
              URL gained a ".git" suffix
      v0.9.5  acadrust = { git = ".../cadcodec.git", rev = "...", features = [...] }
              and no [patch] at all
      v0.9.8  ocs_plugin_api still declares the same shape, but the host ROOT
              adds [patch."https://github.com/.../cadcodec.git"] redirecting it
              to "https://git@github.com/.../cadcodec.git" at a different rev

Each of those tags is a fixture under tests/fixtures/host/, asserted in
tests/test_repin_gates.py. The rule that stops the cycle: a shape that breaks a
gate gets its tag added to the corpus in the same PR as the fix.

Design rule: derive facts from Cargo.lock, not from Cargo.toml, wherever a
choice exists. The lockfile format is cargo's and is stable; manifest spelling
is the upstream author's and has changed four times in six weeks. Every gate
below that reads a version, a URL or a rev reads it from a lockfile.
"""

from __future__ import annotations

import argparse
import os
import re
import sys
import tomllib

# Exit codes. The distinction matters: ESCALATE means a human has a decision to
# make, UNPARSEABLE means this file is out of date. They were indistinguishable
# before, so every failure looked like the same six-item checklist to a reader
# at breakfast.
OK, ESCALATE, UNPARSEABLE = 0, 1, 2


class Escalate(Exception):
    """A human has to decide something; the re-pin cannot proceed mechanically."""


class Unparseable(Exception):
    """A gate could not evaluate its input — the gate itself needs updating."""


# ----------------------------------------------------------------- lockfiles


def locked_package(lock_text: str, name: str) -> dict:
    """The single [[package]] stanza named `name`.

    More than one stanza means two builds of the crate coexist in the graph.
    For acadrust that is precisely the ABI failure this workflow exists to
    prevent, so it is an escalation rather than a "take the first" guess.
    """
    doc = tomllib.loads(lock_text)
    found = [p for p in doc.get("package", []) if p.get("name") == name]
    if not found:
        raise Unparseable(f"no [[package]] named {name!r} in the lockfile")
    if len(found) > 1:
        detail = ", ".join(sorted(f"{p.get('version')} ({p.get('source')})" for p in found))
        raise Escalate(
            f"{name} resolves to {len(found)} different builds: {detail}. The "
            "plugin and the host must share exactly one, or the types crossing "
            "the plugin boundary are not the same types."
        )
    return found[0]


GIT_SOURCE = re.compile(r"git\+(?P<url>[^?#]+)(?:\?[^#]*)?#(?P<rev>[0-9a-f]{40})")


def git_source(pkg: dict) -> tuple[str, str]:
    """(url, 40-hex rev) from a locked package's git `source`."""
    src = pkg.get("source")
    name = pkg.get("name", "?")
    if src is None:
        raise Unparseable(f"{name} has no `source` in the lockfile (a path dependency?)")
    if src.startswith("registry+"):
        raise Escalate(
            f"{name} now resolves from a registry, not git: {src}. Pinning a "
            "published version is a different mechanical path than pinning a "
            "rev, and choosing it is a decision."
        )
    m = GIT_SOURCE.fullmatch(src)
    if not m:
        raise Unparseable(f"unparseable git source for {name}: {src}")
    return m.group("url"), m.group("rev")


def slug(url: str) -> str:
    """owner/repo, so a scheme change or a bare `.git` is not read as a move."""
    return re.sub(r"\.git$", "", re.sub(r"^[a-z+]+://", "", url))


def linked_acadrust(plugin_api_manifest: str) -> tuple[str, str]:
    """acadrust exactly as ocs_plugin_api declares it, url and rev string.

    Verbatim, because cargo's identity for a git source includes the literal
    `?rev=` text. `rev = "931c4ab"` and `rev = "931c4ab0c590...b139f"` resolve to
    the same commit and are still two different sources, so a graph carrying
    both compiles two acadrusts whose types do not interchange. We link
    ocs_plugin_api, so its spelling is the one we have to match — not the host
    root manifest's, and not the lockfile's expanded sha.
    """
    dep = declared_dep(plugin_api_manifest, "acadrust")
    if dep is None:
        raise Unparseable("no [dependencies].acadrust in ocs_plugin_api's manifest")
    kind, value = dep_kind(dep)
    if kind != "git":
        raise Escalate(
            f"ocs_plugin_api declares acadrust as {value!r}, not a git pin. "
            "Matching it needs a different mechanism than copying a rev."
        )
    url, rev = value  # type: ignore[misc]
    return url, rev


# ----------------------------------------------------------------- manifests


def declared_dep(manifest_text: str, name: str, table: str = "dependencies"):
    """The raw `[table].name` value from a manifest, or None."""
    return tomllib.loads(manifest_text).get(table, {}).get(name)


def dep_kind(dep, name: str = "acadrust") -> tuple[str, object]:
    """Classify a dependency as ("version", req) or ("git", (url, rev)).

    Handles every shape upstream has used: a bare string, a table carrying
    `version`, and a table carrying `git` + `rev`.
    """
    if isinstance(dep, str):
        return "version", dep
    if not isinstance(dep, dict):
        raise Unparseable(f"cannot classify {name} dependency: {dep!r}")
    if "git" in dep:
        rev = dep.get("rev")
        if rev is None:
            raise Escalate(
                f"{name} is pinned to a branch or tag, not a rev: {dep!r}. A "
                "moving pin ships a different binary on every rebuild."
            )
        return "git", (dep["git"], rev)
    if "version" in dep:
        return "version", dep["version"]
    raise Unparseable(f"cannot classify {name} dependency: {dep!r}")


# ------------------------------------------------------------------ patches


def patch_entry(manifest_text: str, name: str = "acadrust"):
    """The single `[patch."<url>"]` entry redirecting `name`, or None.

    Host v0.9.8 added one. ocs_plugin_api still declares acadrust at
    `https://github.com/HakanSeven12/cadcodec.git` rev `5b2ae66`; the host root
    redirects that source to `https://git@github.com/HakanSeven12/cadcodec.git`
    at rev `788eea0`. Cargo counts the `git@` userinfo as part of a source's
    identity — which is exactly what makes the redirect legal, since a patch has
    to point somewhere else — so the host ships an acadrust that neither its own
    `[dependencies]` nor ocs_plugin_api's names anywhere.

    A `[patch]` table applies only from the workspace root, and the plugin is its
    own root. Copying ocs_plugin_api's spelling therefore reproduces the source
    upstream patched *away from*: the pin looks right, every spelling gate
    passes, and the lockfile resolves a different commit than the host runs.
    Mirroring the redirect is the only way to land on the host's build.
    """
    patches = tomllib.loads(manifest_text).get("patch", {})
    found = [(key, table[name]) for key, table in patches.items() if name in table]
    if not found:
        return None
    if len(found) > 1:
        keys = ", ".join(sorted(k for k, _ in found))
        raise Escalate(
            f"the host patches {name} from more than one source ({keys}). Which "
            "redirect to mirror is a decision, not a copy."
        )
    key, dep = found[0]
    kind, value = dep_kind(dep, name)
    if kind != "git":
        raise Escalate(
            f"the host patches {name} to a published version ({value!r}) rather "
            "than a git rev. Reproducing that is a source change, not a re-pin."
        )
    url, rev = value  # type: ignore[misc]
    return key, url, rev


def show_patch(patch) -> str:
    """A patch entry as one readable line, for gate messages."""
    if patch is None:
        return "no [patch] for acadrust"
    key, url, rev = patch
    return f'[patch."{key}"] -> "{url}" rev "{rev}"'


def host_acadrust_plan(plugin_api_manifest: str, host_manifest: str,
                       host_lock: str) -> dict:
    """How to reproduce the acadrust build the host actually ships.

    Two manifest facts decide it: the spelling ocs_plugin_api declares, and the
    `[patch]` redirect the host root applies on top. This works out both, then
    checks that they land on the source the host's LOCKFILE records. The lockfile
    is ground truth — it is the only file that says which acadrust the shipped
    binary contains — so a plan that does not reproduce it means this file's
    model of the host is incomplete, and every gate downstream would be reasoning
    about the wrong source. That is `Unparseable`, not a decision: it is a gap
    here, and canary.yml runs this against upstream `main` to find it early.
    """
    declared_url, declared_rev = linked_acadrust(plugin_api_manifest)
    locked_url, locked_rev = git_source(locked_package(host_lock, "acadrust"))
    patch = patch_entry(host_manifest)

    plan = {"url": declared_url, "rev": declared_rev,
            "patch_key": "", "patch_url": "", "patch_rev": "",
            "locked_url": locked_url, "locked_rev": locked_rev}

    if patch is None:
        if slug(locked_url) != slug(declared_url) or not locked_rev.startswith(declared_rev):
            raise Unparseable(
                f"ocs_plugin_api declares acadrust {slug(declared_url)}@{declared_rev} "
                f"but the host locks {slug(locked_url)}@{locked_rev[:7]}, and the host "
                "root carries no [patch] to explain the difference. Something else "
                "is redirecting the source."
            )
        return plan

    key, patch_url, patch_rev = patch
    if slug(key) != slug(declared_url):
        raise Escalate(
            f'the host patches "{key}", but ocs_plugin_api pulls acadrust from '
            f'"{declared_url}". Mirroring that patch would leave ocs_plugin_api\'s '
            "acadrust unredirected and the graph would carry two of them."
        )
    if slug(patch_url) != slug(locked_url) or not locked_rev.startswith(patch_rev):
        raise Unparseable(
            f"the host patches acadrust to {slug(patch_url)}@{patch_rev} but locks "
            f"{slug(locked_url)}@{locked_rev[:7]}. The patch does not explain the "
            "lockfile, so something else is redirecting the source."
        )
    plan |= {"patch_key": key, "patch_url": patch_url, "patch_rev": patch_rev}
    return plan


def effective_acadrust(manifest_text: str) -> tuple[str, str]:
    """The acadrust source a manifest resolves to, its own `[patch]` applied."""
    dep = declared_dep(manifest_text, "acadrust")
    if dep is None:
        raise Unparseable("no [dependencies].acadrust in the manifest")
    kind, value = dep_kind(dep)
    if kind != "git":
        raise Unparseable("an effective git source is only defined for a git pin")
    url, rev = value  # type: ignore[misc]
    patch = patch_entry(manifest_text)
    if patch is not None and slug(patch[0]) == slug(url):
        return patch[1], patch[2]
    return url, rev


SEMVER = re.compile(r"(\d+)\.(\d+)\.(\d+)")


def parse_semver(version: str) -> tuple[int, int, int]:
    m = SEMVER.match(version.strip())
    if not m:
        raise Unparseable(f"unparseable version: {version!r}")
    major, minor, patch = (int(x) for x in m.groups())
    return major, minor, patch


def compat_series(version: str) -> tuple[int, ...]:
    """The part of a version cargo will not cross on its own.

    0.4.1 and 0.4.7 are the same series; 0.4.1 and 0.5.0 are not.
    """
    major, minor, _ = parse_semver(version)
    return (0, minor) if major == 0 else (major,)


def caret_matches(req: str, version: str) -> bool:
    """Cargo's default (caret) semantics for a bare requirement like "0.4".

    Only bare requirements are understood. An explicit operator (=, ~, >=, *)
    means someone made a deliberate choice this function would misread, so it
    raises rather than guessing.
    """
    req = req.strip()
    if not re.fullmatch(r"\d+(\.\d+){0,2}", req):
        raise Unparseable(f"only bare caret requirements are understood, got {req!r}")
    parts = [int(x) for x in req.split(".")]
    lower = parts + [0] * (3 - len(parts))
    ver = list(parse_semver(version))
    if ver < lower:
        return False
    # The upper bound bumps the leftmost non-zero component as written:
    # ^0.4 is <0.5.0, ^1.2 is <2.0.0, ^0.0.3 is <0.0.4.
    idx = next((i for i, x in enumerate(parts) if x != 0), len(parts) - 1)
    upper = lower[:idx] + [lower[idx] + 1] + [0] * (2 - idx)
    return ver < upper


def api_version_window(rs_text: str) -> tuple[int, int]:
    """(min supported, current) from ocs_plugin_api's manifest.rs."""

    def const(name: str) -> int:
        m = re.search(rf"\b{name}: u32 = (\d+)", rs_text)
        if not m:
            raise Unparseable(f"{name} not found in manifest.rs")
        return int(m.group(1))

    return const("API_VERSION_MIN_SUPPORTED"), const("API_VERSION")


# --------------------------------------------------------------------- gates


def gate_acadrust(host_lock: str, our_manifest: str, our_lock: str | None = None,
                  tag: str = "upstream") -> dict:
    """The host's acadrust must stay in the series the plugin is built for.

    Reads the host's acadrust from its lockfile, never from its manifest: the
    manifest has stated the dependency four different ways and currently states
    no version at all, while the lockfile has always carried the resolved
    version, URL and rev in the same place.
    """
    host = locked_package(host_lock, "acadrust")
    host_url, host_rev = git_source(host)
    host_ver = host["version"]

    dep = declared_dep(our_manifest, "acadrust")
    if dep is None:
        raise Unparseable("no [dependencies].acadrust in the plugin manifest")
    kind, value = dep_kind(dep)

    if kind == "version":
        # Legacy shape: a crates.io requirement redirected by [patch.crates-io].
        assert isinstance(value, str)
        if not caret_matches(value, host_ver):
            raise Escalate(
                f'host {tag} locks acadrust {host_ver}, which does not satisfy our '
                f'requirement "{value}". A [patch] only redirects where a version '
                "comes from — it cannot satisfy a requirement the manifest does "
                "not allow. This needs a source change, not a re-pin."
            )
        our_ver = value
    else:
        if our_lock is None:
            raise Unparseable(
                "a git-pinned acadrust needs our Cargo.lock to know which "
                "version we are built against"
            )
        our_ver = locked_package(our_lock, "acadrust")["version"]
        if compat_series(our_ver) != compat_series(host_ver):
            raise Escalate(
                f"host {tag} locks acadrust {host_ver}; we are built against "
                f"{our_ver}. Cargo will not cross that boundary on its own, and "
                "the API churn that rides along is a source decision, not a pin bump."
            )

    return {
        "host_version": host_ver,
        "host_url": host_url,
        "host_rev": host_rev,
        "host_slug": slug(host_url),
        "our_version": our_ver,
        "our_kind": kind,
    }


def gate_source_identity(our_manifest: str, plugin_api_manifest: str,
                         host_manifest: str | None = None) -> dict:
    """We must resolve acadrust to the same source the host does.

    Two halves, because upstream has broken it both ways:

    - We declare acadrust byte-for-byte the way ocs_plugin_api declares it. Two
      spellings of one commit are two sources to cargo, and the duplicate is
      invisible in a manifest diff — it shows up in the lockfile, or as a type
      error a hundred lines into the build.
    - We carry exactly the host root's `[patch]` redirect for acadrust, or no
      redirect if the host has none. A patch applies only from the workspace
      root, so the host's does not reach us; without mirroring it we build the
      source upstream redirected away from. This is what host v0.9.8 introduced.

    Preventive half of the ABI contract; gate_abi is the empirical half.
    """
    want_url, want_rev = linked_acadrust(plugin_api_manifest)
    dep = declared_dep(our_manifest, "acadrust")
    if dep is None:
        raise Unparseable("no [dependencies].acadrust in the plugin manifest")
    kind, value = dep_kind(dep)
    if kind != "git":
        raise Escalate(
            "ocs_plugin_api pulls acadrust from git, so a crates.io requirement "
            "(however patched) cannot resolve to the same source. This needs a "
            "source change, not a re-pin."
        )
    got_url, got_rev = value  # type: ignore[misc]
    if (got_url, got_rev) != (want_url, want_rev):
        raise Escalate(
            f'we declare acadrust as git "{got_url}" rev "{got_rev}"; '
            f'ocs_plugin_api declares git "{want_url}" rev "{want_rev}". '
            "Cargo keys a git source on that literal text, so any difference — "
            "including an abbreviated versus a full rev for the same commit — "
            "resolves two acadrusts into one graph."
        )

    report = {"url": got_url, "rev": got_rev}
    if host_manifest is not None:
        want_patch = patch_entry(host_manifest)
        got_patch = patch_entry(our_manifest)
        # Tuple equality, so this compares the literal strings — the same
        # byte-for-byte discipline the dependency spelling gets, and for the
        # same reason: cargo keys the replacement source on that text too.
        if want_patch != got_patch:
            raise Escalate(
                f"the host root carries {show_patch(want_patch)}; we carry "
                f"{show_patch(got_patch)}. A [patch] applies only from the "
                "workspace root, so ours has to mirror the host's exactly — "
                "otherwise we link a different acadrust than the host ships."
            )
        report["patch"] = show_patch(got_patch)
    return report


def gate_abi(our_lock: str, host_rev: str) -> dict:
    """After re-locking: exactly one acadrust, at exactly the host's rev.

    This states the invariant the re-pin exists to hold, instead of inferring it
    from version strings. It is also the gate that catches v0.9.5 on its own:
    once ocs_plugin_api began pulling acadrust from git, [patch.crates-io]
    stopped applying to it, and the graph would have carried two acadrusts.
    """
    pkg = locked_package(our_lock, "acadrust")  # raises Escalate on duplicates
    url, rev = git_source(pkg)
    if rev != host_rev:
        raise Escalate(
            f"we locked acadrust at {rev[:7]} ({url}) but the host is built "
            f"against {host_rev[:7]}. The loaded library would not share the "
            "host's types."
        )
    return {"url": url, "rev": rev, "version": pkg["version"]}


def gate_api_version(manifest_rs: str, plugin_toml: str, tag: str = "upstream") -> dict:
    """The host must still accept us — and we track its current API_VERSION.

    Acceptance is not sufficient. We link ocs_plugin_api and register with
    `ApiVersion::CURRENT`, so the cdylib reports the host's API_VERSION no
    matter what plugin.toml claims; declaring an older number still loads, which
    is how api_version sat at 2 across three host releases unnoticed.
    """
    low, high = api_version_window(manifest_rs)
    m = re.search(r"^api_version = (\d+)", plugin_toml, re.M)
    if not m:
        raise Unparseable("no api_version in plugin.toml")
    ours = int(m.group(1))
    if not low <= ours <= high:
        raise Escalate(
            f"host {tag} accepts api_version [{low}, {high}]; we declare {ours}. "
            "That is a source migration, not a re-pin."
        )
    return {"min": low, "max": high, "ours": ours, "target": high}


# ------------------------------------------------------------------- rewrite

# The mirrored `[patch]` block is regenerated wholesale rather than edited in
# place, because it has to be able to appear and disappear: v0.9.7 had no
# acadrust patch, v0.9.8 added one, and a later release may drop it again. The
# markers are what make "write nothing here" expressible as a substitution that
# still asserts it fired exactly once.
PATCH_BEGIN = "# --- BEGIN mirrored acadrust patch ---"
PATCH_END = "# --- END mirrored acadrust patch ---"
NO_PATCH = "# (the host declares no acadrust [patch] at this release)"


# The mirrored block contains a line that reads exactly like the dependency the
# rewrite edits — `acadrust = { git = "...", rev = "..." }` at column 0. Lifting
# the whole region out before the other substitutions run, and putting it back
# after, is what keeps "rewrote 1" honest. Doing it any other way made the
# dependency rewrite match twice and abort, which is how this was found.
PATCH_SENTINEL = "#<<<mirrored-acadrust-patch>>>"


def render_patch_block(patch) -> str:
    """The body between the markers: the host's redirect, or a note that there is none."""
    if patch is None:
        return NO_PATCH
    key, url, rev = patch
    return f'[patch."{key}"]\nacadrust = {{ git = "{url}", rev = "{rev}" }}'


def bump_patch(version: str) -> str:
    major, minor, patch = parse_semver(version)
    return f"{major}.{minor}.{patch + 1}"


def rewrite_manifests(cargo: str, plugin: str, *, tag: str, host_sha: str,
                      acad_url: str, acad_rev: str, api_version: int,
                      patch=None) -> tuple[str, str, str]:
    """Apply a mechanical re-pin, returning (Cargo.toml, plugin.toml, version).

    Every substitution asserts it fired exactly once. A regex that silently
    matches nothing is how a re-pin ships a manifest it did not actually update.
    """
    # Line endings come from the file, not from the platform, for the same
    # reason `read`/`write` disable translation: a CRLF checkout must not come
    # back as a whole-file diff.
    eol = "\r\n" if "\r\n" in cargo else "\n"
    cargo, n = re.subn(
        re.escape(PATCH_BEGIN) + r"\r?\n.*?" + re.escape(PATCH_END),
        PATCH_SENTINEL, cargo, flags=re.S,
    )
    if n != 1:
        raise Unparseable(
            f"expected 1 mirrored-patch block between {PATCH_BEGIN!r} and "
            f"{PATCH_END!r} to rewrite, rewrote {n}"
        )

    cargo, n = re.subn(r'(OpenCADStudio", rev = ")[0-9a-f]{40}',
                       lambda m: m.group(1) + host_sha, cargo)
    if n != 1:
        raise Unparseable(f"expected 1 ocs_plugin_api rev to rewrite, rewrote {n}")

    # Rewrite url and rev in place, leaving `features` (and anything else on the
    # line) untouched, so adopting a feature does not need a workflow change.
    # `acad_rev` is ocs_plugin_api's rev string verbatim — abbreviated or not —
    # because cargo keys the git source on that literal text. Expanding it to
    # the full sha here would be a silent ABI break, which is why the pattern
    # accepts any hex length rather than demanding 40.
    cargo, n = re.subn(
        r'^(acadrust = \{ git = ")[^"]+(", rev = ")[0-9a-f]{7,40}(")',
        lambda m: m.group(1) + acad_url + m.group(2) + acad_rev + m.group(3),
        cargo, flags=re.M,
    )
    if n != 1:
        raise Unparseable(f"expected 1 acadrust dependency to rewrite, rewrote {n}")

    # Keep the pin comments honest: they name the release the revs came from.
    cargo = re.sub(r"Pinned to the v[\d.]+ RELEASE TAG commit",
                   f"Pinned to the {tag} RELEASE TAG commit", cargo)
    cargo = re.sub(r"Match the v[\d.]+ release's acadrust",
                   f"Match the {tag} release's acadrust", cargo)

    current = re.search(r'^version = "([\d.]+)"', cargo, re.M)
    if not current:
        raise Unparseable("no [package] version in Cargo.toml")
    new = bump_patch(current.group(1))
    cargo = re.sub(r'^version = "[\d.]+"', f'version = "{new}"', cargo, count=1, flags=re.M)

    plugin = re.sub(r'^version = "[\d.]+"', f'version = "{new}"', plugin, count=1, flags=re.M)
    plugin, n = re.subn(r"^api_version = \d+", f"api_version = {api_version}",
                        plugin, count=1, flags=re.M)
    if n != 1:
        raise Unparseable(f"expected 1 api_version to rewrite, rewrote {n}")

    # Mirror the host root's acadrust [patch], or clear ours when it has none.
    body = render_patch_block(patch).replace("\n", eol)
    cargo = cargo.replace(PATCH_SENTINEL, PATCH_BEGIN + eol + body + eol + PATCH_END)
    return cargo, plugin, new


def current_host_pin(cargo: str) -> str:
    m = re.search(r'OpenCADStudio", rev = "([0-9a-f]{40})', cargo)
    if not m:
        raise Unparseable("could not read the current ocs_plugin_api pin from Cargo.toml")
    return m.group(1)


# ----------------------------------------------------------------------- CLI


def read(path: str) -> str:
    # newline="" disables translation, so a file's own line endings survive the
    # round-trip. Without it, rewriting on a CRLF checkout rewrites every line
    # and the "diff is purely mechanical" gate sees the whole manifest change.
    with open(path, encoding="utf-8", newline="") as f:
        return f.read()


def write(path: str, text: str) -> None:
    with open(path, "w", encoding="utf-8", newline="") as f:
        f.write(text)


def emit(report: dict) -> None:
    """Print key=value pairs, and append them to $GITHUB_OUTPUT when set."""
    lines = [f"{k}={v}" for k, v in report.items()]
    print("\n".join(lines))
    out = os.environ.get("GITHUB_OUTPUT")
    if out:
        with open(out, "a", encoding="utf-8") as f:
            f.write("\n".join(lines) + "\n")


def build_parser() -> argparse.ArgumentParser:
    p = argparse.ArgumentParser(description="Gates for the nightly re-pin.")
    sub = p.add_subparsers(dest="cmd", required=True)

    c = sub.add_parser("current-pin", help="the ocs_plugin_api sha we ship today")
    c.add_argument("--our-manifest", default="Cargo.toml")

    c = sub.add_parser("gate-acadrust", help="host acadrust stayed in our series")
    c.add_argument("--host-lock", required=True)
    c.add_argument("--our-manifest", default="Cargo.toml")
    c.add_argument("--our-lock", default="Cargo.lock")
    c.add_argument("--tag", default="upstream")

    c = sub.add_parser("linked-acadrust", help="acadrust as ocs_plugin_api declares it")
    c.add_argument("--plugin-api-manifest", required=True)

    c = sub.add_parser("host-plan", help="the acadrust source the host actually ships")
    c.add_argument("--plugin-api-manifest", required=True)
    c.add_argument("--host-manifest", required=True)
    c.add_argument("--host-lock", required=True)

    c = sub.add_parser("gate-source", help="our acadrust source matches the host's")
    c.add_argument("--plugin-api-manifest", required=True)
    c.add_argument("--host-manifest")
    c.add_argument("--our-manifest", default="Cargo.toml")

    c = sub.add_parser("gate-abi", help="one acadrust, at the host's rev")
    c.add_argument("--our-lock", default="Cargo.lock")
    c.add_argument("--host-rev", required=True)

    c = sub.add_parser("gate-api", help="host still accepts our api_version")
    c.add_argument("--manifest-rs", required=True)
    c.add_argument("--plugin-toml", default="plugin.toml")
    c.add_argument("--tag", default="upstream")

    c = sub.add_parser("rewrite", help="apply a mechanical re-pin in place")
    c.add_argument("--tag", required=True)
    c.add_argument("--host-sha", required=True)
    c.add_argument("--acadrust-url", required=True)
    c.add_argument("--acadrust-rev", required=True)
    c.add_argument("--api-version", type=int, required=True)
    # Empty means "the host has no acadrust patch", which is a value the rewrite
    # has to be able to express: it clears ours.
    c.add_argument("--patch-key", default="")
    c.add_argument("--patch-url", default="")
    c.add_argument("--patch-rev", default="")
    c.add_argument("--our-manifest", default="Cargo.toml")
    c.add_argument("--plugin-toml", default="plugin.toml")
    return p


def main(argv: list[str] | None = None) -> int:
    a = build_parser().parse_args(argv)
    try:
        if a.cmd == "current-pin":
            cargo = read(a.our_manifest)
            report = {"current_pin": current_host_pin(cargo)}
            kind, value = dep_kind(declared_dep(cargo, "acadrust"))
            if kind == "git":
                url, rev = value  # type: ignore[misc]
                report |= {"current_acadrust_url": url, "current_acadrust_rev": rev,
                           "current_acadrust_slug": slug(url)}
            emit(report)
        elif a.cmd == "gate-acadrust":
            r = gate_acadrust(read(a.host_lock), read(a.our_manifest),
                              read(a.our_lock), a.tag)
            print(f"host {a.tag} locks acadrust {r['host_version']} "
                  f"({r['host_slug']}@{r['host_rev'][:7]}); we ship {r['our_version']}")
            emit({k: v for k, v in r.items() if k.startswith("host_")})
        elif a.cmd == "linked-acadrust":
            url, rev = linked_acadrust(read(a.plugin_api_manifest))
            print(f"ocs_plugin_api pins acadrust to {slug(url)}@{rev}")
            emit({"acadrust_url": url, "acadrust_rev": rev, "acadrust_slug": slug(url)})
        elif a.cmd == "host-plan":
            r = host_acadrust_plan(read(a.plugin_api_manifest), read(a.host_manifest),
                                   read(a.host_lock))
            print(f"ocs_plugin_api pins acadrust to {slug(r['url'])}@{r['rev']}")
            if r["patch_key"]:
                print(f"host redirects it: [patch.\"{r['patch_key']}\"] -> "
                      f"{slug(r['patch_url'])}@{r['patch_rev'][:7]}")
            else:
                print("host applies no acadrust [patch]")
            print(f"host ships {slug(r['locked_url'])}@{r['locked_rev'][:7]}")
            emit({"acadrust_url": r["url"], "acadrust_rev": r["rev"],
                  "acadrust_slug": slug(r["url"]),
                  "patch_key": r["patch_key"], "patch_url": r["patch_url"],
                  "patch_rev": r["patch_rev"]})
        elif a.cmd == "gate-source":
            host_manifest = read(a.host_manifest) if a.host_manifest else None
            r = gate_source_identity(read(a.our_manifest), read(a.plugin_api_manifest),
                                     host_manifest)
            print(f"acadrust source matches ocs_plugin_api: {slug(r['url'])}@{r['rev']}")
            if "patch" in r:
                print(f"patch mirrors the host: {r['patch']}")
        elif a.cmd == "gate-abi":
            r = gate_abi(read(a.our_lock), a.host_rev)
            print(f"one acadrust {r['version']} at {r['rev'][:7]} ({r['url']})")
        elif a.cmd == "gate-api":
            r = gate_api_version(read(a.manifest_rs), read(a.plugin_toml), a.tag)
            print(f"host {a.tag} accepts [{r['min']}, {r['max']}]; "
                  f"plugin declares {r['ours']}")
            if r["ours"] != r["target"]:
                print(f"::notice::api_version {r['ours']} -> {r['target']}")
            emit({"api_version": r["target"]})
        elif a.cmd == "rewrite":
            cargo, plugin, new = rewrite_manifests(
                read(a.our_manifest), read(a.plugin_toml), tag=a.tag,
                host_sha=a.host_sha, acad_url=a.acadrust_url,
                acad_rev=a.acadrust_rev, api_version=a.api_version,
                patch=(a.patch_key, a.patch_url, a.patch_rev) if a.patch_key else None,
            )
            write(a.our_manifest, cargo)
            write(a.plugin_toml, plugin)
            emit({"newver": new})
    except Escalate as e:
        print(f"::error::{e}", file=sys.stderr)
        return ESCALATE
    except Unparseable as e:
        print(f"::error::gate cannot evaluate this input: {e}", file=sys.stderr)
        print("::error::This is a parser gap, not an upstream decision — add the "
              "offending tag to .github/scripts/tests/fixtures/host/ and fix "
              "repin_gates.py.", file=sys.stderr)
        return UNPARSEABLE
    return OK


if __name__ == "__main__":
    sys.exit(main())
