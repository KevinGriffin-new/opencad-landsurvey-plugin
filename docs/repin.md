# Re-pinning to a new host release

This plugin is a cdylib the host loads at runtime. For the load to be sound it
must be built against the same `ocs_plugin_api` commit and the same `acadrust`
build as the host binary — approach B, same toolchain and same dependency
versions. The nightly `repin` workflow does that mechanically and ships the
result; the gates below decide when it is not allowed to.

## The ABI contract, stated plainly

Exactly one `acadrust` in the dependency graph, resolved to the commit the host
was built against.

Two things can break it, and only one of them is obvious:

1. **A different commit.** Caught by comparing revs.
2. **The same commit, spelled differently.** Cargo keys a git source on the
   literal `?rev=` text, so `rev = "931c4ab"` and
   `rev = "931c4ab0c590b755e280bed318a35f41c57b139f"` are *two sources for one
   commit*. A manifest carrying the expanded form alongside an `ocs_plugin_api`
   carrying the short form locks two `acadrust` packages, and nothing in the
   manifest diff looks wrong. `cargo generate-lockfile` will happily produce it.

That is why the acadrust pin is copied verbatim from `ocs_plugin_api`'s own
manifest, abbreviation included, and why `gate-source` compares the spelling
rather than the resolved commit.

## Why the pin is not a `[patch.crates-io]` any more

Until host v0.9.4 the plugin declared `acadrust = "0.4"` and redirected it with
`[patch.crates-io]`. That worked because `ocs_plugin_api` also took `acadrust`
from crates.io, so one patch covered both.

Host v0.9.5 changed `ocs_plugin_api` to depend on `acadrust` by git. A
crates.io patch does not reach a git-sourced dependency, so the patch would
have covered only *our* copy and the graph would have carried two. The plugin
now declares the same git dependency the host does, which is both simpler and
the only arrangement that actually holds the contract.

## The gates

All of them live in [`.github/scripts/repin_gates.py`](../.github/scripts/repin_gates.py)
and are runnable locally.

| gate | asks | on failure |
|---|---|---|
| `gate-acadrust` | did acadrust's series move (0.4 → 0.5)? | escalate — API churn rides along |
| `gate-api` | does the host still accept our `api_version`? | escalate — source migration |
| `gate-source` | do we declare acadrust exactly as `ocs_plugin_api` does? | escalate — two sources |
| `gate-abi` | did the lockfile resolve exactly one acadrust, at the host's rev? | escalate — the real contract |
| *(inline)* | is the diff confined to the three manifests? | escalate — not mechanical |
| *(inline)* | does it build, test, and load into a host built at the tag? | escalate |

### Two kinds of failure

The scripts exit `1` when a human has a decision to make and `2` when a parser
here could not read upstream's input at all. The escalation issue says which,
because they need completely different responses: `2` is a bug in this
repository, `1` is a judgement call about the dependency.

## Why the gates keep breaking, and what stops it

The gates read upstream's manifests, and upstream has respelled its `acadrust`
dependency four times in six weeks:

| host | shape |
|---|---|
| ≤ v0.8.2 | `acadrust = "0.4"`, patched to `HakanSeven12/acadrust`, `branch = "main"` |
| v0.8.7 | patch URL moves to `OpenAEC-Foundation/acadifc` |
| v0.8.8 | `{ version = "0.4", features = [...] }`, patch rev-pinned, URL gains `.git` |
| v0.9.5 | `{ git = ".../cadcodec.git", rev = "...", features = [...] }`, no patch |

Each change broke a gate, and each break was found by a red nightly run — the
v0.9.5 one blocked nine consecutive nights. The gates were fixed reactively
three times because the logic lived in heredocs inside the workflow, where it
could not be run or tested outside a scheduled run.

Three things now close that loop:

- **The corpus.** `.github/scripts/tests/fixtures/host/<tag>/` holds the real
  `Cargo.toml`, `Cargo.lock`, `ocs_plugin_api/Cargo.toml` and `manifest.rs` from
  every tag in that table. The tests assert the correct verdict for each. These
  are real upstream files, not handwritten samples, because the bug class is
  "upstream spelled it a way nobody imagined" — invented fixtures would have
  missed all four.
- **`ci.yml`.** Runs the corpus and `cargo test --locked` on every PR, so gate
  changes are verified before they meet a live release.
- **`canary.yml`.** Runs the parsers against upstream `main` daily. Shape
  changes land on main before they land in a release, so an unreadable shape
  becomes a heads-up with lead time instead of a blocked release.

### The rule

**When a gate breaks, add the offending upstream tag to the corpus in the same
PR as the fix.** That is the whole ratchet. A fix without a fixture is how the
same class of break returns a fourth time.

## Running it by hand

```bash
python3 -m pytest .github/scripts/tests -v
```

To exercise the full chain — gates, build, host build, end-to-end — against the
current upstream release without shipping anything, dispatch `repin` with
`dry_run: true`. It runs every gate and stops before the merge.
