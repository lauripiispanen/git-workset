---
name: release
description: Cut a new release of git-workset — pick the version, tag, push, and verify the automated workflow. Use when the user asks to release, ship, or cut a version.
---

# Release Workflow

The release is fully automated once you push a `v*.*.*` tag: `.github/workflows/release.yml` builds binaries for six targets, generates release notes, publishes the GitHub Release, deploys the marketing page, and updates the Homebrew tap. Your job is to bump the version, tag correctly, and verify the workflow finished cleanly.

## 1. Verify the starting state

```bash
git fetch --tags origin
git status                          # clean working tree, on main
git log --oneline $(git describe --tags --abbrev=0)..HEAD
```

The log above shows what's going into the release. If it's empty, there's nothing to release.

Also confirm the latest CI run on `main` is green — if `main` is broken, don't release from it.

## 2. Pick the version (semver)

Read the unreleased commits from step 1 and pick the smallest correct bump:

- Any `feat:` commits → **minor** bump (0.x.0 → 0.(x+1).0)
- Only `fix:`/`perf:`/etc. → **patch** bump (0.x.y → 0.x.(y+1))
- Any breaking change → **major** bump (this project is pre-1.0, so call this out explicitly to the user before doing it)

If unsure, state your reasoning and the proposed version, then proceed.

## 3. Bump `Cargo.toml` and regenerate `Cargo.lock`

```bash
# Edit Cargo.toml: version = "0.X.Y"
cargo check                         # regenerates Cargo.lock to match
```

Both files must be staged together — a mismatched lockfile causes CI noise.

## 4. Commit the bump

Ideally the version bump rides along with the `feat:`/`fix:` commit that introduces the change (see `AGENTS.md`). When releasing after-the-fact (commits already pushed), use a separate commit:

```bash
git commit -m "chore: bump version to 0.X.Y"
```

The release-notes generator filters `chore:` lines, so this commit won't pollute the notes.

If you're also tweaking release tooling (workflow files, this skill, AGENTS.md) bundle those into the same `chore:` commit — it keeps history tidy.

## 5. Push main and wait for CI

```bash
git push origin main
gh run watch
```

CI must be green across all platforms (ubuntu, macOS, Windows) before tagging. Don't tag a red commit.

## 6. Create the annotated tag and push it

```bash
git tag -a v0.X.Y -m "v0.X.Y"
git push origin v0.X.Y
```

Annotated (`-a`), not lightweight — the workflow and tooling expect tags with metadata.

Pushing the tag triggers `.github/workflows/release.yml`.

## 7. Watch the release workflow

```bash
gh run watch $(gh run list --workflow Release --limit 1 --json databaseId -q '.[0].databaseId') \
  --repo lauripiispanen/git-workset
```

The workflow has these jobs (in dependency order):

| Job | What it does |
|-----|--------------|
| `test` | Runs `cargo test` on ubuntu |
| `build` (×6) | Compiles for linux x86/arm, windows x86/arm, macOS x86/arm; produces `.tar.gz`/`.zip` + `.sha256` |
| `release` | Collects artifacts, generates notes from `<prev-tag>..<new-tag>` (filters `chore:`/`ci:`/`docs:`/`test:`/bump lines), publishes the GitHub Release |
| `deploy-pages` | Stamps the version into the marketing page and pushes to `gh-pages` |
| `deploy-homebrew` | Renders `Formula/git-workset.rb` with the six sha256s and version, pushes to `lauripiispanen/homebrew-tap` |

All must be green. The Homebrew tap update is **automatic** — do not do it manually.

The historically-fragile target is `aarch64-unknown-linux-gnu` (uses `cross`). If a build fails, see step 9.

## 8. Verify the release

After the workflow goes green:

```bash
gh release view v0.X.Y --repo lauripiispanen/git-workset
```

Check:
- All six binaries + six `.sha256` files are attached
- Release notes contain the expected `feat:`/`fix:` lines, no `chore:`/`ci:`/`docs:`/`test:` noise
- Tap formula updated: `gh api repos/lauripiispanen/homebrew-tap/commits/main -q .commit.message` should mention the new tag

Optionally smoke-test:
```bash
brew update && brew upgrade git-workset
git workset --version
```

## 9. If something fails

The tag is what triggers the workflow, so recovering means deleting the tag, fixing the issue, and retagging:

```bash
git push --delete origin v0.X.Y
git tag -d v0.X.Y
# fix the issue, commit, push to main, wait for CI
git tag -a v0.X.Y -m "v0.X.Y"
git push origin v0.X.Y
```

If a GitHub Release was already created before the failure, delete it too:
```bash
gh release delete v0.X.Y --yes
```

Don't try to "rerun the workflow" from the GitHub UI without deleting the tag first — `softprops/action-gh-release` will refuse to overwrite an existing release.

## Out of scope

This skill does not handle code changes or feature work. Use the `implement` skill for that — it includes a version-bump step so that, ideally, by the time you reach `release` the version is already correct and step 3 is a verification rather than an edit.
