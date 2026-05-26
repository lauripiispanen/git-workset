# Agent notes

## Before committing

```sh
cargo fmt
cargo clippy -- -D warnings
cargo test
```

## After changing CLI interface

- Update README.md command docs and examples to match
- Add or update integration tests in `tests/integration.rs`

## Commit messages

Release notes are auto-generated from commit subjects between tags. Write clear, meaningful commit messages — they become user-facing release notes. Prefix internal changes (CI, tooling, docs-only) with `chore:` to exclude them from release notes.

## Release flow

1. Bump `version` in `Cargo.toml` (use semantic versioning — include the bump **in** the feature/fix commit, not as a separate or amended commit, so unreleased commits never pile up unversioned)
2. Tag (annotated): `git tag -a v0.x.y -m "v0.x.y"`
3. Push main, wait for CI to go green: `git push origin main`
4. Push tag, wait for release workflow to go green: `git push origin v0.x.y`
   ```sh
   gh run watch $(gh run list --workflow Release --limit 1 --json databaseId -q '.[0].databaseId') --repo lauripiispanen/git-workset
   ```

The release workflow (`.github/workflows/release.yml`) automatically:
- Builds binaries for all 6 targets
- Generates release notes from commit subjects (filters `chore:`/`ci:`/`docs:`/`test:`)
- Publishes the GitHub Release with binaries + `.sha256` files
- Deploys the marketing page to GitHub Pages
- Updates the Homebrew tap (`lauripiispanen/homebrew-tap`) — no manual step needed
