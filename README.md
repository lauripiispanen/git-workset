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
| `include` | `[]` | Directories to include in sparse checkout (empty = full tree) |
| `exclude` | `[]` | Directories to exclude from sparse checkout (forces `--no-cone` mode) |
| `exclude_lfs` | `[]` | LFS patterns to skip downloading |
| `include_lfs` | `[]` | LFS patterns to download (if set, only these are fetched) |
| `sparse_cone` | `true` | Use cone mode for sparse checkout (faster, directory-based) |
| `submodules.shallow` | `true` | **Clone-time only.** Depth of the *initial* submodule fetch, done by `clone` or the first `carve`. In shared mode later worksets reuse those objects, so there is nothing left to shallow — it only applies again if a workset pins a commit the store does not have yet |
| `submodules.skip` | `[]` | Submodule paths to skip entirely |

Repo-wide settings, top-level:

| Field | Default | Description |
|-------|---------|-------------|
| `submodules.sharing` | `"shared"` | `"shared"`: all worksets check out the same submodule object store. `"isolated"`: every workset clones its own copy (pre-0.4 behaviour) |

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

## Commands

### Using an external config (`-f` / `--config`)

Every command below accepts a global `-f <path>` (or `--config <path>`) to read worksets from an external TOML file instead of the repo's committed `.git-workset.toml`. Useful when you work across many similar repos that haven't adopted worksets yet — keep a personal config and apply it everywhere:

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

### `git workset switch <name>`

Switches the current worktree to a different workset profile in-place, without recreating the worktree.

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
