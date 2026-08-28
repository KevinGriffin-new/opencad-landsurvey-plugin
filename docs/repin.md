# Re-pinning to a new host release

This plugin is a cdylib the host loads at runtime. For the load to be sound it
must be built against the same `ocs_plugin_api` commit and the same `acadrust`
build as the host binary — approach B, same toolchain and same dependency
versions. The nightly `repin` workflow does that mechanically and ships the
result; the gates below decide when it is not allowed to.

## The ABI contract, stated plainly

Exactly one `acadrust` in the dependency graph, resolved to the commit the host
was built against.

Three things can break it, and none of them is obvious:

1. **A different commit.** Caught by comparing revs.
2. **The same commit, spelled differently.** Cargo keys a git source on the
   literal `?rev=` text, so `rev = "931c4ab"` and
   `rev = "931c4ab0c590b755e280bed318a35f41c57b139f"` are *two sources for one
   commit*. A manifest carrying the expanded form alongside an `ocs_plugin_api`
   carrying the short form locks two `acadrust` packages, and nothing in the
   manifest diff looks wrong. `cargo generate-lockfile` will happily produce it.
3. **A `[patch]` in the host root that we do not mirror.** A patch table applies
   only from the *workspace root*, and this plugin is its own root. When the
   host redirects `acadrust` and we copy only `ocs_plugin_api`'s spelling, we
   resolve the source upstream patched *away from* — the pin looks right, every
   spelling check passes, and the lockfile lands on a commit the host does not
   run. Host v0.9.8 did exactly this.

So the acadrust pin is copied verbatim from `ocs_plugin_api`'s own manifest,
abbreviation included, *and* the host root's `[patch]` for `acadrust` is
mirrored into ours — appearing, changing and disappearing as upstream's does.

### Where the truth actually lives

`ocs_plugin_api`'s manifest says what the host *declares*. The host's
**lockfile** says what the host *ships*. Until v0.9.8 those were the same
sentence, so the gates only ever read the first one.

`host-plan` now reconciles them: it works out the source from the two manifest
facts (the declared spelling, plus the root `[patch]` on top) and checks the
result against the host's lockfile. A plan that cannot reproduce the lockfile
exits `2` — a gap in this repository, not a decision — so `canary.yml` raises it
against upstream `main` days before a release depends on it.

Host v0.9.8 is the case for it. Nothing was unparseable; every manifest read
cleanly and simply did not describe the binary upstream shipped:

```toml
# crates/ocs_plugin_api/Cargo.toml — what we used to copy
acadrust = { git = "https://github.com/HakanSeven12/cadcodec.git", rev = "5b2ae66", ... }

# Cargo.toml (host root) — what we used to ignore
[patch."https://github.com/HakanSeven12/cadcodec.git"]
acadrust = { git = "https://git@github.com/HakanSeven12/cadcodec.git", rev = "788eea0..." }
```

The `git@` userinfo is load-bearing: cargo counts it as part of a source's
identity, which is the only reason the redirect is a legal patch at all — and
the reason the two spellings are two different `acadrust` builds.

## Why the pin is not a `[patch.crates-io]` any more

Until host v0.9.4 the plugin declared `acadrust = "0.4"` and redirected it with
`[patch.crates-io]`. That worked because `ocs_plugin_api` also took `acadrust`
from crates.io, so one patch covered both.

Host v0.9.5 changed `ocs_plugin_api` to depend on `acadrust` by git. A
crates.io patch does not reach a git-sourced dependency, so the patch would
have covered only *our* copy and the graph would have carried two. The plugin
now declares the same git dependency the host does, which is both simpler and
the only arrangement that actually holds the contract.

The `[patch]` the plugin carries today is a different animal: not a
`[patch.crates-io]` of our own invention, but a verbatim mirror of the host
root's redirect of one git source to another, written by the re-pin and empty
whenever upstream has none.

## The gates

All of them live in [`.github/scripts/repin_gates.py`](../.github/scripts/repin_gates.py)
and are runnable locally.

| gate | asks | on failure |
|---|---|---|
| `gate-acadrust` | did acadrust's series move (0.4 → 0.5)? | escalate — API churn rides along |
| `gate-api` | does the host still accept our `api_version`? | escalate — source migration |
| `host-plan` | do the host's manifests explain the host's lockfile? | **exit 2** — our model of the host is incomplete |
| `gate-source` | do we declare acadrust exactly as `ocs_plugin_api` does, and mirror the host's `[patch]`? | escalate — two sources, or the wrong one |
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
dependency five times in seven weeks:

| host | shape |
|---|---|
| ≤ v0.8.2 | `acadrust = "0.4"`, patched to `HakanSeven12/acadrust`, `branch = "main"` |
| v0.8.7 | patch URL moves to `OpenAEC-Foundation/acadifc` |
| v0.8.8 | `{ version = "0.4", features = [...] }`, patch rev-pinned, URL gains `.git` |
| v0.9.5 | `{ git = ".../cadcodec.git", rev = "...", features = [...] }`, no patch |
| v0.9.8 | same shape, but the root adds `[patch."…/cadcodec.git"]` redirecting it to `https://git@…/cadcodec.git` at another rev |

Each change broke a gate, and each break was found by a red nightly run — the
v0.9.5 one blocked nine consecutive nights. v0.9.8 is the one that got through
the parsers entirely: it broke no reader, it just made every reader answer a
question that had stopped being the right one. The gates were fixed reactively
three times because the logic lived in heredocs inside the workflow, where it
could not be run or tested outside a scheduled run.

Three things now close that loop:

- **The corpus.** `.github/scripts/tests/fixtures/host/<tag>/` holds the real
  `Cargo.toml`, `Cargo.lock`, `ocs_plugin_api/Cargo.toml` and `manifest.rs` from
  every tag in that table. The tests assert the correct verdict for each. These
  are real upstream files, not handwritten samples, because the bug class is
  "upstream spelled it a way nobody imagined" — invented fixtures would have
  missed every one.
- **`ci.yml`.** Runs the corpus and `cargo test --locked` on every PR, so gate
  changes are verified before they meet a live release.
- **`canary.yml`.** Runs the parsers against upstream `main` daily, through
  `host-plan`, so it catches both kinds of drift: a shape no parser can read,
  and manifests that read fine but no longer explain the host's own lockfile.
  Changes land on main before they land in a release, so either becomes a
  heads-up with lead time instead of a blocked release.

### The rule

**When a gate breaks, add the offending upstream tag to the corpus in the same
PR as the fix.** That is the whole ratchet. A fix without a fixture is how the
same class of break returns a sixth time.

## Running it by hand

```bash
python3 -m pytest .github/scripts/tests -v
```

Any gate can be run against a vendored tag without touching the network:

```bash
F=.github/scripts/tests/fixtures/host/v0.9.8
python3 .github/scripts/repin_gates.py host-plan \
  --plugin-api-manifest $F/ocs_plugin_api-Cargo.toml \
  --host-manifest $F/Cargo.toml --host-lock $F/Cargo.lock
```

To exercise the full chain — gates, build, host build, end-to-end — against the
current upstream release without shipping anything, dispatch `repin` with
`dry_run: true`. It runs every gate and stops before the merge.
