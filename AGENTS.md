# Agent notes

## Before committing

```sh
cargo fmt
cargo clippy -- -D warnings
cargo test
```

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
