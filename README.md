# git-workset

Named sparse-checkout profiles for git worktrees. Like Perforce stream filters, but for git.

Create lightweight worktrees that only check out the directories you need, with shallow submodules and selective LFS downloads — all driven by a single `.git-workset.toml` config.

## Install

```sh
cargo install --path .
```

This installs a `git-workset` binary. Git automatically discovers it as a subcommand, so you can use `git workset` directly.

## Quick start

```sh
# In your repo root, create a config template
git workset init

# Edit .git-workset.toml to define your profiles (see below)

# Create a worktree with a profile applied
git workset carve ../feature-branch feature-branch --workset server

# Compose multiple profiles
git workset carve ../fix main --workset server+art
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
| `include` | — | Directories to include in sparse checkout |
| `exclude_lfs` | `[]` | LFS patterns to skip downloading |
| `include_lfs` | `[]` | LFS patterns to download (if set, only these are fetched) |
| `sparse_cone` | `true` | Use cone mode for sparse checkout (faster, directory-based) |
| `submodules.shallow` | `true` | Clone submodules with `--depth 1` |
| `submodules.skip` | `[]` | Submodule paths to skip entirely |

## Commands

### `git workset init`

Creates a `.git-workset.toml` template in the current repo.

### `git workset carve <path> <branch> --workset <name>`

Creates a new worktree and applies a workset profile. This:

1. Creates the worktree with `GIT_LFS_SKIP_SMUDGE=1` (instant, no large file downloads)
2. Enables worktree-scoped config (`extensions.worktreeConfig`) so all settings are isolated from the main repo
3. Applies sparse checkout to include only the configured directories
4. Initializes submodules (shallow, skipping excluded ones) and marks skipped submodules as inactive
5. Configures LFS filters and pulls only matching files

Use `+` to compose profiles: `--workset server+art` unions both profiles.

### `git workset sync`

Re-applies the active workset profile to the current worktree. Run this after editing `.git-workset.toml` to pick up changes.

### `git workset switch <name>`

Switches the current worktree to a different workset profile in-place, without recreating the worktree.

### `git workset list`

Shows all worktrees and their active workset profiles.

### `git workset remove <path>`

Removes a worktree.

## How it works

Under the hood, `git workset` orchestrates standard git primitives:

- **Sparse checkout** (`git sparse-checkout`) — each worktree gets its own sparse-checkout config
- **Worktree-scoped config** (`git config --worktree`) — all settings (LFS filters, submodule active flags) are isolated per-worktree so the main repo is unaffected
- **Shallow submodules** (`git submodule update --depth 1`) — just the pinned commit, no history; skipped submodules are marked `active=false` so `git fetch` won't try to access them
- **LFS filters** (`lfs.fetchinclude` / `lfs.fetchexclude`) — download only the assets you need
- **Worktree metadata** — the active workset name is stored in `.git/worktrees/<name>/workset`

## License

MIT
