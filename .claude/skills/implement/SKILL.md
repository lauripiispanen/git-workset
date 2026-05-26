---
name: implement
description: Standard implementation workflow for this repo — code, test, lint, push, verify CI. Use after the plan is agreed and you're about to write code.
---

# Implementation Workflow

Follow these steps in order. Don't skip steps to "save time" — each one catches a class of bugs the others miss.

## 1. Write the code

Make the changes per the agreed plan.

## 2. Add integration tests

Tests live in `tests/integration.rs`. They must be **substantive and actionable**:

- Test observable behavior, not implementation details. A test that just asserts "function was called" is worthless.
- Each test must measure something a user would notice if it broke — exit codes, files on disk, git state, stdout/stderr content.
- If a test passes whether or not the feature works, delete it and write a better one.
- Cover the golden path AND at least one realistic failure mode (missing file, bad input, conflicting flags).
- For CLI features, prefer end-to-end tests that invoke the binary, not unit tests of internal helpers.

## 3. Run the full test suite

```bash
cargo test
```

All tests must pass. If a pre-existing test breaks, fix it — don't disable it.

## 4. Lint and format

```bash
cargo clippy --all-targets --all-features -- -D warnings
cargo fmt --all -- --check
```

Both must be clean. If `fmt --check` fails, run `cargo fmt --all` and re-run tests.

## 5. Commit and push

Only after steps 2–4 are clean:

```bash
git add <specific files>
git commit -m "..."
git push origin main
```

Use conventional commit prefixes (`feat:`, `fix:`, `chore:`, etc.) — match the style of recent commits in `git log`.

## 6. Watch GitHub Actions

Multi-platform CI must pass before the task is done.

```bash
gh run watch
```

Or list and follow the latest run:

```bash
gh run list --limit 1
gh run view <id> --log-failed   # if any job failed
```

Do **not** declare the task complete until the run is green across all platforms.

## 7. If CI fails

1. Read the failing job's logs (`gh run view <id> --log-failed`).
2. Reproduce locally if possible.
3. Fix the issue.
4. Re-run steps 3–4 (tests + clippy + fmt) locally.
5. **Amend the commit** (don't pile on fixup commits):
   ```bash
   git add <files>
   git commit --amend --no-edit
   git push --force-with-lease origin main
   ```
6. Watch CI again. Repeat until green.

## Out of scope

This skill does not cover releasing, tagging, or publishing. A separate `release` skill handles that.
