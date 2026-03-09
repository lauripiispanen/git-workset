# Releasing git-workset

Releases are automated via GitHub Actions. When a version tag is pushed, the
release workflow builds binaries for all supported platforms and publishes a
GitHub Release with the artifacts attached.

## Supported platforms

| Target                      | OS            | Architecture |
|-----------------------------|---------------|--------------|
| `x86_64-unknown-linux-gnu`  | Linux         | x86_64       |
| `aarch64-unknown-linux-gnu` | Linux         | ARM64        |
| `x86_64-pc-windows-msvc`    | Windows       | x86_64       |
| `aarch64-pc-windows-msvc`   | Windows       | ARM64        |
| `x86_64-apple-darwin`       | macOS         | x86_64       |
| `aarch64-apple-darwin`      | macOS         | ARM64        |

## How to release

1. Make sure all changes are merged to `main` and CI is green.

2. Update the version in `Cargo.toml` if needed.

3. Create and push a version tag:

```sh
git tag v0.2.0
git push origin v0.2.0
```

GitHub Actions builds and publishes the release automatically.

## What happens

- The **test** job runs `cargo test` to verify the code.
- The **build** job compiles release binaries for all six targets in parallel.
  - Linux ARM64 uses [cross](https://github.com/cross-rs/cross) for cross-compilation.
  - All other targets build natively on their respective runners.
- Archives are created: `.tar.gz` for Unix targets, `.zip` for Windows.
- SHA-256 checksums are generated for every archive.
- The **release** job collects all artifacts and creates a GitHub Release with
  the tag name, auto-generated release notes, and all binaries attached.

## Verifying a download

After downloading a binary, verify its checksum:

```sh
# Unix
shasum -a 256 -c git-workset-x86_64-unknown-linux-gnu.tar.gz.sha256

# Or manually
shasum -a 256 git-workset-x86_64-unknown-linux-gnu.tar.gz
# Compare with the contents of the .sha256 file
```
