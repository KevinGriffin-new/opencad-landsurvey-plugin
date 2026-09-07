"""Regression corpus for the re-pin gates.

The fixtures under fixtures/host/ are the real Cargo.toml, Cargo.lock and
ocs_plugin_api/src/manifest.rs from upstream release tags — not handwritten
approximations. That is deliberate: every historical break of these gates was
upstream spelling its acadrust dependency a way nobody had imagined, so a
corpus of imagined shapes would have missed all four of them.

**When a gate breaks, add the offending upstream tag to fixtures/host/ in the
same PR as the fix.** That is what turns a one-off patch into a ratchet.

Negative cases (a series bump, a duplicated crate, a rev mismatch) are
synthesised inline: those are shapes upstream has not produced yet, and the
point of them is to prove the gate still fires when it should.
"""

from __future__ import annotations

import tomllib
from pathlib import Path

import pytest

import repin_gates as rg

TESTS = Path(__file__).resolve().parent
HOSTS = TESTS / "fixtures" / "host"
PLUGINS = TESTS / "fixtures" / "plugin"
REPO = TESTS.parents[2]  # <repo>/.github/scripts/tests -> <repo>


def host(tag: str, name: str) -> str:
    return (HOSTS / tag / name).read_text(encoding="utf-8")


# Ground truth read out of the vendored files themselves, then written down
# here so a fixture that silently changes fails the test instead of redefining
# the expectation.
TAGS = {
    #                acadrust  repo slug                                lock rev
    "v0.8.1": ("0.4.0", "github.com/HakanSeven12/acadrust", "c8e63eb6b5e6d23faeebc504d57accc091b4cae4"),
    "v0.8.2": ("0.4.0", "github.com/HakanSeven12/acadrust", "d1e198b66db8a460915d7ebcdb293e912d7bf964"),
    "v0.8.7": ("0.4.0", "github.com/OpenAEC-Foundation/acadifc", "022290a3a2a548c54c8fdf580ef5e44de36b4520"),
    "v0.8.8": ("0.4.0", "github.com/OpenAEC-Foundation/acadifc", "4afb27db9a15d5f94b117b881e44a3c0e6c91795"),
    "v0.9.4": ("0.4.0", "github.com/OpenAEC-Foundation/acadifc", "9ecaffc3f5c32bad61c9624b6acb03afe97a2863"),
    "v0.9.5": ("0.4.1", "github.com/HakanSeven12/cadcodec", "7e2fa8c7c7c774edc4fa54b328b727dc76a3ad95"),
    "v0.9.6": ("0.4.1", "github.com/HakanSeven12/cadcodec", "931c4ab0c590b755e280bed318a35f41c57b139f"),
    "v0.9.7": ("0.4.1", "github.com/HakanSeven12/cadcodec", "0908da7b6e4f702a6c78359a57f53e2b79cf39eb"),
    # v0.9.8's slug carries the `git@` userinfo, because the host root patches
    # the source to that spelling. Cargo treats it as a different source — which
    # is what makes the patch legal, and what made the re-pin ship the wrong one.
    "v0.9.8": ("0.4.1", "git@github.com/HakanSeven12/cadcodec", "788eea0161ba9f3eb7ee6569fd8bb52a8207a2ed"),
}

# How each tag spells [dependencies].acadrust in its Cargo.toml. The v0.8.8 row
# is the shape that broke the grep-based gate; the v0.9.5 row is the shape that
# broke the tomllib-based gate that replaced it.
MANIFEST_SHAPES = {
    "v0.8.1": "version",
    "v0.8.2": "version",
    "v0.8.7": "version",
    "v0.8.8": "version",
    "v0.9.4": "version",
    "v0.9.5": "git",
    "v0.9.6": "git",
    "v0.9.7": "git",
    "v0.9.8": "git",
}

API_WINDOWS = {
    "v0.8.1": (2, 3),
    "v0.8.2": (2, 3),
    "v0.8.7": (2, 3),
    "v0.8.8": (2, 3),
    "v0.9.4": (2, 3),
    "v0.9.5": (2, 3),
    "v0.9.6": (2, 4),
    "v0.9.7": (2, 4),
    "v0.9.8": (2, 4),
}


# ------------------------------------------------------- the corpus, per tag


@pytest.mark.parametrize("tag", sorted(TAGS))
def test_host_acadrust_resolves_from_the_lockfile(tag):
    """The lockfile is readable at every tag, whatever the manifest says.

    This is the load-bearing property: the gates read the lockfile precisely so
    that a manifest respelling cannot break them.
    """
    version, expect_slug, rev = TAGS[tag]
    pkg = rg.locked_package(host(tag, "Cargo.lock"), "acadrust")
    url, got_rev = rg.git_source(pkg)
    assert pkg["version"] == version
    assert rg.slug(url) == expect_slug
    assert got_rev == rev


@pytest.mark.parametrize("tag", sorted(MANIFEST_SHAPES))
def test_host_manifest_shape_is_classified(tag):
    """Every historical spelling classifies instead of raising.

    v0.9.5 is the regression this file exists for: a git-pinned dependency
    declares no version, and the previous gate treated that as "the requirement
    moved" and escalated nine nights running.
    """
    dep = rg.declared_dep(host(tag, "Cargo.toml"), "acadrust")
    kind, value = rg.dep_kind(dep)
    assert kind == MANIFEST_SHAPES[tag]
    if kind == "git":
        # The EFFECTIVE source, patch applied — at v0.9.8 the root manifest
        # declares one source in [dependencies] and redirects it in [patch], so
        # the declared spelling alone no longer predicts what the host builds.
        url, rev = rg.effective_acadrust(host(tag, "Cargo.toml"))
        assert rg.slug(url) == TAGS[tag][1]
        assert TAGS[tag][2].startswith(rev)  # manifest abbreviates the rev


@pytest.mark.parametrize("tag", sorted(API_WINDOWS))
def test_api_version_window(tag):
    assert rg.api_version_window(host(tag, "manifest.rs")) == API_WINDOWS[tag]


@pytest.mark.parametrize("tag", sorted(TAGS))
def test_gate_acadrust_accepts_every_tag_for_the_legacy_manifest(tag):
    """The crates.io + [patch] plugin manifest was compatible with all of these.

    Includes v0.9.5 and v0.9.6, whose 0.4.1 satisfies "0.4" — the requirement
    never moved, which is why escalating on it was wrong.
    """
    report = rg.gate_acadrust(
        host(tag, "Cargo.lock"), (PLUGINS / "legacy-patch-Cargo.toml").read_text(encoding="utf-8"), tag=tag
    )
    assert report["host_version"] == TAGS[tag][0]
    assert report["host_rev"] == TAGS[tag][2]


@pytest.mark.parametrize("tag", ["v0.9.5", "v0.9.6", "v0.9.7", "v0.9.8"])
def test_gate_acadrust_accepts_the_git_pinned_manifest(tag):
    """The shape we migrated to: our own git pin, compared series to series."""
    # The v0.9.8 host lockfile stands in for OUR lockfile here. The shipped
    # Cargo.lock has moved on to the 0.5.x acadrust series with host v2026.36,
    # so comparing it against these 0.4.x-era hosts would (correctly) escalate
    # and this test would stop exercising the accept path. The shipped lock is
    # checked against its own host in test_build_metadata.py
    # (test_2026_36_dependency_override_and_api).
    report = rg.gate_acadrust(
        host(tag, "Cargo.lock"),
        (REPO / "Cargo.toml").read_text(encoding="utf-8"),
        host("v0.9.8", "Cargo.lock"),
        tag=tag,
    )
    assert rg.compat_series(report["our_version"]) == rg.compat_series(TAGS[tag][0])


# ---------------------------------------------------- the gates still bite


def lock_with(*packages: tuple[str, str, str]) -> str:
    """A minimal lockfile carrying the given (name, version, source) stanzas."""
    out = ["version = 4", ""]
    for name, version, source in packages:
        out += ["[[package]]", f'name = "{name}"', f'version = "{version}"',
                f'source = "{source}"', ""]
    return "\n".join(out)


CADCODEC = "git+https://github.com/HakanSeven12/cadcodec.git?rev=931c4ab#931c4ab0c590b755e280bed318a35f41c57b139f"
OTHER_REV = "git+https://github.com/HakanSeven12/cadcodec.git?rev=7e2fa8c#7e2fa8c7c7c774edc4fa54b328b727dc76a3ad95"


def test_series_bump_escalates_for_the_legacy_manifest():
    """0.4 -> 0.5 is the case the original gate was written for. Still fires."""
    bumped = lock_with(("acadrust", "0.5.0", CADCODEC))
    manifest = (PLUGINS / "legacy-patch-Cargo.toml").read_text(encoding="utf-8")
    with pytest.raises(rg.Escalate, match="does not satisfy"):
        rg.gate_acadrust(bumped, manifest, tag="v9.9.9")


def test_series_bump_escalates_for_the_git_manifest():
    bumped = lock_with(("acadrust", "0.5.0", CADCODEC))
    # Same stand-in as above: a 0.4.x-series "our lock" is what makes the
    # 0.5.0 host bump a series crossing. The shipped lock is already 0.5.x.
    with pytest.raises(rg.Escalate, match="Cargo will not cross"):
        rg.gate_acadrust(
            bumped,
            (REPO / "Cargo.toml").read_text(encoding="utf-8"),
            host("v0.9.8", "Cargo.lock"),
            tag="v9.9.9",
        )


def test_two_acadrusts_escalate():
    """The failure mode [patch.crates-io] silently created at v0.9.5.

    Once ocs_plugin_api pulled acadrust from git, a crates.io patch no longer
    reached it, and the graph would have carried two builds of the crate whose
    types are not interchangeable across the plugin boundary.
    """
    doubled = lock_with(("acadrust", "0.4.0", OTHER_REV), ("acadrust", "0.4.1", CADCODEC))
    with pytest.raises(rg.Escalate, match="2 different builds"):
        rg.gate_abi(doubled, "931c4ab0c590b755e280bed318a35f41c57b139f")


def test_abi_gate_catches_a_rev_mismatch():
    ours = lock_with(("acadrust", "0.4.1", OTHER_REV))
    with pytest.raises(rg.Escalate, match="would not share the"):
        rg.gate_abi(ours, "931c4ab0c590b755e280bed318a35f41c57b139f")


def test_abi_gate_passes_on_a_matching_rev():
    ours = lock_with(("acadrust", "0.4.1", CADCODEC))
    assert rg.gate_abi(ours, "931c4ab0c590b755e280bed318a35f41c57b139f")["version"] == "0.4.1"


def test_registry_source_escalates_rather_than_crashing():
    published = lock_with(("acadrust", "0.4.1", "registry+https://github.com/rust-lang/crates.io-index"))
    with pytest.raises(rg.Escalate, match="resolves from a registry"):
        rg.gate_acadrust(published, (REPO / "Cargo.toml").read_text(encoding="utf-8"))


def test_branch_pin_escalates():
    """A branch pin ships a different binary on every rebuild."""
    with pytest.raises(rg.Escalate, match="branch or tag"):
        rg.dep_kind({"git": "https://example.invalid/x", "branch": "main"})


def test_api_version_gate_escalates_outside_the_window():
    plugin = 'api_version = 1\nversion = "0.1.0"\n'
    with pytest.raises(rg.Escalate, match="source migration"):
        rg.gate_api_version(host("v0.9.6", "manifest.rs"), plugin, tag="v0.9.6")


# --------------------------------------- unparseable is not the same as moved


def test_unknown_shape_is_unparseable_not_escalate():
    """A shape we cannot read is a bug in this file, not an upstream decision.

    Conflating the two is what made the last nine failures read as a six-item
    checklist rather than "the parser needs a new case".
    """
    with pytest.raises(rg.Unparseable):
        rg.dep_kind({"workspace": True})


def test_explicit_version_operators_are_unparseable():
    with pytest.raises(rg.Unparseable):
        rg.caret_matches(">=0.4, <0.6", "0.4.1")


def test_cli_exit_codes_distinguish_the_two(tmp_path):
    """The workflow branches on these, so they are part of the contract."""
    manifest = tmp_path / "Cargo.toml"
    manifest.write_text('[dependencies]\nacadrust = { workspace = true }\n', encoding="utf-8")
    lock = tmp_path / "host.lock"
    lock.write_text(lock_with(("acadrust", "0.4.1", CADCODEC)), encoding="utf-8")
    assert rg.main(["gate-acadrust", "--host-lock", str(lock),
                    "--our-manifest", str(manifest)]) == rg.UNPARSEABLE

    manifest.write_text('[dependencies]\nacadrust = "0.4"\n', encoding="utf-8")
    lock.write_text(lock_with(("acadrust", "0.5.0", CADCODEC)), encoding="utf-8")
    assert rg.main(["gate-acadrust", "--host-lock", str(lock),
                    "--our-manifest", str(manifest)]) == rg.ESCALATE

    lock.write_text(lock_with(("acadrust", "0.4.1", CADCODEC)), encoding="utf-8")
    assert rg.main(["gate-acadrust", "--host-lock", str(lock),
                    "--our-manifest", str(manifest)]) == rg.OK


# --------------------------------------------------------- caret arithmetic


@pytest.mark.parametrize(
    "req,version,expected",
    [
        ("0.4", "0.4.0", True),
        ("0.4", "0.4.1", True),
        ("0.4", "0.4.99", True),
        ("0.4", "0.5.0", False),
        ("0.4", "0.3.9", False),
        ("0.4.1", "0.4.0", False),
        ("0.4.1", "0.4.7", True),
        ("1.2", "1.9.0", True),
        ("1.2", "2.0.0", False),
        ("0.0.3", "0.0.3", True),
        ("0.0.3", "0.0.4", False),
        ("0", "0.9.9", True),
        ("0", "1.0.0", False),
    ],
)
def test_caret_matches(req, version, expected):
    assert rg.caret_matches(req, version) is expected


# ------------------------------------------------------------- the rewrite


NEW_SHA = "a" * 40
NEW_REV = "b" * 40


def test_rewrite_is_complete_and_bumps_the_patch_version():
    cargo = (REPO / "Cargo.toml").read_text(encoding="utf-8")
    plugin = (REPO / "plugin.toml").read_text(encoding="utf-8")
    out_cargo, out_plugin, new = rg.rewrite_manifests(
        cargo, plugin, tag="v9.9.9", host_sha=NEW_SHA,
        acad_url="https://example.invalid/acadrust.git", acad_rev=NEW_REV,
        api_version=7,
    )
    assert f'OpenCADStudio", rev = "{NEW_SHA}"' in out_cargo
    assert f'"https://example.invalid/acadrust.git", rev = "{NEW_REV}"' in out_cargo
    assert rg.parse_semver(new) > rg.parse_semver(
        rg.SEMVER.search(cargo).group(0)  # the [package] version comes first
    )
    assert f'version = "{new}"' in out_plugin
    assert "api_version = 7" in out_plugin
    # The features list on the acadrust line survives the rewrite.
    assert 'features = ["serde"]' in out_cargo


def test_rewrite_preserves_everything_it_was_not_asked_to_change():
    cargo = (REPO / "Cargo.toml").read_text(encoding="utf-8")
    plugin = (REPO / "plugin.toml").read_text(encoding="utf-8")
    out_cargo, _, _ = rg.rewrite_manifests(
        cargo, plugin, tag="v9.9.9", host_sha=NEW_SHA,
        acad_url="https://example.invalid/acadrust.git", acad_rev=NEW_REV,
        api_version=7, patch=rg.patch_entry(cargo),
    )
    assert out_cargo.count("\n") == cargo.count("\n"), "the rewrite added or dropped lines"
    assert "[workspace]" in out_cargo


def test_rewrite_refuses_a_manifest_it_did_not_actually_match():
    """A regex that matches nothing is how a re-pin ships an unchanged manifest."""
    with pytest.raises(rg.Unparseable, match="acadrust dependency to rewrite, rewrote 0"):
        rg.rewrite_manifests(
            '[package]\nversion = "0.1.0"\n[dependencies]\n'
            'ocs_plugin_api = { git = "https://github.com/HakanSeven12/OpenCADStudio", rev = "'
            + "c" * 40 + '" }\n'
            + rg.PATCH_BEGIN + "\n" + rg.NO_PATCH + "\n" + rg.PATCH_END + "\n",
            "version = \"0.1.0\"\napi_version = 3\n",
            tag="v9.9.9", host_sha=NEW_SHA, acad_url="https://example.invalid/a.git",
            acad_rev=NEW_REV, api_version=7,
        )


# ------------------------------------------------- what we actually ship


def test_shipped_lockfile_agrees_with_the_shipped_pin():
    """Cargo.lock must match Cargo.toml's acadrust rev.

    release.yml builds --locked, so a manifest bump committed without a
    refreshed lockfile passes every gate here and fails at release time.
    """
    cargo = (REPO / "Cargo.toml").read_text(encoding="utf-8")
    kind, value = rg.dep_kind(rg.declared_dep(cargo, "acadrust"))
    assert kind == "git", "the plugin pins acadrust by rev, like the host does"
    declared_url, declared_rev = value
    patch = rg.patch_entry(cargo)
    if patch is not None:
        assert patch[0] == declared_url
        _, declared_url, declared_rev = patch
    locked_url, locked_rev = rg.git_source(
        rg.locked_package((REPO / "Cargo.lock").read_text(encoding="utf-8"), "acadrust")
    )
    assert rg.slug(locked_url) == rg.slug(declared_url)
    assert locked_rev.startswith(declared_rev)


def test_no_stale_crates_io_patch_for_acadrust():
    """[patch.crates-io] cannot reach ocs_plugin_api's git-sourced acadrust.

    Leaving one behind would look like it was doing something while the graph
    quietly resolved two different acadrusts.
    """
    cargo = (REPO / "Cargo.toml").read_text(encoding="utf-8")
    assert rg.declared_dep(cargo, "acadrust", table="patch") is None
    import tomllib

    patch = tomllib.loads(cargo).get("patch", {}).get("crates-io", {})
    assert "acadrust" not in patch


# ------------------------------------------- matching ocs_plugin_api's source


def plugin_api(tag: str) -> str:
    return host(tag, "ocs_plugin_api-Cargo.toml")


def shipped_host_tag() -> str:
    """The corpus tag whose ocs_plugin_api matches the acadrust we declare.

    Hardcoding a tag here goes stale the first time the re-pin ships: the tests
    then compare what we ship against a host we no longer pin, and start passing
    or failing for reasons unrelated to what they assert. A mechanical re-pin to
    a release nobody had to debug leaves no fixture behind — that is the ratchet
    working as intended — so not finding one is a skip, not a failure.
    """
    ours = rg.effective_acadrust((REPO / "Cargo.toml").read_text(encoding="utf-8"))
    for tag in sorted(TAGS, reverse=True):
        try:
            if rg.linked_acadrust(plugin_api(tag)) == ours:
                return tag
        except (rg.Escalate, rg.Unparseable):
            continue
    pytest.skip("the host we ship is newer than the vendored corpus")


@pytest.mark.parametrize("tag", ["v0.8.1", "v0.8.2", "v0.8.7", "v0.8.8", "v0.9.4"])
def test_linked_acadrust_escalates_while_ocs_plugin_api_used_crates_io(tag):
    """Before v0.9.5 the linked crate took acadrust from crates.io.

    That is what made [patch.crates-io] work at all, and its disappearance is
    the real reason v0.9.5 could not be re-pinned mechanically.
    """
    with pytest.raises(rg.Escalate, match="not a git pin"):
        rg.linked_acadrust(plugin_api(tag))


@pytest.mark.parametrize("tag,rev", [("v0.9.5", "7e2fa8c"), ("v0.9.6", "931c4ab"),
                                     ("v0.9.7", "0908da7"), ("v0.9.8", "5b2ae66")])
def test_linked_acadrust_returns_the_rev_verbatim(tag, rev):
    """Abbreviated, exactly as upstream wrote it — not expanded to the full sha."""
    url, got = rg.linked_acadrust(plugin_api(tag))
    assert got == rev
    assert rg.slug(url) == "github.com/HakanSeven12/cadcodec"


@pytest.mark.parametrize("tag", ["v0.9.5", "v0.9.6", "v0.9.7"])
def test_linked_rev_predicted_the_shipped_rev_until_v0_9_8(tag):
    """Up to v0.9.7, ocs_plugin_api's spelling was the whole story."""
    _, rev = rg.linked_acadrust(plugin_api(tag))
    assert TAGS[tag][2].startswith(rev)


def test_v0_9_8_ships_an_acadrust_ocs_plugin_api_does_not_name():
    """The break this file's [patch] handling exists for.

    ocs_plugin_api declares `5b2ae66`; the host root redirects that source and
    ships `788eea0`. Copying the declared spelling — which is what every gate
    before this one checked — produced a manifest that passed and a binary that
    could not share the host's types.
    """
    _, declared = rg.linked_acadrust(plugin_api("v0.9.8"))
    assert declared == "5b2ae66"
    assert not TAGS["v0.9.8"][2].startswith(declared)


# ---------------------------------------------- the host's [patch] redirect

# What the host root declares for acadrust in [patch], per tag. Ground truth
# from the vendored files, written down so a fixture that changes fails a test
# instead of quietly redefining the expectation.
HOST_PATCH = {
    "v0.9.5": None,
    "v0.9.6": None,
    "v0.9.7": None,
    "v0.9.8": ("https://github.com/HakanSeven12/cadcodec.git",
               "https://git@github.com/HakanSeven12/cadcodec.git",
               "788eea0161ba9f3eb7ee6569fd8bb52a8207a2ed"),
}


@pytest.mark.parametrize("tag", ["v0.9.5", "v0.9.6", "v0.9.7", "v0.9.8"])
def test_patch_entry_reads_the_host_root(tag):
    assert rg.patch_entry(host(tag, "Cargo.toml")) == HOST_PATCH[tag]


def test_legacy_crates_io_patch_is_not_read_as_a_git_redirect():
    """Before v0.9.5 the patch was a [patch.crates-io] branch pin.

    It redirected acadrust too, but to a branch, which is a shape the re-pin
    cannot mirror — a moving pin ships a different binary on every rebuild.
    """
    with pytest.raises(rg.Escalate, match="branch or tag"):
        rg.patch_entry(host("v0.8.1", "Cargo.toml"))


@pytest.mark.parametrize("tag", ["v0.9.5", "v0.9.6", "v0.9.7", "v0.9.8"])
def test_host_plan_reproduces_the_hosts_lockfile(tag):
    """The plan's whole job: manifests in, the shipped source out."""
    plan = rg.host_acadrust_plan(plugin_api(tag), host(tag, "Cargo.toml"),
                                 host(tag, "Cargo.lock"))
    assert plan["locked_rev"] == TAGS[tag][2]
    assert rg.slug(plan["locked_url"]) == TAGS[tag][1]
    if HOST_PATCH[tag] is None:
        assert plan["patch_key"] == ""
        assert plan["locked_rev"].startswith(plan["rev"])
    else:
        assert (plan["patch_key"], plan["patch_url"], plan["patch_rev"]) == HOST_PATCH[tag]


def test_host_plan_is_unparseable_when_it_cannot_explain_the_lockfile():
    """An unexplained redirect is a gap here, not a decision for a human.

    This is the case v0.9.8 actually was before the [patch] was read: the
    manifests parsed perfectly and simply did not describe the binary upstream
    shipped. Reported as UNPARSEABLE so canary.yml raises it against upstream
    main, with lead time, instead of a release stalling on it.
    """
    lock = lock_with(("acadrust", "0.4.1", OTHER_REV))
    with pytest.raises(rg.Unparseable, match="no \\[patch\\] to explain"):
        rg.host_acadrust_plan(plugin_api("v0.9.6"), host("v0.9.6", "Cargo.toml"), lock)


def test_host_plan_escalates_when_the_patch_targets_another_source():
    """A redirect we could mirror, that would leave ocs_plugin_api unredirected.

    Mirroring it verbatim would look right and lock two acadrusts: ours patched,
    ocs_plugin_api's not.
    """
    manifest = host("v0.9.8", "Cargo.toml").replace(
        '[patch."https://github.com/HakanSeven12/cadcodec.git"]',
        '[patch."https://github.com/HakanSeven12/elsewhere.git"]',
    )
    with pytest.raises(rg.Escalate, match="would leave ocs_plugin_api"):
        rg.host_acadrust_plan(plugin_api("v0.9.8"), manifest, host("v0.9.8", "Cargo.lock"))


def test_host_plan_is_unparseable_when_the_patch_does_not_explain_the_lockfile():
    manifest = host("v0.9.8", "Cargo.toml").replace(
        '"788eea0161ba9f3eb7ee6569fd8bb52a8207a2ed" }',
        '"7e2fa8c7c7c774edc4fa54b328b727dc76a3ad95" }',
    )
    with pytest.raises(rg.Unparseable, match="does not explain the"):
        rg.host_acadrust_plan(plugin_api("v0.9.8"), manifest, host("v0.9.8", "Cargo.lock"))


def test_gate_source_identity_passes_for_what_we_ship():
    """The real manifest against the real host it is pinned to, patch included."""
    tag = shipped_host_tag()
    rg.gate_source_identity((REPO / "Cargo.toml").read_text(encoding="utf-8"),
                            plugin_api(tag), host(tag, "Cargo.toml"))


def synth_plugin(rev: str, patch=None) -> str:
    """A minimal plugin manifest declaring acadrust, optionally patched."""
    out = ['[package]', 'version = "0.1.0"', '', '[dependencies]',
           'acadrust = { git = "https://github.com/HakanSeven12/cadcodec.git", '
           f'rev = "{rev}", features = ["serde"] }}']
    if patch is not None:
        key, url, prev = patch
        out += ['', f'[patch."{key}"]',
                f'acadrust = {{ git = "{url}", rev = "{prev}" }}']
    return "\n".join(out) + "\n"


def test_gate_source_identity_requires_the_hosts_patch_to_be_mirrored():
    """The exact state the failing nightly produced.

    The dependency spelling matches ocs_plugin_api character for character — the
    only thing this gate used to check — and the graph still resolves a
    different acadrust, because the host's redirect is not mirrored.
    """
    ours = synth_plugin("5b2ae66")
    rg.gate_source_identity(ours, plugin_api("v0.9.8"))  # the old gate is happy
    with pytest.raises(rg.Escalate, match="mirror the host's exactly"):
        rg.gate_source_identity(ours, plugin_api("v0.9.8"), host("v0.9.8", "Cargo.toml"))


def test_gate_source_identity_rejects_a_patch_the_host_does_not_have():
    """Mirroring runs both ways: a patch left behind is a break too.

    A redirect upstream has dropped points us at a rev the host no longer
    builds, and every spelling in the manifest still looks right.
    """
    ours = synth_plugin("0908da7", HOST_PATCH["v0.9.8"])
    with pytest.raises(rg.Escalate, match="mirror the host's exactly"):
        rg.gate_source_identity(ours, plugin_api("v0.9.7"), host("v0.9.7", "Cargo.toml"))


def test_gate_source_identity_accepts_a_correctly_mirrored_patch():
    ours = synth_plugin("5b2ae66", HOST_PATCH["v0.9.8"])
    report = rg.gate_source_identity(ours, plugin_api("v0.9.8"),
                                     host("v0.9.8", "Cargo.toml"))
    assert "git@github.com" in report["patch"]
    # And the manifest really does resolve to what the host ships.
    assert rg.effective_acadrust(ours) == (HOST_PATCH["v0.9.8"][1], HOST_PATCH["v0.9.8"][2])


def test_gate_source_identity_rejects_an_expanded_rev():
    """The trap this gate exists for: the same commit, spelled two ways.

    `cargo generate-lockfile` really does emit two acadrust packages for this,
    and nothing in the manifest diff looks wrong.
    """
    expanded = (REPO / "Cargo.toml").read_text(encoding="utf-8").replace(
        'rev = "931c4ab"', 'rev = "931c4ab0c590b755e280bed318a35f41c57b139f"'
    )
    with pytest.raises(rg.Escalate, match="abbreviated versus a full rev"):
        rg.gate_source_identity(expanded, plugin_api("v0.9.6"))


def test_gate_source_identity_rejects_the_legacy_patch_manifest():
    legacy = (PLUGINS / "legacy-patch-Cargo.toml").read_text(encoding="utf-8")
    with pytest.raises(rg.Escalate, match="cannot resolve to the same source"):
        rg.gate_source_identity(legacy, plugin_api("v0.9.6"))


def test_rewrite_preserves_line_endings(tmp_path):
    """A CRLF checkout must not come back as a whole-file rewrite.

    Python's text mode translates on write, which would turn a three-line pin
    bump into a diff touching every line — and the "purely mechanical" gate
    reads that diff.
    """
    src = (REPO / "Cargo.toml").read_bytes().replace(b"\r\n", b"\n").replace(b"\n", b"\r\n")
    manifest = tmp_path / "Cargo.toml"
    manifest.write_bytes(src)
    plugin = tmp_path / "plugin.toml"
    plugin.write_bytes((REPO / "plugin.toml").read_bytes())

    key, url, rev = HOST_PATCH["v0.9.8"]
    assert rg.main([
        "rewrite", "--tag", "v9.9.9", "--host-sha", NEW_SHA,
        "--acadrust-url", "https://github.com/HakanSeven12/cadcodec.git",
        "--acadrust-rev", "deadbee", "--api-version", "4",
        # With a patch, so the generated block's own line endings are covered:
        # it is composed rather than edited, so it is the one place a bare LF
        # can be introduced from this file instead of inherited from the input.
        "--patch-key", key, "--patch-url", url, "--patch-rev", rev,
        "--our-manifest", str(manifest), "--plugin-toml", str(plugin),
    ]) == rg.OK

    out = manifest.read_bytes()
    # The shipped manifest may already carry a two-line patch block.
    delta = 0 if rg.patch_entry(src.decode()) else 1
    assert out.count(b"\r\n") == src.count(b"\r\n") + delta
    assert b"\n" not in out.replace(b"\r\n", b""), "a bare LF crept in"


def test_rewrite_mirrors_the_hosts_patch_and_then_drops_it():
    """The block has to appear and disappear, not just change.

    v0.9.7 had no acadrust patch, v0.9.8 added one, and nothing says a later
    release keeps it. A rewrite that could only fill the block in would leave a
    stale redirect behind the day upstream removes theirs — pointing us at a rev
    the host no longer builds, with every spelling in the manifest still right.
    """
    cargo = (REPO / "Cargo.toml").read_text(encoding="utf-8")
    plugin = (REPO / "plugin.toml").read_text(encoding="utf-8")
    common = dict(tag="v9.9.9", host_sha=NEW_SHA,
                  acad_url="https://github.com/HakanSeven12/cadcodec.git",
                  api_version=4)

    added, _, _ = rg.rewrite_manifests(cargo, plugin, acad_rev="5b2ae66",
                                       patch=HOST_PATCH["v0.9.8"], **common)
    key, url, rev = HOST_PATCH["v0.9.8"]
    assert f'[patch."{key}"]' in added
    assert f'acadrust = {{ git = "{url}", rev = "{rev}" }}' in added
    assert rg.NO_PATCH not in added
    # The mirrored manifest resolves to the source the host actually ships.
    assert rg.effective_acadrust(added) == (url, rev)
    rg.gate_source_identity(added, plugin_api("v0.9.8"), host("v0.9.8", "Cargo.toml"))

    dropped, _, _ = rg.rewrite_manifests(added, plugin, acad_rev="0908da7",
                                         patch=None, **common)
    assert rg.NO_PATCH in dropped
    assert rg.patch_entry(dropped) is None
    assert "patch" not in tomllib.loads(dropped)
    rg.gate_source_identity(dropped, plugin_api("v0.9.7"), host("v0.9.7", "Cargo.toml"))


def test_rewrite_refuses_a_manifest_without_the_patch_markers():
    """Silently skipping the block is how a stale redirect ships unnoticed."""
    cargo = (REPO / "Cargo.toml").read_text(encoding="utf-8").replace(rg.PATCH_END, "")
    with pytest.raises(rg.Unparseable, match="mirrored-patch block"):
        rg.rewrite_manifests(
            cargo, (REPO / "plugin.toml").read_text(encoding="utf-8"),
            tag="v9.9.9", host_sha=NEW_SHA, acad_url="https://example.invalid/a.git",
            acad_rev=NEW_REV, api_version=4,
        )


def test_rewrite_writes_an_abbreviated_rev_verbatim():
    """The rewrite must not helpfully expand the rev it was handed."""
    cargo = (REPO / "Cargo.toml").read_text(encoding="utf-8")
    plugin = (REPO / "plugin.toml").read_text(encoding="utf-8")
    out, _, _ = rg.rewrite_manifests(
        cargo, plugin, tag="v9.9.9", host_sha=NEW_SHA,
        acad_url="https://github.com/HakanSeven12/cadcodec.git",
        acad_rev="deadbee", api_version=4,
    )
    assert 'rev = "deadbee", features = ["serde"]' in out
