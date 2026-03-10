# git-workset

Named sparse-checkout profiles for git worktrees. Like Perforce stream filters, but for git.

Create lightweight worktrees that only check out the directories you need, with shallow submodules and selective LFS downloads — all driven by a single `.git-workset.toml` config.

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

| Field | Default | Description |
|-------|---------|-------------|
| `description` | — | Human-readable profile description |
| `include` | `[]` | Directories to include in sparse checkout (empty = full tree) |
| `exclude` | `[]` | Directories to exclude from sparse checkout (forces `--no-cone` mode) |
| `exclude_lfs` | `[]` | LFS patterns to skip downloading |
| `include_lfs` | `[]` | LFS patterns to download (if set, only these are fetched) |
| `sparse_cone` | `true` | Use cone mode for sparse checkout (faster, directory-based) |
| `submodules.shallow` | `true` | Clone submodules with `--depth 1` |
| `submodules.skip` | `[]` | Submodule paths to skip entirely |

## Commands

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
4. Initializes submodules (shallow, skipping excluded ones) and marks skipped submodules as inactive
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

Removes a worktree.

### `git workset deepen [--by <n>]`

Fetches more history for a shallow clone. Useful when you need `git blame` or `git log` beyond the shallow depth. Omit `--by` to fetch full history.

## How it works

Under the hood, `git workset` orchestrates standard git primitives:

- **Sparse clone** (`git init` → `sparse-checkout` → `fetch` → `checkout`) — configures sparse checkout before any checkout happens, avoiding full-tree iteration through smudge filters
- **Sparse checkout** (`git sparse-checkout`) — each worktree gets its own sparse-checkout config
- **Worktree-scoped config** (`git config --worktree`) — all settings (LFS filters, submodule active flags) are isolated per-worktree so the main repo is unaffected
- **Shallow submodules** (`git submodule update --depth 1`) — just the pinned commit, no history; skipped submodules are marked `active=false` so `git fetch` won't try to access them
- **LFS filters** (`lfs.fetchinclude` / `lfs.fetchexclude`) — download only the assets you need
- **Worktree metadata** — the active workset name is stored in `.git/worktrees/<name>/workset`

## License

MIT
