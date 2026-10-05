# git-workset

Named sparse-checkout profiles for git worktrees. Like Perforce stream filters, but for git.

Create lightweight worktrees that only check out the directories you need, sharing one submodule object store across every worktree, with selective LFS downloads — all driven by a single `.git-workset.toml` config.

## Install

### Homebrew (macOS/Linux)

```sh
brew install lauripiispanen/tap/git-workset
```

This uses a [Homebrew tap](https://github.com/lauripiispanen/homebrew-tap). The formula template is in `Formula/git-workset.rb` in this repo.

### Pre-built binaries

Download the latest release from [GitHub Releases](https://github.com/lauripiispanen/git-workset/releases), extract the archive, and place `git-workset` somewhere on your `PATH`.

On Linux, prefer the `*-unknown-linux-musl` archives: they are statically linked and run on any distribution, including Alpine and older-glibc hosts such as Debian 12, Ubuntu 22.04 and RHEL 9. The `*-linux-gnu` builds need a recent glibc. `SHA256SUMS` covers every archive:

```sh
sha256sum -c SHA256SUMS --ignore-missing
```

### From source

```sh
cargo install --path .
```

---

Once installed, git automatically discovers `git-workset` as a subcommand, so you can use `git workset` directly.

## Quick start

```sh
# Clone a repo with only the files you need — no full checkout
git workset clone git@github.com:org/repo.git ./repo --workset server

# Clone with minimal history too
git workset clone git@github.com:org/repo.git ./repo --workset server --shallow

# Or if you already have a repo, create a config template
git workset init

# Edit .git-workset.toml to define your profiles (see below)

# Carve a lightweight worktree with a new branch
git workset carve ../feature-branch -b feature-branch --workset server

# Carve from an existing branch
git workset carve ../feature-branch feature-branch --workset server

# Compose multiple profiles
git workset carve ../fix -b fix main --workset server+art
```

## Configuration

Define profiles in `.git-workset.toml` at the repo root:

```toml
version = 1            # optional; a newer git-workset may add versions

# Repo-wide settings (not per-profile — see "Submodules and worksets")
[submodules]
sharing = "shared"     # or "isolated"

[workset.server]
description = "Backend server development"
include = ["src/server", "src/shared", "src/networking"]
exclude_lfs = ["*.psd", "*.fbx", "*.wav"]
include_lfs = ["*.json", "*.toml"]
sparse_cone = true

[workset.server.submodules]
shallow = true
skip = ["third_party/art-pipeline"]

[workset.client]
description = "Game client work"
include = ["src/client", "src/shared", "src/rendering"]
include_lfs = ["*.png", "*.atlas"]

[workset.client.submodules]
shallow = true

[workset.art]
description = "Full asset pipeline"
include = ["assets/", "src/tools/asset-pipeline"]

[workset.art.submodules]
shallow = false
```

### Config reference

Per-profile fields, under `[workset.<name>]`:

| Field | Default | Description |
|-------|---------|-------------|
| `description` | — | Human-readable profile description |
| `include` | `[]` | Directories to include in sparse checkout, relative to the repo root (empty = full tree). In no-cone mode an entry without a leading `/` or glob is anchored at the root, so `assets` means `/assets`, not any `assets` directory |
| `exclude` | `[]` | Directories to exclude from sparse checkout (forces `--no-cone` mode) |
| `exclude_lfs` | `[]` | LFS patterns to skip downloading |
| `include_lfs` | `[]` | LFS patterns to download (if set, only these are fetched) |
| `sparse_cone` | `true` | Use cone mode for sparse checkout (faster, directory-based) |
| `submodules.shallow` | `true` | **Clone-time only.** Depth of the *initial* submodule fetch, done by `clone` or the first `carve`. In shared mode later worksets reuse those objects, so there is nothing left to shallow — it only applies again if a workset pins a commit the store does not have yet |
| `submodules.skip` | `[]` | Submodule paths to skip entirely |

Repo-wide settings, top-level:

| Field | Default | Description |
|-------|---------|-------------|
| `version` | `1` | Config schema version. A git-workset that does not know the version refuses the file |
| `submodules.sharing` | `"shared"` | `"shared"`: all worksets check out the same submodule object store. `"isolated"`: every workset clones its own copy (pre-0.4 behaviour) |

Unknown keys are an error, so a typo such as `incldue` or a `skip` at the wrong
level fails loudly instead of silently doing nothing.

`[submodules]` is deliberately repo-wide rather than per-profile: a profile
describes *which files you want*, while the object-store layout is a property of
the clone. Two profiles disagreeing about the layout of the same submodule
gitdir is not a meaningful thing to express.

Precedence for the sharing mode, highest first:

1. `--shared-submodules` / `--isolated-submodules` on the command line
2. the mode already recorded in the worktree (so `sync`/`switch` never convert one silently)
3. `git config workset.submoduleSharing` (local beats global — the escape hatch when you can't edit a committed file)
4. `[submodules] sharing` in `.git-workset.toml`
5. the default, `shared`

### Composing profiles

`server+art` checks out the union of what `server` and `art` each check out:

- If any part is a full-tree profile (no `include` and no `exclude`), the result is the full tree.
- Includes are unioned. A part with only excludes includes everything else.
- An exclude in one part never removes what another part includes: it is dropped
  if another part covers the whole path, and narrower includes under it are kept.
- Cone mode is used only if every part allows it and nothing is excluded.

LFS patterns and `submodules.skip` are unioned; `submodules.shallow` is on if any part wants it.

## Commands

### Using an external config (`-f` / `--config`)

Every command below accepts a global `-f <path>` (or `--config <path>`) to read worksets from an external TOML file instead of the repo's committed `.git-workset.toml`. `-f -` reads the config from stdin. Useful when you work across many similar repos that haven't adopted worksets yet — keep a personal config and apply it everywhere:

```bash
git workset -f ~/worksets/unreal-engine.toml clone <url> game-a --workset engine-only
git workset -f ~/worksets/unreal-engine.toml carve ../game-a-feature -w engine-only
```

When `-f` is set and the repo also has a committed `.git-workset.toml`, the external file wins silently. For `clone`, `-f` also skips the remote probe entirely (faster).

Once others adopt worksets, commit the config and drop the flag.

### Choosing a submodule layout (`--shared-submodules` / `--isolated-submodules`)

Both are global flags and apply to `clone`, `carve`, `sync`, and `switch`. They
are mutually exclusive and beat every other source of the setting:

```bash
git workset carve ../feature -w server --isolated-submodules   # private clone
git workset sync --shared-submodules                           # migrate to the shared store
```

The mode is recorded on the worktree at carve time, so a later `sync` or
`switch` keeps it rather than converting the worktree underneath you. See
[Submodules and worksets](#submodules-and-worksets).

### `git workset clone <url> <path> --workset <name>`

Clones a repo from scratch with only the workset's files. Sparse checkout is configured *before* the first checkout, so git never iterates the full tree through smudge filters — this matters in large repos with tens of thousands of files.

The flow: probes the remote for `.git-workset.toml`, then does `git init` → sparse checkout → `git fetch` → `git checkout` so only workset files are ever materialized.

Options:
- `--branch <branch>` — branch to clone (default: remote HEAD)
- `--shallow` — clone with depth 1 (minimal history)
- `--depth <n>` — clone with specific history depth

### `git workset init`

Creates a `.git-workset.toml` template in the current repo.

### `git workset carve <path> [<commit-ish>] --workset <name>`

Creates a new worktree and applies a workset profile. This:

1. Creates the worktree with `GIT_LFS_SKIP_SMUDGE=1` (instant, no large file downloads)
2. Enables worktree-scoped config (`extensions.worktreeConfig`) so all settings are isolated from the main repo
3. Applies sparse checkout to include only the configured directories
4. Attaches submodules to the main clone's shared object stores (no re-clone, no network), skipping excluded ones and marking them inactive
5. Configures LFS filters and pulls only matching files

Use `+` to compose profiles: `--workset server+art` unions both profiles.

Options:
- `-b <name>` — create a new branch (fails if it already exists)
- `-B <name>` — create or reset a branch (force-creates even if it exists)
- `<commit-ish>` — the branch/commit to check out, or the start point when used with `-b`/`-B` (default: HEAD)

If neither `-b`/`-B` nor `<commit-ish>` is given, git auto-creates a branch named after the path basename.

```sh
# New branch from HEAD
git workset carve ../my-feature -b my-feature --workset server

# New branch from a specific commit
git workset carve ../hotfix -b hotfix v2.0 --workset server

# Check out an existing branch
git workset carve ../my-feature existing-branch --workset server

# Auto-name the branch after the directory ("my-feature")
git workset carve ../my-feature --workset server

# Force-reset an existing branch to HEAD
git workset carve ../retry -B stale-branch --workset server
```

### `git workset sync`

Re-applies the active workset profile to the current worktree. Run this after editing `.git-workset.toml` to pick up changes.

`sync` and `switch` read the current worktree's own `.git-workset.toml` (falling
back to the one committed at its `HEAD`), so a worktree on a branch with
different profiles uses that branch's definitions, not the main worktree's.

### `git workset switch <name>`

Switches the current worktree to a different workset profile in-place, without recreating the worktree.

The sparse patterns are replaced, so directories added by hand with
`git sparse-checkout add` are dropped; `switch` and `sync` name them in a warning
so you can re-add them. Switching to a profile that no longer skips a submodule
clears the `active=false` the earlier profile wrote and checks it out.

### `git workset list`

Shows all worktrees and their active workset profiles.

### `git workset remove <path>`

Removes a worktree. Submodule checkouts are detached from the shared object
store first — plain `git worktree remove` refuses outright on any worktree that
contains submodules — and both the superproject and submodule worktree
registries are pruned afterwards.

Options:
- `--force` — remove even if the worktree has local modifications

### `git workset doctor [--fix]`

Checks the repo's submodule plumbing and reports what it finds. Read-only by
default (exit 1 if anything is wrong); `--fix` applies the repairs and exits 0.

| Check | What it means |
|-------|---------------|
| **D1** | A submodule's `core.worktree` still lives in its shared config while several checkouts use it |
| **D2** | A checkout's effective `core.worktree` points somewhere else — the damage v0.3.x left on every carve |
| **D3** | Orphaned worktree registrations, e.g. after `rm -rf`ing a workset |
| **D4** | A gitdir/worktree link no longer resolves (the tree moved) |
| **D5** | Duplicate submodule object stores that shared mode could reclaim |
| **D6** | git is older than 2.20, which cannot do shared stores |

If you used git-workset before 0.4.0, run `git workset doctor --fix` once: v0.3.x
rewrote `core.worktree` in the shared submodule config on every carve, which can
leave `git status` in the main clone failing outright.

### `git workset deepen [--by <n>]`

Fetches more history for a shallow clone. Useful when you need `git blame` or `git log` beyond the shallow depth. Omit `--by` to fetch full history.

## Using git-workset from CI and sandboxes

`apply` and `profiles` are for tools that already own a checkout — a CI job, a
container build, an agent sandbox — and want git-workset's profile semantics
without anything else.

### `git workset apply <profile>`

Applies a profile's sparse patterns to an existing checkout, and by default does
nothing else: it never fetches, clones, creates branches, probes remotes or
writes global config, and works with no remote named `origin`. The only network
I/O it can cause is git's own lazy blob fetch through a promisor remote on a
partial clone.

It can run before the first checkout, so only the cone is ever written:

```sh
git clone --no-checkout --filter=blob:none "$URL" repo
git workset apply server -C repo --rev "$SHA" --json > workset.json
git -C repo checkout --detach "$SHA"
```

Options:
- `-C <path>` — operate on that checkout instead of the current directory
- `--rev <commit-ish>` — read `.git-workset.toml` from that commit. Without it
  (and without `-f`) the config comes from the target's `HEAD` — never the
  working-tree file or the main worktree's
- `--submodules=report|ignore|manage` (default `report`)
  - `report`: touch nothing, write no `submodule.*` config, and report each
    `.gitmodules` entry as `in_cone`, `out_of_cone` or `skipped`
  - `ignore`: touch nothing, report nothing
  - `manage`: what `switch` does — clone wanted submodules, mark skipped ones inactive
- `--lfs=report|configure|pull|ignore` (default `report`)
  - `report`: write nothing, report the resolved patterns. Does not need `git-lfs`
  - `configure`: write `lfs.fetchinclude`/`lfs.fetchexclude`, do not pull
  - `pull`: what `switch` does
- `--no-marker` — do not record the active profile in the worktree's git dir

In the default modes `apply` writes only what `git sparse-checkout` writes
(`core.sparseCheckout`, `core.sparseCheckoutCone`, the `info/sparse-checkout`
file, `extensions.worktreeConfig`) plus the `workset` marker.

With `--json`, stdout carries exactly one document:

```json
{
  "schema": "git-workset/apply@1",
  "profile": "server+tools",
  "profiles": ["server", "tools"],
  "config_source": { "kind": "rev", "rev": "3f2a…", "path": ".git-workset.toml" },
  "sparse": { "enabled": true, "cone": true, "patterns": ["src/server", "src/shared", "tools"] },
  "submodules": [
    { "name": "ext/lib", "path": "ext/lib", "state": "in_cone" },
    { "name": "third_party/art", "path": "third_party/art", "state": "skipped" },
    { "name": "docs/theme", "path": "docs/theme", "state": "out_of_cone" }
  ],
  "lfs": { "include": ["*.json"], "exclude": ["*.psd"] }
}
```

Submodule states:
- `skipped` — listed in `submodules.skip`. This wins over the cone. It is advice:
  nothing is written, so `git submodule update --init -- <path>` still works later.
  Entries are normalised, so `./ext/lib` and `ext/lib/` both match `ext/lib`
- `in_cone` / `out_of_cone` — decided by git's own rules
  (`git sparse-checkout check-rules`, git 2.42+) against the gitlink path
- `unknown` — git is older than 2.42 and the profile is no-cone, so the answer
  cannot be computed exactly. Cone-mode profiles are always decided
- Only top-level submodules are listed; nested ones are not visible until their
  parent is cloned
- With `--submodules=ignore` the `submodules` field is absent rather than empty,
  and with `--lfs=ignore` so is `lfs`

### `git workset profiles`

Lists the profiles a config defines, in file order, with their fields. Needs no
repository when given `-f`:

```sh
git workset profiles -f .git-workset.toml --json
curl -s "$FORGE/raw/.git-workset.toml" | git workset profiles -f - --json
```

```json
{
  "schema": "git-workset/profiles@1",
  "config_version": 1,
  "profiles": [
    { "name": "server", "description": "Backend server development",
      "include": ["src/server", "src/shared"], "exclude": [], "sparse_cone": true,
      "include_lfs": ["*.json"], "exclude_lfs": ["*.psd"],
      "submodules": { "skip": ["third_party/art-pipeline"], "shallow": true } }
  ]
}
```

Options: `-C <path>`, `--rev <commit-ish>`, as for `apply`.

### `--json`, errors and exit codes

`--json` is a global flag. With it, stdout carries exactly one JSON document —
on success or failure — and git's own output always goes to stderr. Every
document has a `schema` field, `git-workset/<kind>@<major>`; adding fields is
not a breaking change, a new major is.

On failure the document is:

```json
{
  "schema": "git-workset/error@1",
  "code": "unknown_profile",
  "message": "Workset 'srever' not found. Available: client, server",
  "details": { "requested": "srever", "available": ["client", "server"] }
}
```

| Exit | `code` | Meaning |
|------|--------|---------|
| 0 | — | success |
| 1 | `failed` | a git command or filesystem operation failed |
| 2 | — | bad flags. Rejected before anything runs: the message is on stderr and stdout is empty |
| 3 | `config_missing` | no `.git-workset.toml` at the requested source: the commit's tree has none, the file does not exist, `--rev` is not a commit, or `-f -` received empty input |
| 4 | `config_invalid` | TOML syntax error, unknown key, wrong type, or unsupported `version` |
| 5 | `unknown_profile` | a requested profile, or one part of `a+b`, does not exist |
| 6 | `fetch_failed` | the file is in the commit but could not be read: its blob is not local and fetching it from the promisor remote failed. Retry, or fix the remote |

The exit codes apply with or without `--json`.

`message` is always a single line and never contains git output or config
text. Those go in `details`: `git_stderr` (for `fetch_failed`), `parse_error`
(for `config_invalid`) and `cause` (for `failed`). Treat `details` as
untrusted — git stderr can carry remote URLs, and parse errors quote the
config — and do not log it where those would be a problem.

A missing `.gitmodules` gives `"submodules": []`; one that is in the commit but
cannot be read is `fetch_failed`, never an empty list.

## How it works

Under the hood, `git workset` orchestrates standard git primitives:

- **Sparse clone** (`git init` → `sparse-checkout` → `fetch` → `checkout`) — configures sparse checkout before any checkout happens, avoiding full-tree iteration through smudge filters
- **Sparse checkout** (`git sparse-checkout`) — each worktree gets its own sparse-checkout config
- **Worktree-scoped config** (`git config --worktree`) — all settings (LFS filters, submodule active flags) are isolated per-worktree so the main repo is unaffected
- **Shared submodule object stores** (`git worktree add` inside the submodule) — a workset's submodule checkout is a *worktree* of the main clone's submodule gitdir, not a fresh clone. N submodules across M worksets cost N object stores instead of N×M, and carving does no submodule network I/O at all because the objects are already local. Skipped submodules are marked `active=false` so `git fetch` won't try to access them
- **LFS filters** (`lfs.fetchinclude` / `lfs.fetchexclude`) — download only the assets you need
- **Per-worktree submodule config** (`extensions.worktreeConfig`) — `core.worktree` is moved out of the submodule's shared config into per-worktree config, so a stray `git submodule update` in one workset cannot redirect the others
- **Worktree metadata** — the active workset name is stored in `.git/worktrees/<name>/workset`

## Submodules and worksets

By default every workset shares one object store per submodule. That is a large
win on disk and on carve time, and it comes with a small contract:

1. **Use `git workset` to create, remove, and move worksets.** `carve` attaches
   the submodule checkouts, `remove` detaches them. `rm -rf`ing a workset leaves
   orphaned registrations behind in *each* submodule; `git workset doctor --fix`
   cleans them up.
2. **Use plain git for everything else.** Committing, branching, fetching,
   `status`, `diff`, `log` inside a submodule all work normally and are safe.
   Even a bare `git submodule update` is safe — the per-worktree `core.worktree`
   shadows whatever it writes into the shared config.
3. **One branch per submodule across all worksets.** Two worktrees of the same
   repository cannot have the same branch checked out, and a submodule's
   checkouts are worktrees of one repository:

   ```
   fatal: 'master' is already used by worktree at '/repo/ws1/ext/lib'
   ```

   Worksets check submodules out detached at the pinned commit, so this only
   surfaces when you `git checkout <branch>` inside a submodule. Do submodule
   branch work in one designated workset (or the main clone) and let the others
   stay detached.

If a submodule can't be shared — git older than 2.20, or `git worktree add`
failing for any reason — git-workset prints a one-line warning and falls back to
an isolated clone for *that submodule only*. Nine well-behaved submodules still
get shared when the tenth is awkward.

To opt a repo out entirely, set `[submodules] sharing = "isolated"` in
`.git-workset.toml`, or pass `--isolated-submodules`.

### Migrating an existing repo

There is no automatic migration on upgrade. Both paths are explicit:

```sh
git workset doctor --fix                    # repair core.worktree damage from v0.3.x
git workset sync --shared-submodules        # in a worktree: drop its duplicate store
```

`sync` refuses to migrate a submodule with uncommitted changes, and names it.

## License

MIT
