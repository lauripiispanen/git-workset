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

1. Bump `version` in `Cargo.toml`
2. Commit and push to main
3. Tag (annotated) and push:
   ```sh
   git tag -a v0.x.y -m "v0.x.y"
   git push origin main v0.x.y
   ```
4. Wait for the release workflow to complete:
   ```sh
   gh run watch $(gh run list --workflow Release --limit 1 --json databaseId -q '.[0].databaseId') --repo lauripiispanen/git-workset
   ```
5. Update the Homebrew tap (`lauripiispanen/homebrew-tap`):
   - Get sha256s from release assets (`.sha256` files)
   - Update `version` and all `sha256` values in `Formula/git-workset.rb`
   - Commit and push to the tap repo
