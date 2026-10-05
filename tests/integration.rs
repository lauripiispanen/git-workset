use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use tempfile::TempDir;

/// Return the path to the compiled binary.
fn git_workset_bin() -> PathBuf {
    // cargo test sets this env var pointing to the deps directory;
    // the binary lives next to it.
    let mut path = std::env::current_exe()
        .expect("current_exe")
        .parent()
        .expect("parent of test exe")
        .parent()
        .expect("parent of deps dir")
        .to_path_buf();
    path.push("git-workset");
    path
}

/// Run the git-workset binary with the given args, inheriting the given cwd.
fn run_workset(args: &[&str], cwd: &Path) -> Output {
    Command::new(git_workset_bin())
        .args(args)
        .current_dir(cwd)
        .env("GIT_LFS_SKIP_SMUDGE", "1")
        .env("GIT_CONFIG_COUNT", "1")
        .env("GIT_CONFIG_KEY_0", "protocol.file.allow")
        .env("GIT_CONFIG_VALUE_0", "always")
        .output()
        .expect("failed to execute git-workset")
}

/// Run a raw git command in a directory.
fn run_git(args: &[&str], cwd: &Path) -> Output {
    Command::new("git")
        .args(args)
        .current_dir(cwd)
        .env("GIT_LFS_SKIP_SMUDGE", "1")
        .env("GIT_CONFIG_COUNT", "1")
        .env("GIT_CONFIG_KEY_0", "protocol.file.allow")
        .env("GIT_CONFIG_VALUE_0", "always")
        .output()
        .expect("failed to execute git")
}

fn run_git_ok(args: &[&str], cwd: &Path) {
    let output = run_git(args, cwd);
    assert!(
        output.status.success(),
        "git {} failed in {}: {}",
        args.join(" "),
        cwd.display(),
        String::from_utf8_lossy(&output.stderr)
    );
}

fn stdout(output: &Output) -> String {
    String::from_utf8_lossy(&output.stdout).to_string()
}

fn stderr(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).to_string()
}

/// Create a bare-bones git repo with some directories and a .git-workset.toml.
/// Returns the TempDir (keep it alive!) and the path to the repo.
fn create_test_repo() -> (TempDir, PathBuf) {
    let dir = TempDir::new().expect("tempdir");
    let repo = dir.path().join("origin");
    std::fs::create_dir_all(&repo).unwrap();

    run_git_ok(&["init", "--initial-branch=main"], &repo);
    run_git_ok(&["config", "user.email", "test@test.com"], &repo);
    run_git_ok(&["config", "user.name", "Test"], &repo);

    // Create directory structure
    for subdir in &["src/server", "src/client", "src/shared", "assets"] {
        let p = repo.join(subdir);
        std::fs::create_dir_all(&p).unwrap();
        std::fs::write(p.join("hello.txt"), format!("file in {}", subdir)).unwrap();
    }

    // Create a small sub-repo to use as a submodule
    let sub_repo = dir.path().join("subrepo");
    std::fs::create_dir_all(&sub_repo).unwrap();
    run_git_ok(&["init", "--initial-branch=main"], &sub_repo);
    run_git_ok(&["config", "user.email", "test@test.com"], &sub_repo);
    run_git_ok(&["config", "user.name", "Test"], &sub_repo);
    std::fs::write(sub_repo.join("lib.txt"), "submodule content").unwrap();
    run_git_ok(&["add", "-A"], &sub_repo);
    run_git_ok(&["commit", "-m", "sub initial"], &sub_repo);

    // A second sub-repo, for multi-submodule cases
    let sub_repo2 = dir.path().join("subrepo2");
    std::fs::create_dir_all(&sub_repo2).unwrap();
    run_git_ok(&["init", "--initial-branch=main"], &sub_repo2);
    run_git_ok(&["config", "user.email", "test@test.com"], &sub_repo2);
    run_git_ok(&["config", "user.name", "Test"], &sub_repo2);
    std::fs::write(sub_repo2.join("lib2.txt"), "second submodule content").unwrap();
    run_git_ok(&["add", "-A"], &sub_repo2);
    run_git_ok(&["commit", "-m", "sub2 initial"], &sub_repo2);

    // Add them as submodules (using file:// URLs so they resolve locally)
    let sub_url = format!("file://{}", sub_repo.display());
    run_git_ok(&["submodule", "add", &sub_url, "ext/lib"], &repo);
    let sub_url2 = format!("file://{}", sub_repo2.display());
    run_git_ok(&["submodule", "add", &sub_url2, "ext/lib2"], &repo);

    // Create .git-workset.toml
    let config = r#"
[workset.backend]
description = "Backend services"
include = ["src/server", "src/shared"]

[workset.backend.submodules]
skip = ["ext/lib", "ext/lib2"]

[workset.frontend]
description = "Frontend client"
include = ["src/client", "src/shared"]

[workset.frontend.submodules]
skip = ["ext/lib", "ext/lib2"]

[workset.with-sub]
description = "Backend plus the ext/lib submodule"
include = ["src/server", "ext"]

[workset.with-sub.submodules]
skip = ["ext/lib2"]

[workset.with-subs]
description = "Backend plus both submodules"
include = ["src/server", "ext"]

[workset.all]
description = "Everything (explicit)"
include = ["src/server", "src/client", "src/shared", "assets", "ext"]

[workset.everything]
description = "Everything (empty includes = no sparse checkout)"
include = []

[workset.no-assets]
description = "Everything except assets"
exclude = ["assets"]
"#;
    std::fs::write(repo.join(".git-workset.toml"), config).unwrap();

    run_git_ok(&["add", "-A"], &repo);
    run_git_ok(&["commit", "-m", "initial commit"], &repo);

    (dir, repo)
}

// ---- Submodule sharing helpers ----

/// The shared gitdir backing a submodule in the main clone.
/// Submodule names default to their path, so `ext/lib` lives at
/// `.git/modules/ext/lib`.
fn subrepo_gitdir(main: &Path, sub_path: &str) -> PathBuf {
    main.join(".git").join("modules").join(sub_path)
}

/// Count distinct object stores that exist for one submodule: the shared one
/// under `.git/modules`, plus any per-worktree clones under
/// `.git/worktrees/*/modules`.
fn count_object_stores(main: &Path, sub_path: &str) -> usize {
    let mut count = 0;
    if subrepo_gitdir(main, sub_path).join("objects").exists() {
        count += 1;
    }
    if let Ok(entries) = std::fs::read_dir(main.join(".git").join("worktrees")) {
        for entry in entries.flatten() {
            if entry
                .path()
                .join("modules")
                .join(sub_path)
                .join("objects")
                .exists()
            {
                count += 1;
            }
        }
    }
    count
}

/// The `core.worktree` git will use for a checkout of `gitdir`. Pass the
/// linked worktree's id to read its per-worktree config, or `None` for the
/// main checkout.
fn effective_core_worktree(gitdir: &Path, worktree_id: Option<&str>) -> Option<String> {
    let shared = gitdir.join("config");
    let per_worktree = match worktree_id {
        Some(id) => gitdir.join("worktrees").join(id).join("config.worktree"),
        None => gitdir.join("config.worktree"),
    };
    let enabled = config_file_get(&shared, "extensions.worktreeConfig")
        .map(|v| v == "true")
        .unwrap_or(false);
    let own = if enabled {
        config_file_get(&per_worktree, "core.worktree")
    } else {
        None
    };
    own.or_else(|| config_file_get(&shared, "core.worktree"))
}

fn config_file_get(file: &Path, key: &str) -> Option<String> {
    if !file.exists() {
        return None;
    }
    let output = Command::new("git")
        .args(["config", "-f", file.to_str()?, "--get", key])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let value = String::from_utf8(output.stdout).ok()?.trim().to_string();
    if value.is_empty() {
        None
    } else {
        Some(value)
    }
}

/// The worktree id git assigned to a linked submodule checkout.
fn worktree_id(checkout: &Path) -> String {
    let out = run_git(&["rev-parse", "--absolute-git-dir"], checkout);
    assert!(
        out.status.success(),
        "rev-parse in {} failed: {}",
        checkout.display(),
        stderr(&out)
    );
    PathBuf::from(stdout(&out).trim())
        .file_name()
        .unwrap()
        .to_string_lossy()
        .to_string()
}

/// The workset-scoped git config value recorded in a worktree.
fn worktree_config(worktree: &Path, key: &str) -> Option<String> {
    let out = run_git(&["config", "--worktree", "--get", key], worktree);
    if !out.status.success() {
        return None;
    }
    let value = stdout(&out).trim().to_string();
    if value.is_empty() {
        None
    } else {
        Some(value)
    }
}

fn same_dir(a: &Path, b: &Path) -> bool {
    match (std::fs::canonicalize(a), std::fs::canonicalize(b)) {
        (Ok(x), Ok(y)) => x == y,
        _ => false,
    }
}

/// Rewrite the committed `.git-workset.toml` with an extra top-level table.
fn prepend_config(repo: &Path, extra: &str) {
    let path = repo.join(".git-workset.toml");
    let existing = std::fs::read_to_string(&path).unwrap();
    std::fs::write(&path, format!("{}\n{}", extra, existing)).unwrap();
    run_git_ok(&["add", ".git-workset.toml"], repo);
    run_git_ok(&["commit", "-m", "update workset config"], repo);
}

// ---- Tests ----

#[test]
fn test_init_creates_config() {
    let dir = TempDir::new().unwrap();
    let repo = dir.path().join("repo");
    std::fs::create_dir_all(&repo).unwrap();
    run_git_ok(&["init"], &repo);

    let output = run_workset(&["init"], &repo);
    assert!(output.status.success(), "init failed: {}", stderr(&output));

    let config_path = repo.join(".git-workset.toml");
    assert!(
        config_path.exists(),
        ".git-workset.toml should exist after init"
    );

    let content = std::fs::read_to_string(&config_path).unwrap();
    assert!(
        content.contains("[workset.all]"),
        "template should contain [workset.all]"
    );
}

#[test]
fn test_init_fails_if_config_exists() {
    let dir = TempDir::new().unwrap();
    let repo = dir.path().join("repo");
    std::fs::create_dir_all(&repo).unwrap();
    run_git_ok(&["init"], &repo);

    // First init succeeds
    let output = run_workset(&["init"], &repo);
    assert!(output.status.success());

    // Second init should fail
    let output = run_workset(&["init"], &repo);
    assert!(
        !output.status.success(),
        "init should fail when config already exists"
    );
    assert!(
        stderr(&output).contains("already exists"),
        "error should mention 'already exists'"
    );
}

#[test]
fn test_config_parsing_single_workset() {
    let (_dir, repo) = create_test_repo();
    let config_content = std::fs::read_to_string(repo.join(".git-workset.toml")).unwrap();
    let config: toml::Value = toml::from_str(&config_content).unwrap();

    let backend = &config["workset"]["backend"];
    assert_eq!(backend["description"].as_str().unwrap(), "Backend services");

    let includes: Vec<&str> = backend["include"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_str().unwrap())
        .collect();
    assert!(includes.contains(&"src/server"));
    assert!(includes.contains(&"src/shared"));
}

#[test]
fn test_config_parsing_composite_workset() {
    // Test the "+" composite workset logic by exercising get_workset through
    // the binary. We verify by carving with a composite and checking which
    // directories are present.
    let (_dir, repo) = create_test_repo();

    // Create a branch for the worktree
    run_git_ok(&["branch", "composite-test"], &repo);

    let wt_path = _dir.path().join("composite-wt");
    let output = run_workset(
        &[
            "carve",
            wt_path.to_str().unwrap(),
            "composite-test",
            "-w",
            "backend+frontend",
        ],
        &repo,
    );
    assert!(
        output.status.success(),
        "carve with composite workset failed: {}",
        stderr(&output)
    );

    // Composite of backend (src/server, src/shared) + frontend (src/client, src/shared)
    // should include all three directories.
    assert!(
        wt_path.join("src/server/hello.txt").exists(),
        "src/server should exist"
    );
    assert!(
        wt_path.join("src/client/hello.txt").exists(),
        "src/client should exist"
    );
    assert!(
        wt_path.join("src/shared/hello.txt").exists(),
        "src/shared should exist"
    );
    // assets should NOT be present
    assert!(
        !wt_path.join("assets/hello.txt").exists(),
        "assets should NOT exist in composite"
    );
}

#[test]
fn test_carve_creates_worktree_with_sparse_checkout() {
    let (_dir, repo) = create_test_repo();
    run_git_ok(&["branch", "feature-backend"], &repo);

    let wt_path = _dir.path().join("wt-backend");
    let output = run_workset(
        &[
            "carve",
            wt_path.to_str().unwrap(),
            "feature-backend",
            "-w",
            "backend",
        ],
        &repo,
    );
    assert!(output.status.success(), "carve failed: {}", stderr(&output));

    // The worktree should exist
    assert!(wt_path.exists(), "worktree directory should exist");

    // Backend workset includes src/server and src/shared
    assert!(
        wt_path.join("src/server/hello.txt").exists(),
        "src/server should be checked out"
    );
    assert!(
        wt_path.join("src/shared/hello.txt").exists(),
        "src/shared should be checked out"
    );

    // src/client should NOT be checked out (not in backend workset)
    assert!(
        !wt_path.join("src/client/hello.txt").exists(),
        "src/client should NOT be checked out in backend workset"
    );

    // assets should NOT be checked out
    assert!(
        !wt_path.join("assets/hello.txt").exists(),
        "assets should NOT be checked out in backend workset"
    );
}

#[test]
fn test_list_shows_worktrees() {
    let (_dir, repo) = create_test_repo();
    run_git_ok(&["branch", "list-test"], &repo);

    let wt_path = _dir.path().join("wt-list");
    let output = run_workset(
        &[
            "carve",
            wt_path.to_str().unwrap(),
            "list-test",
            "-w",
            "backend",
        ],
        &repo,
    );
    assert!(output.status.success(), "carve failed: {}", stderr(&output));

    // Run list from the main repo
    let output = run_workset(&["list"], &repo);
    assert!(output.status.success(), "list failed: {}", stderr(&output));

    let list_output = stdout(&output);
    // Should show the main worktree
    assert!(
        list_output.contains("main"),
        "list output should mention 'main' branch: {}",
        list_output
    );
    // Should show the carved worktree with its workset name
    assert!(
        list_output.contains("backend"),
        "list output should mention 'backend' workset: {}",
        list_output
    );
    assert!(
        list_output.contains("list-test"),
        "list output should mention 'list-test' branch: {}",
        list_output
    );
}

#[test]
fn test_switch_changes_workset() {
    let (_dir, repo) = create_test_repo();
    run_git_ok(&["branch", "switch-test"], &repo);

    let wt_path = _dir.path().join("wt-switch");
    let output = run_workset(
        &[
            "carve",
            wt_path.to_str().unwrap(),
            "switch-test",
            "-w",
            "backend",
        ],
        &repo,
    );
    assert!(output.status.success(), "carve failed: {}", stderr(&output));

    // Initially backend: has src/server, no src/client
    assert!(wt_path.join("src/server/hello.txt").exists());
    assert!(!wt_path.join("src/client/hello.txt").exists());

    // Switch to frontend
    let output = run_workset(&["switch", "frontend"], &wt_path);
    assert!(
        output.status.success(),
        "switch failed: {}",
        stderr(&output)
    );

    // After switching to frontend: has src/client, no src/server
    assert!(
        wt_path.join("src/client/hello.txt").exists(),
        "src/client should exist after switching to frontend"
    );
    assert!(
        wt_path.join("src/shared/hello.txt").exists(),
        "src/shared should exist after switching to frontend"
    );
    assert!(
        !wt_path.join("src/server/hello.txt").exists(),
        "src/server should NOT exist after switching to frontend"
    );

    // Verify the workset marker was updated
    let output = run_workset(&["list"], &wt_path);
    let list_output = stdout(&output);
    assert!(
        list_output.contains("frontend"),
        "list should show 'frontend' after switch: {}",
        list_output
    );
}

#[test]
fn test_remove_worktree() {
    let (_dir, repo) = create_test_repo();
    run_git_ok(&["branch", "remove-test"], &repo);

    let wt_path = _dir.path().join("wt-remove");
    let output = run_workset(
        &[
            "carve",
            wt_path.to_str().unwrap(),
            "remove-test",
            "-w",
            "backend",
        ],
        &repo,
    );
    assert!(output.status.success(), "carve failed: {}", stderr(&output));
    assert!(wt_path.exists(), "worktree should exist after carve");

    // Remove it
    let output = run_workset(&["remove", wt_path.to_str().unwrap()], &repo);
    assert!(
        output.status.success(),
        "remove failed: {}",
        stderr(&output)
    );

    // The worktree directory should be gone
    assert!(!wt_path.exists(), "worktree directory should be removed");

    // git worktree list should no longer show it
    let output = run_workset(&["list"], &repo);
    let list_output = stdout(&output);
    assert!(
        !list_output.contains("remove-test"),
        "list should not mention removed worktree: {}",
        list_output
    );
}

#[test]
fn test_clone_sparse() {
    let (_dir, repo) = create_test_repo();

    // Use file:// URL for local clone
    let repo_url = format!("file://{}", repo.display());
    let clone_path = _dir.path().join("cloned");

    let output = run_workset(
        &[
            "clone",
            &repo_url,
            clone_path.to_str().unwrap(),
            "-w",
            "backend",
            "-b",
            "main",
        ],
        _dir.path(),
    );
    assert!(output.status.success(), "clone failed: {}", stderr(&output));

    // The cloned repo should exist
    assert!(clone_path.exists(), "clone directory should exist");

    // Backend workset should have src/server and src/shared
    assert!(
        clone_path.join("src/server/hello.txt").exists(),
        "src/server should be checked out in clone"
    );
    assert!(
        clone_path.join("src/shared/hello.txt").exists(),
        "src/shared should be checked out in clone"
    );

    // src/client should NOT be checked out
    assert!(
        !clone_path.join("src/client/hello.txt").exists(),
        "src/client should NOT be checked out in backend clone"
    );

    // Verify workset marker is stored
    let output = run_workset(&["list"], &clone_path);
    let list_output = stdout(&output);
    assert!(
        list_output.contains("backend"),
        "list should show 'backend' workset in clone: {}",
        list_output
    );
}

#[test]
fn test_clone_shallow() {
    let (_dir, repo) = create_test_repo();

    // Add a second commit so we can verify shallow depth
    std::fs::write(repo.join("src/server/extra.txt"), "extra file").unwrap();
    run_git_ok(&["add", "-A"], &repo);
    run_git_ok(&["commit", "-m", "second commit"], &repo);

    let repo_url = format!("file://{}", repo.display());
    let clone_path = _dir.path().join("shallow-clone");

    let output = run_workset(
        &[
            "clone",
            &repo_url,
            clone_path.to_str().unwrap(),
            "-w",
            "backend",
            "-b",
            "main",
            "--shallow",
        ],
        _dir.path(),
    );
    assert!(
        output.status.success(),
        "shallow clone failed: {}",
        stderr(&output)
    );

    // Verify it's shallow
    let output = run_git(&["rev-list", "--count", "HEAD"], &clone_path);
    let count = stdout(&output).trim().to_string();
    assert_eq!(
        count, "1",
        "shallow clone should have only 1 commit, got {}",
        count
    );
}

#[test]
fn test_carve_nonexistent_workset() {
    let (_dir, repo) = create_test_repo();
    run_git_ok(&["branch", "no-such-ws"], &repo);

    let wt_path = _dir.path().join("wt-bad");
    let output = run_workset(
        &[
            "carve",
            wt_path.to_str().unwrap(),
            "no-such-ws",
            "-w",
            "nonexistent",
        ],
        &repo,
    );
    assert!(
        !output.status.success(),
        "carve with nonexistent workset should fail"
    );
    assert!(
        stderr(&output).contains("not found"),
        "error should mention 'not found': {}",
        stderr(&output)
    );
}

#[test]
fn test_switch_nonexistent_workset() {
    let (_dir, repo) = create_test_repo();
    run_git_ok(&["branch", "switch-bad"], &repo);

    let wt_path = _dir.path().join("wt-switch-bad");
    let output = run_workset(
        &[
            "carve",
            wt_path.to_str().unwrap(),
            "switch-bad",
            "-w",
            "backend",
        ],
        &repo,
    );
    assert!(output.status.success());

    let output = run_workset(&["switch", "nonexistent"], &wt_path);
    assert!(
        !output.status.success(),
        "switch to nonexistent workset should fail"
    );
    assert!(
        stderr(&output).contains("not found"),
        "error should mention 'not found': {}",
        stderr(&output)
    );
}

#[test]
fn test_init_and_carve_default_workset() {
    // Tests that `init` creates a config whose "all" workset actually works
    let dir = TempDir::new().unwrap();
    let repo = dir.path().join("repo");
    std::fs::create_dir_all(&repo).unwrap();
    run_git_ok(&["init", "--initial-branch=main"], &repo);
    run_git_ok(&["config", "user.email", "test@test.com"], &repo);
    run_git_ok(&["config", "user.name", "Test"], &repo);

    // Create some content and commit
    let src = repo.join("src");
    std::fs::create_dir_all(&src).unwrap();
    std::fs::write(src.join("main.rs"), "fn main() {}").unwrap();
    run_git_ok(&["add", "-A"], &repo);
    run_git_ok(&["commit", "-m", "initial"], &repo);

    // Run init to generate the default config
    let output = run_workset(&["init"], &repo);
    assert!(output.status.success(), "init failed: {}", stderr(&output));
    run_git_ok(&["add", ".git-workset.toml"], &repo);
    run_git_ok(&["commit", "-m", "add workset config"], &repo);

    // Carve a worktree with the default "all" workset
    run_git_ok(&["branch", "test-all"], &repo);
    let wt_path = dir.path().join("wt-all");
    let output = run_workset(
        &["carve", wt_path.to_str().unwrap(), "test-all", "-w", "all"],
        &repo,
    );
    assert!(
        output.status.success(),
        "carve with default 'all' workset failed: {}",
        stderr(&output)
    );

    // Everything should be checked out
    assert!(
        wt_path.join("src/main.rs").exists(),
        "src/main.rs should exist in 'all' workset"
    );
}

#[test]
fn test_carve_empty_includes_gets_full_tree() {
    let (_dir, repo) = create_test_repo();
    run_git_ok(&["branch", "everything-test"], &repo);

    let wt_path = _dir.path().join("wt-everything");
    let output = run_workset(
        &[
            "carve",
            wt_path.to_str().unwrap(),
            "everything-test",
            "-w",
            "everything",
        ],
        &repo,
    );
    assert!(
        output.status.success(),
        "carve with empty includes failed: {}",
        stderr(&output)
    );

    // All directories should be present since sparse checkout is disabled
    assert!(wt_path.join("src/server/hello.txt").exists());
    assert!(wt_path.join("src/client/hello.txt").exists());
    assert!(wt_path.join("src/shared/hello.txt").exists());
    assert!(wt_path.join("assets/hello.txt").exists());
}

#[test]
fn test_carve_with_excludes() {
    let (_dir, repo) = create_test_repo();
    run_git_ok(&["branch", "exclude-test"], &repo);

    let wt_path = _dir.path().join("wt-exclude");
    let output = run_workset(
        &[
            "carve",
            wt_path.to_str().unwrap(),
            "exclude-test",
            "-w",
            "no-assets",
        ],
        &repo,
    );
    assert!(
        output.status.success(),
        "carve with excludes failed: {}",
        stderr(&output)
    );

    // Everything except assets should be present
    assert!(
        wt_path.join("src/server/hello.txt").exists(),
        "src/server should exist"
    );
    assert!(
        wt_path.join("src/client/hello.txt").exists(),
        "src/client should exist"
    );
    assert!(
        wt_path.join("src/shared/hello.txt").exists(),
        "src/shared should exist"
    );
    assert!(
        !wt_path.join("assets/hello.txt").exists(),
        "assets should NOT exist with exclude"
    );
}

#[test]
fn test_carve_reads_config_from_target_branch() {
    // Verify that carve reads .git-workset.toml from the target branch,
    // not from the current working tree. This matters when workset definitions
    // differ between branches.
    let (_dir, repo) = create_test_repo();

    // Create a branch with a different config: "backend" now includes assets too
    run_git_ok(&["checkout", "-b", "custom-config"], &repo);
    let custom_config = r#"
[workset.backend]
description = "Backend with assets"
include = ["src/server", "src/shared", "assets"]

[workset.backend.submodules]
skip = ["ext/lib"]
"#;
    std::fs::write(repo.join(".git-workset.toml"), custom_config).unwrap();
    run_git_ok(&["add", ".git-workset.toml"], &repo);
    run_git_ok(&["commit", "-m", "update config on custom branch"], &repo);

    // Go back to main — its config does NOT include assets in "backend"
    run_git_ok(&["checkout", "main"], &repo);

    // Carve from main, targeting the custom-config branch
    let wt_path = _dir.path().join("wt-custom-cfg");
    let output = run_workset(
        &[
            "carve",
            wt_path.to_str().unwrap(),
            "custom-config",
            "-w",
            "backend",
        ],
        &repo,
    );
    assert!(output.status.success(), "carve failed: {}", stderr(&output));

    // The worktree should use custom-config's definition of "backend",
    // which includes assets — NOT main's definition which excludes them.
    assert!(
        wt_path.join("src/server/hello.txt").exists(),
        "src/server should exist"
    );
    assert!(
        wt_path.join("src/shared/hello.txt").exists(),
        "src/shared should exist"
    );
    assert!(
        wt_path.join("assets/hello.txt").exists(),
        "assets SHOULD exist — config was read from target branch"
    );
}

#[test]
fn test_carve_with_new_branch_flag() {
    let (_dir, repo) = create_test_repo();

    let wt_path = _dir.path().join("wt-new-branch");
    let output = run_workset(
        &[
            "carve",
            wt_path.to_str().unwrap(),
            "-b",
            "feature/new-thing",
            "-w",
            "backend",
        ],
        &repo,
    );
    assert!(
        output.status.success(),
        "carve with -b failed: {}",
        stderr(&output)
    );

    // The worktree should exist with sparse checkout applied
    assert!(
        wt_path.join("src/server/hello.txt").exists(),
        "src/server should be checked out"
    );
    assert!(
        !wt_path.join("src/client/hello.txt").exists(),
        "src/client should NOT be checked out in backend workset"
    );

    // The branch should be "feature/new-thing"
    let output = run_git(&["branch", "--show-current"], &wt_path);
    let branch = stdout(&output).trim().to_string();
    assert_eq!(
        branch, "feature/new-thing",
        "worktree should be on the new branch"
    );
}

#[test]
fn test_carve_with_new_branch_from_commit() {
    let (_dir, repo) = create_test_repo();

    // Add a second commit so we have two distinct points
    std::fs::write(repo.join("src/server/extra.txt"), "extra").unwrap();
    run_git_ok(&["add", "-A"], &repo);
    run_git_ok(&["commit", "-m", "second commit"], &repo);

    // Get the first commit SHA
    let output = run_git(&["rev-parse", "HEAD~1"], &repo);
    let first_commit = stdout(&output).trim().to_string();

    let wt_path = _dir.path().join("wt-from-commit");
    let output = run_workset(
        &[
            "carve",
            wt_path.to_str().unwrap(),
            "-b",
            "feature/from-old",
            &first_commit,
            "-w",
            "backend",
        ],
        &repo,
    );
    assert!(
        output.status.success(),
        "carve with -b <commit> failed: {}",
        stderr(&output)
    );

    // Should be on the new branch
    let output = run_git(&["branch", "--show-current"], &wt_path);
    let branch = stdout(&output).trim().to_string();
    assert_eq!(branch, "feature/from-old");

    // Should be at the first commit (no extra.txt in src/server)
    assert!(
        !wt_path.join("src/server/extra.txt").exists(),
        "extra.txt should NOT exist — branched from first commit"
    );
}

#[test]
fn test_carve_with_force_branch_flag() {
    let (_dir, repo) = create_test_repo();

    // Create a branch that already exists
    run_git_ok(&["branch", "existing-branch"], &repo);

    // Add another commit on main
    std::fs::write(repo.join("src/server/new.txt"), "new content").unwrap();
    run_git_ok(&["add", "-A"], &repo);
    run_git_ok(&["commit", "-m", "newer commit"], &repo);

    let wt_path = _dir.path().join("wt-force-branch");
    // -b should fail because the branch already exists
    let output = run_workset(
        &[
            "carve",
            wt_path.to_str().unwrap(),
            "-b",
            "existing-branch",
            "-w",
            "backend",
        ],
        &repo,
    );
    assert!(
        !output.status.success(),
        "carve with -b should fail for existing branch"
    );

    // -B should succeed and reset the branch to HEAD
    let output = run_workset(
        &[
            "carve",
            wt_path.to_str().unwrap(),
            "-B",
            "existing-branch",
            "-w",
            "backend",
        ],
        &repo,
    );
    assert!(
        output.status.success(),
        "carve with -B should succeed: {}",
        stderr(&output)
    );

    // The branch should now point at the newer commit (has new.txt)
    assert!(
        wt_path.join("src/server/new.txt").exists(),
        "new.txt should exist — branch was reset to HEAD"
    );
}

#[test]
fn test_carve_auto_branch_from_path() {
    let (_dir, repo) = create_test_repo();

    let wt_path = _dir.path().join("auto-branch-name");
    let output = run_workset(
        &["carve", wt_path.to_str().unwrap(), "-w", "backend"],
        &repo,
    );
    assert!(
        output.status.success(),
        "carve without branch should auto-create: {}",
        stderr(&output)
    );

    // Git should create a branch named after the path basename
    let output = run_git(&["branch", "--show-current"], &wt_path);
    let branch = stdout(&output).trim().to_string();
    assert_eq!(
        branch, "auto-branch-name",
        "branch name should match path basename"
    );

    // Sparse checkout should still be applied
    assert!(wt_path.join("src/server/hello.txt").exists());
    assert!(!wt_path.join("src/client/hello.txt").exists());
}

// ---- -f / --config flag tests ----

#[test]
fn test_carve_with_config_flag_overrides_committed() {
    // The strongest possible test: an external file redefines a workset the
    // committed config already defines. If the override works, we see files
    // from the external definition; if it doesn't, we see files from the
    // committed one.
    let (dir, repo) = create_test_repo();
    run_git_ok(&["branch", "override-test"], &repo);

    // External config redefines `backend` to include `assets` only (not
    // src/server or src/shared like the committed one).
    let external_cfg = dir.path().join("external.toml");
    std::fs::write(
        &external_cfg,
        r#"
[workset.backend]
description = "Override"
include = ["assets"]

[workset.backend.submodules]
skip = ["ext/lib"]
"#,
    )
    .unwrap();

    let wt_path = dir.path().join("wt-override");
    let output = run_workset(
        &[
            "-f",
            external_cfg.to_str().unwrap(),
            "carve",
            wt_path.to_str().unwrap(),
            "override-test",
            "-w",
            "backend",
        ],
        &repo,
    );
    assert!(
        output.status.success(),
        "carve -f failed: {}",
        stderr(&output)
    );

    // External definition wins: assets present, src/server absent.
    assert!(
        wt_path.join("assets/hello.txt").exists(),
        "assets/ should exist (from external config)"
    );
    assert!(
        !wt_path.join("src/server/hello.txt").exists(),
        "src/server should NOT exist — committed config must NOT have won"
    );
    assert!(
        !wt_path.join("src/shared/hello.txt").exists(),
        "src/shared should NOT exist — committed config must NOT have won"
    );
}

#[test]
fn test_sync_with_config_flag_changes_sparse_checkout() {
    // Carve with the committed `backend` workset, then re-sync with an
    // external config that redefines `backend`. The sparse checkout should
    // visibly change.
    let (dir, repo) = create_test_repo();
    run_git_ok(&["branch", "sync-override", "main"], &repo);

    let wt_path = dir.path().join("wt-sync-override");
    let output = run_workset(
        &[
            "carve",
            wt_path.to_str().unwrap(),
            "sync-override",
            "-w",
            "backend",
        ],
        &repo,
    );
    assert!(output.status.success(), "carve failed: {}", stderr(&output));

    // Baseline: committed backend → src/server present, assets absent.
    assert!(wt_path.join("src/server/hello.txt").exists());
    assert!(!wt_path.join("assets/hello.txt").exists());

    // Redefine backend externally.
    let external_cfg = dir.path().join("sync-external.toml");
    std::fs::write(
        &external_cfg,
        r#"
[workset.backend]
description = "Override at sync"
include = ["assets"]

[workset.backend.submodules]
skip = ["ext/lib"]
"#,
    )
    .unwrap();

    let output = run_workset(&["-f", external_cfg.to_str().unwrap(), "sync"], &wt_path);
    assert!(
        output.status.success(),
        "sync -f failed: {}",
        stderr(&output)
    );

    // After sync with override: assets present, src/server gone.
    assert!(
        wt_path.join("assets/hello.txt").exists(),
        "assets/ should appear after sync with override"
    );
    assert!(
        !wt_path.join("src/server/hello.txt").exists(),
        "src/server should disappear after sync with override"
    );
}

#[test]
fn test_config_flag_missing_file_errors_cleanly() {
    let (dir, repo) = create_test_repo();
    run_git_ok(&["branch", "missing-cfg-test"], &repo);

    let bogus = dir.path().join("does-not-exist.toml");
    let wt_path = dir.path().join("wt-bogus");

    let output = run_workset(
        &[
            "-f",
            bogus.to_str().unwrap(),
            "carve",
            wt_path.to_str().unwrap(),
            "missing-cfg-test",
            "-w",
            "backend",
        ],
        &repo,
    );
    assert!(
        !output.status.success(),
        "carve with missing -f file should fail"
    );
    let err = stderr(&output);
    assert!(
        err.contains("does-not-exist.toml"),
        "error should mention the missing path: {}",
        err
    );
    assert!(
        !wt_path.exists(),
        "no worktree should have been created on config error"
    );
}

#[test]
fn test_clone_with_config_flag_works_on_repo_without_committed_config() {
    // The headline use case: a repo that hasn't adopted worksets. The remote
    // probe would find no .git-workset.toml; -f lets the user supply one.
    let dir = TempDir::new().unwrap();
    let origin = dir.path().join("origin-no-cfg");
    std::fs::create_dir_all(&origin).unwrap();

    run_git_ok(&["init", "--initial-branch=main"], &origin);
    run_git_ok(&["config", "user.email", "test@test.com"], &origin);
    run_git_ok(&["config", "user.name", "Test"], &origin);

    for subdir in &["engine", "game-a", "game-b"] {
        let p = origin.join(subdir);
        std::fs::create_dir_all(&p).unwrap();
        std::fs::write(p.join("file.txt"), format!("file in {}", subdir)).unwrap();
    }
    run_git_ok(&["add", "-A"], &origin);
    run_git_ok(&["commit", "-m", "no worksets"], &origin);

    // Make sure no .git-workset.toml is committed.
    assert!(!origin.join(".git-workset.toml").exists());

    // External config defining "engine-only".
    let external_cfg = dir.path().join("my-personal-worksets.toml");
    std::fs::write(
        &external_cfg,
        r#"
[workset.engine-only]
description = "Just the engine"
include = ["engine"]
"#,
    )
    .unwrap();

    let repo_url = format!("file://{}", origin.display());
    let clone_path = dir.path().join("cloned-no-cfg");

    let output = run_workset(
        &[
            "-f",
            external_cfg.to_str().unwrap(),
            "clone",
            &repo_url,
            clone_path.to_str().unwrap(),
            "-w",
            "engine-only",
            "-b",
            "main",
        ],
        dir.path(),
    );
    assert!(
        output.status.success(),
        "clone -f against repo with no committed config failed: {}",
        stderr(&output)
    );

    assert!(
        clone_path.join("engine/file.txt").exists(),
        "engine/ should be checked out from external workset"
    );
    assert!(
        !clone_path.join("game-a/file.txt").exists(),
        "game-a/ should NOT be checked out"
    );
    assert!(
        !clone_path.join("game-b/file.txt").exists(),
        "game-b/ should NOT be checked out"
    );

    // The clone should NOT have a probe-leftover directory next to it.
    let probe_leftover = dir.path().join(".cloned-no-cfg-config-probe");
    assert!(
        !probe_leftover.exists(),
        "probe leftover dir should not exist — -f should skip the probe"
    );
}

// ---- Shared submodule object stores ----

/// T1: a carved workset must not get its own copy of the submodule store.
#[test]
fn test_shared_mode_creates_no_duplicate_object_store() {
    let (dir, repo) = create_test_repo();
    run_git_ok(&["branch", "shared-1"], &repo);

    let wt_path = dir.path().join("wt-shared-1");
    let output = run_workset(
        &[
            "carve",
            wt_path.to_str().unwrap(),
            "shared-1",
            "-w",
            "with-sub",
        ],
        &repo,
    );
    assert!(output.status.success(), "carve failed: {}", stderr(&output));

    let wt_gitdir = PathBuf::from(
        stdout(&run_git(&["rev-parse", "--absolute-git-dir"], &wt_path))
            .trim()
            .to_string(),
    );
    assert!(
        !wt_gitdir.join("modules").exists(),
        "workset should have no private modules dir, found {}",
        wt_gitdir.join("modules").display()
    );
    assert!(
        subrepo_gitdir(&repo, "ext/lib").join("worktrees").exists(),
        "shared submodule gitdir should have a worktrees registry"
    );
    assert_eq!(
        count_object_stores(&repo, "ext/lib"),
        1,
        "exactly one object store should exist for ext/lib"
    );
    assert!(
        wt_path.join("ext/lib/lib.txt").exists(),
        "submodule content should be checked out in the workset"
    );
}

/// T2: the shared checkout must sit at the SHA the workset's tree pins.
#[test]
fn test_shared_submodule_checkout_is_at_pinned_sha() {
    let (dir, repo) = create_test_repo();
    let sub_repo = dir.path().join("subrepo");

    // Branch pinning the original submodule commit.
    run_git_ok(&["branch", "older"], &repo);
    let old_pin = stdout(&run_git(&["rev-parse", "older:ext/lib"], &repo))
        .trim()
        .to_string();

    // Advance the submodule and record the newer SHA on main.
    std::fs::write(sub_repo.join("lib.txt"), "updated submodule content").unwrap();
    run_git_ok(&["commit", "-am", "sub second"], &sub_repo);
    let new_pin = stdout(&run_git(&["rev-parse", "HEAD"], &sub_repo))
        .trim()
        .to_string();
    run_git_ok(&["fetch", "origin"], &repo.join("ext/lib"));
    run_git_ok(&["checkout", "--detach", &new_pin], &repo.join("ext/lib"));
    run_git_ok(&["add", "ext/lib"], &repo);
    run_git_ok(&["commit", "-m", "bump submodule"], &repo);

    assert_ne!(old_pin, new_pin);

    let wt_path = dir.path().join("wt-pinned");
    let output = run_workset(
        &[
            "carve",
            wt_path.to_str().unwrap(),
            "older",
            "-w",
            "with-sub",
        ],
        &repo,
    );
    assert!(output.status.success(), "carve failed: {}", stderr(&output));

    let ws_head = stdout(&run_git(&["rev-parse", "HEAD"], &wt_path.join("ext/lib")))
        .trim()
        .to_string();
    assert_eq!(
        ws_head, old_pin,
        "workset submodule should be at the pin recorded on 'older'"
    );

    let main_head = stdout(&run_git(&["rev-parse", "HEAD"], &repo.join("ext/lib")))
        .trim()
        .to_string();
    assert_eq!(
        main_head, new_pin,
        "the main checkout must be left at its own pin"
    );
}

/// T3: a plain `git submodule update` inside a workset must not break the
/// main checkout — regression guard for the shared `core.worktree`.
#[test]
fn test_bare_submodule_update_in_workset_does_not_break_main() {
    let (dir, repo) = create_test_repo();
    run_git_ok(&["branch", "regress"], &repo);

    let wt_path = dir.path().join("wt-regress");
    let output = run_workset(
        &[
            "carve",
            wt_path.to_str().unwrap(),
            "regress",
            "-w",
            "with-sub",
        ],
        &repo,
    );
    assert!(output.status.success(), "carve failed: {}", stderr(&output));

    // The dangerous command: it rewrites core.worktree in the shared config.
    let update = run_git(&["submodule", "update"], &wt_path);
    assert!(
        update.status.success(),
        "submodule update in the workset failed: {}",
        stderr(&update)
    );

    let status = run_git(&["status", "--short"], &repo);
    assert!(
        status.status.success(),
        "main checkout's git status broke: {}",
        stderr(&status)
    );

    let toplevel = run_git(&["rev-parse", "--show-toplevel"], &repo.join("ext/lib"));
    assert!(
        toplevel.status.success(),
        "main submodule no longer resolves: {}",
        stderr(&toplevel)
    );
    assert!(
        same_dir(Path::new(stdout(&toplevel).trim()), &repo.join("ext/lib")),
        "main submodule resolved to {} instead of {}",
        stdout(&toplevel).trim(),
        repo.join("ext/lib").display()
    );

    // The workset checkout must still resolve too.
    let ws_toplevel = run_git(&["rev-parse", "--show-toplevel"], &wt_path.join("ext/lib"));
    assert!(
        ws_toplevel.status.success(),
        "workset submodule no longer resolves: {}",
        stderr(&ws_toplevel)
    );
}

/// T4: hardening runs on every carve and stays correct.
#[test]
fn test_hardening_is_idempotent() {
    let (dir, repo) = create_test_repo();
    run_git_ok(&["branch", "harden-1"], &repo);
    run_git_ok(&["branch", "harden-2"], &repo);

    let ws1 = dir.path().join("wt-harden-1");
    let ws2 = dir.path().join("wt-harden-2");
    for (path, branch) in [(&ws1, "harden-1"), (&ws2, "harden-2")] {
        let output = run_workset(
            &["carve", path.to_str().unwrap(), branch, "-w", "with-sub"],
            &repo,
        );
        assert!(output.status.success(), "carve failed: {}", stderr(&output));
    }

    let gitdir = subrepo_gitdir(&repo, "ext/lib");
    assert_eq!(
        config_file_get(&gitdir.join("config"), "core.worktree"),
        None,
        "core.worktree must not remain in the shared config"
    );
    assert_eq!(
        config_file_get(&gitdir.join("config"), "extensions.worktreeConfig").as_deref(),
        Some("true"),
        "extensions.worktreeConfig should be enabled"
    );

    // The main checkout keeps its own value.
    let main_value = effective_core_worktree(&gitdir, None).expect("main core.worktree");
    let main_resolved = if Path::new(&main_value).is_absolute() {
        PathBuf::from(&main_value)
    } else {
        gitdir.join(&main_value)
    };
    assert!(
        same_dir(&main_resolved, &repo.join("ext/lib")),
        "main core.worktree '{}' should resolve to {}",
        main_value,
        repo.join("ext/lib").display()
    );

    // Each workset has its own, distinct, correct value.
    let id1 = worktree_id(&ws1.join("ext/lib"));
    let id2 = worktree_id(&ws2.join("ext/lib"));
    assert_ne!(id1, id2, "worksets should use distinct worktree ids");

    let v1 = effective_core_worktree(&gitdir, Some(&id1)).expect("ws1 core.worktree");
    let v2 = effective_core_worktree(&gitdir, Some(&id2)).expect("ws2 core.worktree");
    assert_ne!(v1, v2, "per-worktree core.worktree values must differ");
    assert!(
        same_dir(Path::new(&v1), &ws1.join("ext/lib")),
        "ws1 core.worktree was {}",
        v1
    );
    assert!(
        same_dir(Path::new(&v2), &ws2.join("ext/lib")),
        "ws2 core.worktree was {}",
        v2
    );
}

/// T5: `--isolated-submodules` keeps the old per-worktree clone behaviour.
#[test]
fn test_isolated_mode_still_works() {
    let (dir, repo) = create_test_repo();
    run_git_ok(&["branch", "iso"], &repo);

    let wt_path = dir.path().join("wt-iso");
    let output = run_workset(
        &[
            "--isolated-submodules",
            "carve",
            wt_path.to_str().unwrap(),
            "iso",
            "-w",
            "with-sub",
        ],
        &repo,
    );
    assert!(output.status.success(), "carve failed: {}", stderr(&output));

    let wt_gitdir = PathBuf::from(
        stdout(&run_git(&["rev-parse", "--absolute-git-dir"], &wt_path))
            .trim()
            .to_string(),
    );
    assert!(
        wt_gitdir.join("modules/ext/lib/objects").exists(),
        "isolated mode should create {}",
        wt_gitdir.join("modules/ext/lib").display()
    );
    assert!(
        wt_path.join("ext/lib/lib.txt").exists(),
        "submodule content should be checked out"
    );
    assert_eq!(
        count_object_stores(&repo, "ext/lib"),
        2,
        "isolated mode means a second object store"
    );

    // Hardening applies in isolated mode too — the main checkout stays healthy.
    let status = run_git(&["status", "--short"], &repo);
    assert!(
        status.status.success(),
        "main checkout broke: {}",
        stderr(&status)
    );
    assert_eq!(
        config_file_get(
            &subrepo_gitdir(&repo, "ext/lib").join("config"),
            "core.worktree"
        ),
        None,
        "the main submodule gitdir should be hardened even in isolated mode"
    );
}

/// T6: CLI flag > git config > .git-workset.toml > default.
#[test]
fn test_mode_precedence() {
    struct Case {
        name: &'static str,
        toml: Option<&'static str>,
        git_config: Option<&'static str>,
        flag: Option<&'static str>,
        expected: &'static str,
    }

    let cases = [
        Case {
            name: "nothing-set",
            toml: None,
            git_config: None,
            flag: None,
            expected: "shared",
        },
        Case {
            name: "toml-only",
            toml: Some("isolated"),
            git_config: None,
            flag: None,
            expected: "isolated",
        },
        Case {
            name: "git-config-beats-toml",
            toml: Some("isolated"),
            git_config: Some("shared"),
            flag: None,
            expected: "shared",
        },
        Case {
            name: "flag-beats-both",
            toml: Some("shared"),
            git_config: Some("shared"),
            flag: Some("--isolated-submodules"),
            expected: "isolated",
        },
    ];

    for case in cases {
        let (dir, repo) = create_test_repo();
        if let Some(value) = case.toml {
            prepend_config(&repo, &format!("[submodules]\nsharing = \"{}\"\n", value));
        }
        if let Some(value) = case.git_config {
            run_git_ok(&["config", "workset.submoduleSharing", value], &repo);
        }
        run_git_ok(&["branch", "prec"], &repo);

        let wt_path = dir.path().join(format!("wt-{}", case.name));
        let mut args: Vec<&str> = Vec::new();
        if let Some(flag) = case.flag {
            args.push(flag);
        }
        let wt_str = wt_path.to_str().unwrap();
        args.extend(["carve", wt_str, "prec", "-w", "with-sub"]);
        let output = run_workset(&args, &repo);
        assert!(
            output.status.success(),
            "[{}] carve failed: {}",
            case.name,
            stderr(&output)
        );

        assert_eq!(
            worktree_config(&wt_path, "workset.submoduleSharing").as_deref(),
            Some(case.expected),
            "[{}] recorded mode mismatch",
            case.name
        );

        let expected_stores = if case.expected == "shared" { 1 } else { 2 };
        assert_eq!(
            count_object_stores(&repo, "ext/lib"),
            expected_stores,
            "[{}] object store count mismatch",
            case.name
        );
    }
}

/// T7: a worktree carved isolated is never silently converted by `sync`.
#[test]
fn test_mode_persists_across_sync() {
    let (dir, repo) = create_test_repo();
    prepend_config(&repo, "[submodules]\nsharing = \"shared\"\n");
    run_git_ok(&["branch", "persist"], &repo);

    let wt_path = dir.path().join("wt-persist");
    let output = run_workset(
        &[
            "--isolated-submodules",
            "carve",
            wt_path.to_str().unwrap(),
            "persist",
            "-w",
            "with-sub",
        ],
        &repo,
    );
    assert!(output.status.success(), "carve failed: {}", stderr(&output));
    assert_eq!(count_object_stores(&repo, "ext/lib"), 2);

    let output = run_workset(&["sync"], &wt_path);
    assert!(output.status.success(), "sync failed: {}", stderr(&output));

    assert_eq!(
        worktree_config(&wt_path, "workset.submoduleSharing").as_deref(),
        Some("isolated"),
        "sync must not convert an isolated worktree"
    );
    assert_eq!(
        count_object_stores(&repo, "ext/lib"),
        2,
        "the isolated store should still be there after sync"
    );
}

/// T8: remove cleans both the superproject and submodule registries.
#[test]
fn test_remove_cleans_both_registries() {
    let (dir, repo) = create_test_repo();
    run_git_ok(&["branch", "rm-1"], &repo);

    let wt_path = dir.path().join("wt-rm-1");
    let output = run_workset(
        &["carve", wt_path.to_str().unwrap(), "rm-1", "-w", "with-sub"],
        &repo,
    );
    assert!(output.status.success(), "carve failed: {}", stderr(&output));

    let output = run_workset(&["remove", wt_path.to_str().unwrap()], &repo);
    assert!(
        output.status.success(),
        "remove failed: {}",
        stderr(&output)
    );
    assert!(!wt_path.exists(), "worktree directory should be gone");

    let list = stdout(&run_git(&["worktree", "list"], &repo));
    assert_eq!(
        list.lines().count(),
        1,
        "superproject should have one worktree left: {}",
        list
    );

    let sub_list = stdout(&run_git(&["worktree", "list"], &repo.join("ext/lib")));
    assert_eq!(
        sub_list.lines().count(),
        1,
        "submodule should have one worktree left: {}",
        sub_list
    );
    assert!(
        !sub_list.contains("prunable"),
        "submodule registry should have no prunable entries: {}",
        sub_list
    );
}

/// T9: removal is not blocked by `working trees containing submodules`.
#[test]
fn test_remove_is_not_blocked_by_submodules() {
    let (dir, repo) = create_test_repo();
    run_git_ok(&["branch", "rm-2"], &repo);

    let wt_path = dir.path().join("wt-rm-2");
    let output = run_workset(
        &[
            "carve",
            wt_path.to_str().unwrap(),
            "rm-2",
            "-w",
            "with-subs",
        ],
        &repo,
    );
    assert!(output.status.success(), "carve failed: {}", stderr(&output));
    assert!(wt_path.join("ext/lib/lib.txt").exists());
    assert!(wt_path.join("ext/lib2/lib2.txt").exists());

    let output = run_workset(&["remove", wt_path.to_str().unwrap()], &repo);
    let err = stderr(&output);
    assert!(
        !err.contains("working trees containing submodules"),
        "remove hit the submodule block: {}",
        err
    );
    assert!(output.status.success(), "remove failed: {}", err);
    assert!(!wt_path.exists(), "worktree directory should be gone");

    for sub in ["ext/lib", "ext/lib2"] {
        let sub_list = stdout(&run_git(&["worktree", "list"], &repo.join(sub)));
        assert!(
            !sub_list.contains("prunable"),
            "{} registry should be clean: {}",
            sub,
            sub_list
        );
    }
}

/// T10: doctor detects and repairs a clobbered core.worktree (the damage
/// v0.3.x left behind).
#[test]
fn test_doctor_detects_and_fixes_clobbered_core_worktree() {
    let (dir, repo) = create_test_repo();
    run_git_ok(&["branch", "doc-1"], &repo);

    let wt_path = dir.path().join("wt-doc-1");
    let output = run_workset(
        &[
            "carve",
            wt_path.to_str().unwrap(),
            "doc-1",
            "-w",
            "with-sub",
        ],
        &repo,
    );
    assert!(output.status.success(), "carve failed: {}", stderr(&output));

    // Reproduce the damage: un-harden, then point core.worktree somewhere bogus.
    let gitdir = subrepo_gitdir(&repo, "ext/lib");
    let shared_cfg = gitdir.join("config");
    let _ = std::fs::remove_file(gitdir.join("config.worktree"));
    run_git_ok(
        &[
            "config",
            "-f",
            shared_cfg.to_str().unwrap(),
            "extensions.worktreeConfig",
            "false",
        ],
        &repo,
    );
    run_git_ok(
        &[
            "config",
            "-f",
            shared_cfg.to_str().unwrap(),
            "core.worktree",
            "/nonexistent/path",
        ],
        &repo,
    );
    assert!(
        !run_git(&["status", "--short"], &repo.join("ext/lib"))
            .status
            .success(),
        "the damage should actually break the main submodule checkout"
    );

    let output = run_workset(&["doctor"], &repo);
    assert!(
        !output.status.success(),
        "doctor should exit non-zero when issues are found"
    );
    assert!(
        stdout(&output).contains("D2"),
        "doctor should report D2: {}",
        stdout(&output)
    );

    let output = run_workset(&["doctor", "--fix"], &repo);
    assert!(
        output.status.success(),
        "doctor --fix failed: {} {}",
        stdout(&output),
        stderr(&output)
    );

    let status = run_git(&["status", "--short"], &repo);
    assert!(
        status.status.success(),
        "main status still broken after --fix: {}",
        stderr(&status)
    );
    let toplevel = run_git(&["rev-parse", "--show-toplevel"], &repo.join("ext/lib"));
    assert!(
        toplevel.status.success()
            && same_dir(Path::new(stdout(&toplevel).trim()), &repo.join("ext/lib")),
        "main submodule should resolve after --fix: {}",
        stderr(&toplevel)
    );

    let output = run_workset(&["doctor"], &repo);
    assert!(
        output.status.success(),
        "doctor should be clean after --fix: {}",
        stdout(&output)
    );
}

/// T11: doctor prunes orphan submodule worktree registrations.
#[test]
fn test_doctor_prunes_orphan_submodule_worktrees() {
    let (dir, repo) = create_test_repo();
    run_git_ok(&["branch", "doc-2"], &repo);

    let wt_path = dir.path().join("wt-doc-2");
    let output = run_workset(
        &[
            "carve",
            wt_path.to_str().unwrap(),
            "doc-2",
            "-w",
            "with-sub",
        ],
        &repo,
    );
    assert!(output.status.success(), "carve failed: {}", stderr(&output));

    // The user rm -rf's the workset instead of using `git workset remove`.
    std::fs::remove_dir_all(&wt_path).unwrap();

    let output = run_workset(&["doctor"], &repo);
    assert!(!output.status.success(), "doctor should report the orphans");
    assert!(
        stdout(&output).contains("D3"),
        "doctor should report D3: {}",
        stdout(&output)
    );

    let output = run_workset(&["doctor", "--fix"], &repo);
    assert!(
        output.status.success(),
        "doctor --fix failed: {} {}",
        stdout(&output),
        stderr(&output)
    );

    let sub_list = stdout(&run_git(&["worktree", "list"], &repo.join("ext/lib")));
    assert!(
        !sub_list.contains("prunable"),
        "submodule registry should be clean: {}",
        sub_list
    );

    let output = run_workset(&["doctor"], &repo);
    assert!(
        output.status.success(),
        "doctor should be clean after --fix: {}",
        stdout(&output)
    );
}

/// T12: skipped submodules behave exactly as before in shared mode.
#[test]
fn test_skipped_submodules_unaffected_in_shared_mode() {
    let (dir, repo) = create_test_repo();
    run_git_ok(&["branch", "skip-test"], &repo);

    let wt_path = dir.path().join("wt-skip");
    let output = run_workset(
        &[
            "carve",
            wt_path.to_str().unwrap(),
            "skip-test",
            "-w",
            "backend",
        ],
        &repo,
    );
    assert!(output.status.success(), "carve failed: {}", stderr(&output));

    assert_eq!(
        worktree_config(&wt_path, "submodule.ext/lib.active").as_deref(),
        Some("false"),
        "skipped submodule should be marked inactive"
    );

    let sub_list = stdout(&run_git(&["worktree", "list"], &repo.join("ext/lib")));
    assert_eq!(
        sub_list.lines().count(),
        1,
        "no submodule worktree should have been created: {}",
        sub_list
    );

    let checkout = wt_path.join("ext/lib");
    let empty = !checkout.exists()
        || std::fs::read_dir(&checkout)
            .map(|mut d| d.next().is_none())
            .unwrap_or(true);
    assert!(empty, "{} should be empty", checkout.display());
}

/// T13: two worksets can hold the same submodule; only branch checkout is
/// exclusive, and that never breaks the other workset (§3.5).
#[test]
fn test_same_branch_in_two_worksets_is_not_fatal() {
    let (dir, repo) = create_test_repo();
    run_git_ok(&["branch", "excl-1"], &repo);
    run_git_ok(&["branch", "excl-2"], &repo);

    let ws1 = dir.path().join("wt-excl-1");
    let output = run_workset(
        &["carve", ws1.to_str().unwrap(), "excl-1", "-w", "with-sub"],
        &repo,
    );
    assert!(
        output.status.success(),
        "carve ws1 failed: {}",
        stderr(&output)
    );

    // The user starts branch work inside ws1's submodule.
    run_git_ok(&["checkout", "-b", "sub-work"], &ws1.join("ext/lib"));

    let ws2 = dir.path().join("wt-excl-2");
    let output = run_workset(
        &["carve", ws2.to_str().unwrap(), "excl-2", "-w", "with-sub"],
        &repo,
    );
    assert!(
        output.status.success(),
        "carve ws2 must not fail because ws1 holds a submodule branch: {}",
        stderr(&output)
    );
    assert!(
        ws2.join("ext/lib/lib.txt").exists(),
        "ws2 should have a working submodule checkout"
    );

    // ws1's branch checkout is untouched.
    let branch = stdout(&run_git(
        &["branch", "--show-current"],
        &ws1.join("ext/lib"),
    ));
    assert_eq!(
        branch.trim(),
        "sub-work",
        "ws1 should still be on its branch"
    );
    let status = run_git(&["status", "--short"], &ws1.join("ext/lib"));
    assert!(
        status.status.success(),
        "ws1 submodule broke after ws2 was carved: {}",
        stderr(&status)
    );

    // The documented limitation: the same branch cannot be checked out twice.
    let conflict = run_git(&["checkout", "sub-work"], &ws2.join("ext/lib"));
    assert!(
        !conflict.status.success() && stderr(&conflict).contains("already used by worktree"),
        "checking out the same submodule branch twice should be refused: {}",
        stderr(&conflict)
    );
}

/// T14: a pin missing from the shared store is fetched, not re-cloned.
#[test]
fn test_missing_pin_is_fetched_into_shared_store() {
    let (dir, repo) = create_test_repo();
    let sub_repo = dir.path().join("subrepo");

    // A submodule commit that exists on the remote but not in the local store.
    std::fs::write(sub_repo.join("lib.txt"), "remote-only content").unwrap();
    run_git_ok(&["commit", "-am", "remote-only commit"], &sub_repo);
    let remote_only = stdout(&run_git(&["rev-parse", "HEAD"], &sub_repo))
        .trim()
        .to_string();

    let gitdir = subrepo_gitdir(&repo, "ext/lib");
    assert!(
        !run_git(
            &[
                "--git-dir",
                gitdir.to_str().unwrap(),
                "cat-file",
                "-e",
                &format!("{}^{{commit}}", remote_only)
            ],
            &repo
        )
        .status
        .success(),
        "the commit should be missing from the shared store to begin with"
    );

    // Record the new pin without fetching it into the main store.
    run_git_ok(
        &[
            "update-index",
            "--cacheinfo",
            &format!("160000,{},ext/lib", remote_only),
        ],
        &repo,
    );
    run_git_ok(&["commit", "-m", "pin remote-only submodule commit"], &repo);
    run_git_ok(&["branch", "fetch-pin"], &repo);

    let wt_path = dir.path().join("wt-fetch-pin");
    let output = run_workset(
        &[
            "carve",
            wt_path.to_str().unwrap(),
            "fetch-pin",
            "-w",
            "with-sub",
        ],
        &repo,
    );
    assert!(
        output.status.success(),
        "carve with a missing pin failed: {}",
        stderr(&output)
    );

    let head = stdout(&run_git(&["rev-parse", "HEAD"], &wt_path.join("ext/lib")))
        .trim()
        .to_string();
    assert_eq!(head, remote_only, "workset should be at the fetched pin");
    assert_eq!(
        count_object_stores(&repo, "ext/lib"),
        1,
        "fetching a pin must not create a second object store"
    );
}

/// T15: `sync --shared-submodules` migrates an isolated worktree.
#[test]
fn test_sync_migrates_isolated_to_shared() {
    let (dir, repo) = create_test_repo();
    run_git_ok(&["branch", "migrate"], &repo);

    let wt_path = dir.path().join("wt-migrate");
    let output = run_workset(
        &[
            "--isolated-submodules",
            "carve",
            wt_path.to_str().unwrap(),
            "migrate",
            "-w",
            "with-sub",
        ],
        &repo,
    );
    assert!(output.status.success(), "carve failed: {}", stderr(&output));

    let wt_gitdir = PathBuf::from(
        stdout(&run_git(&["rev-parse", "--absolute-git-dir"], &wt_path))
            .trim()
            .to_string(),
    );
    let duplicate = wt_gitdir.join("modules/ext/lib");
    assert!(duplicate.exists(), "setup should produce a duplicate store");
    let content_before = std::fs::read_to_string(wt_path.join("ext/lib/lib.txt")).unwrap();

    let output = run_workset(&["--shared-submodules", "sync"], &wt_path);
    assert!(output.status.success(), "sync failed: {}", stderr(&output));

    assert!(
        !duplicate.exists(),
        "the duplicate gitdir should be gone: {}",
        duplicate.display()
    );
    assert_eq!(
        count_object_stores(&repo, "ext/lib"),
        1,
        "only the shared store should remain"
    );

    let sub_list = stdout(&run_git(&["worktree", "list"], &repo.join("ext/lib")));
    assert!(
        sub_list.contains(wt_path.join("ext/lib").to_str().unwrap())
            || sub_list.lines().count() == 2,
        "the workset should now be registered as a submodule worktree: {}",
        sub_list
    );
    assert_eq!(
        std::fs::read_to_string(wt_path.join("ext/lib/lib.txt")).unwrap(),
        content_before,
        "submodule content should be unchanged"
    );
    assert_eq!(
        worktree_config(&wt_path, "workset.submoduleSharing").as_deref(),
        Some("shared")
    );
}

/// T16: migration refuses to run over uncommitted submodule work.
#[test]
fn test_sync_refuses_migration_with_dirty_submodule() {
    let (dir, repo) = create_test_repo();
    run_git_ok(&["branch", "dirty"], &repo);

    let wt_path = dir.path().join("wt-dirty");
    let output = run_workset(
        &[
            "--isolated-submodules",
            "carve",
            wt_path.to_str().unwrap(),
            "dirty",
            "-w",
            "with-sub",
        ],
        &repo,
    );
    assert!(output.status.success(), "carve failed: {}", stderr(&output));

    let wt_gitdir = PathBuf::from(
        stdout(&run_git(&["rev-parse", "--absolute-git-dir"], &wt_path))
            .trim()
            .to_string(),
    );
    let duplicate = wt_gitdir.join("modules/ext/lib");

    // Uncommitted work in the submodule.
    std::fs::write(wt_path.join("ext/lib/lib.txt"), "work in progress").unwrap();

    let output = run_workset(&["--shared-submodules", "sync"], &wt_path);
    assert!(
        !output.status.success(),
        "sync should refuse to migrate a dirty submodule"
    );
    assert!(
        stderr(&output).contains("ext/lib"),
        "the error should name the dirty submodule: {}",
        stderr(&output)
    );

    assert!(
        duplicate.exists(),
        "nothing should have been deleted: {}",
        duplicate.display()
    );
    assert_eq!(
        std::fs::read_to_string(wt_path.join("ext/lib/lib.txt")).unwrap(),
        "work in progress",
        "the user's work must be intact"
    );
}

/// An unknown `sharing` value must fail loudly and name the valid options,
/// rather than silently falling back to the default.
#[test]
fn test_invalid_sharing_value_is_a_parse_error() {
    let (dir, repo) = create_test_repo();
    prepend_config(&repo, "[submodules]\nsharing = \"sometimes\"\n");
    run_git_ok(&["branch", "bad-mode"], &repo);

    let wt_path = dir.path().join("wt-bad-mode");
    let output = run_workset(
        &[
            "carve",
            wt_path.to_str().unwrap(),
            "bad-mode",
            "-w",
            "with-sub",
        ],
        &repo,
    );
    assert!(
        !output.status.success(),
        "carve should fail on an unknown sharing value"
    );
    let err = stderr(&output);
    assert!(
        err.contains("shared") && err.contains("isolated"),
        "the error should name the valid options: {}",
        err
    );
    assert!(
        !wt_path.exists(),
        "no worktree should have been created on a config error"
    );
}

/// The carve summary reports how the submodules were handled.
#[test]
fn test_carve_reports_submodule_summary() {
    let (dir, repo) = create_test_repo();
    run_git_ok(&["branch", "summary"], &repo);

    let wt_path = dir.path().join("wt-summary");
    let output = run_workset(
        &[
            "carve",
            wt_path.to_str().unwrap(),
            "summary",
            "-w",
            "with-sub",
        ],
        &repo,
    );
    assert!(output.status.success(), "carve failed: {}", stderr(&output));
    assert!(
        stderr(&output).contains("submodules: 1 shared (0 cloned), 1 skipped"),
        "carve should summarise submodule handling: {}",
        stderr(&output)
    );
}

/// Removal refuses to throw away uncommitted work unless asked to.
#[test]
fn test_remove_refuses_dirty_worktree_without_force() {
    let (dir, repo) = create_test_repo();
    run_git_ok(&["branch", "rm-dirty"], &repo);

    let wt_path = dir.path().join("wt-rm-dirty");
    let output = run_workset(
        &[
            "carve",
            wt_path.to_str().unwrap(),
            "rm-dirty",
            "-w",
            "with-sub",
        ],
        &repo,
    );
    assert!(output.status.success(), "carve failed: {}", stderr(&output));

    std::fs::write(wt_path.join("src/server/hello.txt"), "unsaved work").unwrap();

    let output = run_workset(&["remove", wt_path.to_str().unwrap()], &repo);
    assert!(
        !output.status.success(),
        "remove should refuse a dirty worktree"
    );
    assert!(
        stderr(&output).contains("--force"),
        "the error should point at --force: {}",
        stderr(&output)
    );
    assert!(wt_path.exists(), "the worktree must still be there");

    let output = run_workset(&["remove", "--force", wt_path.to_str().unwrap()], &repo);
    assert!(
        output.status.success(),
        "remove --force failed: {}",
        stderr(&output)
    );
    assert!(!wt_path.exists(), "worktree should be gone after --force");

    let sub_list = stdout(&run_git(&["worktree", "list"], &repo.join("ext/lib")));
    assert!(
        !sub_list.contains("prunable"),
        "submodule registry should be clean: {}",
        sub_list
    );
}

// ---- v0.5.0: config lookup, composition, strict config ----

/// Run git-workset with `input` on stdin.
fn run_workset_stdin(args: &[&str], cwd: &Path, input: &str) -> Output {
    use std::io::Write;
    use std::process::Stdio;
    let mut child = Command::new(git_workset_bin())
        .args(args)
        .current_dir(cwd)
        .env("GIT_LFS_SKIP_SMUDGE", "1")
        .env("GIT_CONFIG_COUNT", "1")
        .env("GIT_CONFIG_KEY_0", "protocol.file.allow")
        .env("GIT_CONFIG_VALUE_0", "always")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("failed to spawn git-workset");
    child
        .stdin
        .take()
        .unwrap()
        .write_all(input.as_bytes())
        .unwrap();
    child.wait_with_output().expect("failed to wait")
}

fn carve_ok(repo: &Path, wt: &Path, branch: &str, workset: &str) {
    run_git_ok(&["branch", branch], repo);
    let output = run_workset(
        &["carve", wt.to_str().unwrap(), branch, "-w", workset],
        repo,
    );
    assert!(output.status.success(), "carve failed: {}", stderr(&output));
}

/// `switch` in a linked worktree uses that worktree's config, not the main
/// worktree's.
#[test]
fn test_switch_reads_config_from_current_worktree() {
    let (dir, repo) = create_test_repo();
    let wt = dir.path().join("wt-own-cfg");
    carve_ok(&repo, &wt, "own-cfg", "backend");

    // Only this worktree's branch redefines frontend to include assets.
    let cfg = std::fs::read_to_string(wt.join(".git-workset.toml"))
        .unwrap()
        .replace(
            "include = [\"src/client\", \"src/shared\"]",
            "include = [\"assets\"]",
        );
    std::fs::write(wt.join(".git-workset.toml"), cfg).unwrap();
    run_git_ok(&["commit", "-am", "branch-local frontend"], &wt);

    let output = run_workset(&["switch", "frontend"], &wt);
    assert!(
        output.status.success(),
        "switch failed: {}",
        stderr(&output)
    );
    assert!(
        wt.join("assets/hello.txt").exists(),
        "the worktree's own definition of frontend should apply"
    );
    assert!(
        !wt.join("src/client/hello.txt").exists(),
        "the main worktree's definition must not be used"
    );
}

/// A full-tree profile composed with anything is still the full tree.
#[test]
fn test_compose_with_full_tree_profile_is_full_tree() {
    let (dir, repo) = create_test_repo();
    let wt = dir.path().join("wt-everything-plus");
    carve_ok(&repo, &wt, "everything-plus", "everything+backend");

    for p in ["src/server", "src/client", "src/shared", "assets"] {
        assert!(
            wt.join(p).join("hello.txt").exists(),
            "{} should be checked out in everything+backend",
            p
        );
    }
}

/// One profile's exclude does not remove what another explicitly includes.
#[test]
fn test_compose_exclude_does_not_remove_other_include() {
    let (dir, repo) = create_test_repo();
    let wt = dir.path().join("wt-noassets-plus");
    carve_ok(&repo, &wt, "noassets-plus", "no-assets+all");

    assert!(
        wt.join("assets/hello.txt").exists(),
        "all includes assets, so no-assets must not remove it"
    );
    assert!(wt.join("src/client/hello.txt").exists());
}

/// An exclude still applies when no other profile covers the excluded path,
/// and a narrower include under it is re-added.
#[test]
fn test_compose_exclude_keeps_narrower_include() {
    let (dir, repo) = create_test_repo();
    let cfg = dir.path().join("compose.toml");
    std::fs::write(
        &cfg,
        r#"
[workset.no-src]
exclude = ["src"]
[workset.server]
include = ["src/server"]
"#,
    )
    .unwrap();
    run_git_ok(&["branch", "narrow"], &repo);
    let wt = dir.path().join("wt-narrow");
    let output = run_workset(
        &[
            "-f",
            cfg.to_str().unwrap(),
            "carve",
            wt.to_str().unwrap(),
            "narrow",
            "-w",
            "no-src+server",
        ],
        &repo,
    );
    assert!(output.status.success(), "carve failed: {}", stderr(&output));
    assert!(wt.join("assets/hello.txt").exists(), "no-src keeps assets");
    assert!(
        wt.join("src/server/hello.txt").exists(),
        "server's include must survive no-src's exclude"
    );
    assert!(
        !wt.join("src/client/hello.txt").exists(),
        "the rest of src stays excluded"
    );
}

/// No-cone includes are anchored at the root, like cone mode.
#[test]
fn test_no_cone_include_is_anchored() {
    let (dir, repo) = create_test_repo();
    std::fs::create_dir_all(repo.join("src/client/assets")).unwrap();
    std::fs::write(repo.join("src/client/assets/nested.txt"), "nested").unwrap();
    run_git_ok(&["add", "-A"], &repo);
    run_git_ok(&["commit", "-m", "nested assets dir"], &repo);

    let cfg = dir.path().join("nocone.toml");
    std::fs::write(
        &cfg,
        "[workset.art]\ninclude = [\"assets\"]\nsparse_cone = false\n",
    )
    .unwrap();
    run_git_ok(&["branch", "nocone"], &repo);
    let wt = dir.path().join("wt-nocone");
    let output = run_workset(
        &[
            "-f",
            cfg.to_str().unwrap(),
            "carve",
            wt.to_str().unwrap(),
            "nocone",
            "-w",
            "art",
        ],
        &repo,
    );
    assert!(output.status.success(), "carve failed: {}", stderr(&output));
    assert!(wt.join("assets/hello.txt").exists());
    assert!(
        !wt.join("src/client/assets/nested.txt").exists(),
        "`assets` must not match src/client/assets"
    );
}

/// Switching to a profile that no longer skips a submodule brings it back,
/// even in isolated mode where `active=false` would block the update.
#[test]
fn test_switch_unskips_previously_skipped_submodule() {
    let (dir, repo) = create_test_repo();
    run_git_ok(&["branch", "unskip"], &repo);
    let wt = dir.path().join("wt-unskip");
    let output = run_workset(
        &[
            "--isolated-submodules",
            "carve",
            wt.to_str().unwrap(),
            "unskip",
            "-w",
            "backend",
        ],
        &repo,
    );
    assert!(output.status.success(), "carve failed: {}", stderr(&output));
    assert_eq!(
        worktree_config(&wt, "submodule.ext/lib.active").as_deref(),
        Some("false")
    );

    let output = run_workset(&["switch", "with-sub"], &wt);
    assert!(
        output.status.success(),
        "switch failed: {}",
        stderr(&output)
    );
    assert!(
        wt.join("ext/lib/lib.txt").exists(),
        "ext/lib should be checked out after switching to with-sub"
    );
    assert_eq!(
        worktree_config(&wt, "submodule.ext/lib.active"),
        None,
        "the stale active=false should be cleared"
    );
    assert_eq!(
        worktree_config(&wt, "submodule.ext/lib2.active").as_deref(),
        Some("false"),
        "ext/lib2 is still skipped"
    );
}

/// `switch` names sparse patterns added by hand that it is about to drop.
#[test]
fn test_switch_warns_about_dropped_manual_patterns() {
    let (dir, repo) = create_test_repo();
    let wt = dir.path().join("wt-dropped");
    carve_ok(&repo, &wt, "dropped", "backend");
    run_git_ok(&["sparse-checkout", "add", "assets"], &wt);

    let output = run_workset(&["switch", "frontend"], &wt);
    assert!(
        output.status.success(),
        "switch failed: {}",
        stderr(&output)
    );
    let err = stderr(&output);
    assert!(
        err.contains("warning: dropping sparse patterns") && err.contains("assets"),
        "switch should name the dropped pattern: {}",
        err
    );
    assert!(
        !err.contains("src/server, "),
        "patterns git-workset applied itself are not reported: {}",
        err
    );
}

/// A plain re-sync with no manual additions prints no warning.
#[test]
fn test_sync_without_manual_patterns_does_not_warn() {
    let (dir, repo) = create_test_repo();
    let wt = dir.path().join("wt-nowarn");
    carve_ok(&repo, &wt, "nowarn", "backend");

    let output = run_workset(&["switch", "frontend"], &wt);
    assert!(
        output.status.success(),
        "switch failed: {}",
        stderr(&output)
    );
    assert!(
        !stderr(&output).contains("dropping"),
        "nothing was added by hand: {}",
        stderr(&output)
    );
}

/// `-f -` reads the config from stdin.
#[test]
fn test_config_from_stdin() {
    let (dir, repo) = create_test_repo();
    let wt = dir.path().join("wt-stdin");
    carve_ok(&repo, &wt, "stdin", "backend");

    let output = run_workset_stdin(
        &["-f", "-", "switch", "piped"],
        &wt,
        "[workset.piped]\ninclude = [\"assets\"]\n",
    );
    assert!(
        output.status.success(),
        "switch failed: {}",
        stderr(&output)
    );
    assert!(wt.join("assets/hello.txt").exists());
    assert!(!wt.join("src/server/hello.txt").exists());
}

/// An empty config says so instead of printing an empty list.
#[test]
fn test_empty_config_error_message() {
    let (dir, repo) = create_test_repo();
    let output = run_workset_stdin(&["-f", "-", "switch", "a"], &repo, "");
    assert!(!output.status.success());
    let err = stderr(&output);
    assert!(
        err.contains("defines no worksets") && !err.contains("Available: \n"),
        "unhelpful message: {}",
        err
    );
    drop(dir);
}

/// Unknown keys are errors that name the key, at any depth.
#[test]
fn test_unknown_config_key_is_rejected() {
    let (dir, repo) = create_test_repo();
    for (cfg, key) in [
        ("bogus = 1\n[workset.a]\ninclude = [\"src\"]\n", "bogus"),
        ("[workset.a]\nincldue = [\"src\"]\n", "incldue"),
        (
            "[workset.a]\ninclude = [\"src\"]\n[workset.a.submodules]\nskp = []\n",
            "skp",
        ),
    ] {
        let output = run_workset_stdin(&["-f", "-", "switch", "a"], &repo, cfg);
        assert!(!output.status.success(), "{} should be rejected", key);
        assert!(
            stderr(&output).contains(key),
            "error should name `{}`: {}",
            key,
            stderr(&output)
        );
    }
    drop(dir);
}

/// A config written for a newer git-workset is refused.
#[test]
fn test_newer_config_version_is_rejected() {
    let (dir, repo) = create_test_repo();
    let output = run_workset_stdin(
        &["-f", "-", "switch", "a"],
        &repo,
        "version = 2\n[workset.a]\ninclude = [\"src\"]\n",
    );
    assert!(!output.status.success());
    assert!(
        stderr(&output).contains("unsupported config version 2"),
        "{}",
        stderr(&output)
    );

    let output = run_workset_stdin(
        &["-f", "-", "switch", "a"],
        &repo,
        "version = 1\n[workset.a]\ninclude = [\"src\"]\n",
    );
    assert!(
        output.status.success(),
        "version 1 is accepted: {}",
        stderr(&output)
    );
    drop(dir);
}

/// git's own output never lands on stdout.
#[test]
fn test_child_git_output_goes_to_stderr() {
    let (dir, repo) = create_test_repo();
    run_git_ok(&["branch", "quiet"], &repo);
    let wt = dir.path().join("wt-quiet");
    let output = run_workset(
        &[
            "--isolated-submodules",
            "carve",
            wt.to_str().unwrap(),
            "quiet",
            "-w",
            "with-subs",
        ],
        &repo,
    );
    assert!(output.status.success(), "carve failed: {}", stderr(&output));
    assert_eq!(stdout(&output), "", "stdout should be empty");
    assert!(
        stderr(&output).contains("ext/lib"),
        "git's submodule output should be on stderr"
    );
}

// ---- v0.6.0: apply, profiles, --json, exit codes ----

/// Run git-workset with a clean environment: an empty HOME and global
/// config, and an optional PATH override.
fn run_workset_isolated(args: &[&str], cwd: &Path, home: &Path, path: Option<&str>) -> Output {
    let mut cmd = Command::new(git_workset_bin());
    cmd.args(args)
        .current_dir(cwd)
        .env("HOME", home)
        .env("GIT_CONFIG_GLOBAL", home.join(".gitconfig"))
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_COUNT", "1")
        .env("GIT_CONFIG_KEY_0", "protocol.file.allow")
        .env("GIT_CONFIG_VALUE_0", "always");
    if let Some(p) = path {
        cmd.env("PATH", p);
    }
    cmd.output().expect("failed to execute git-workset")
}

/// stdout must be exactly one JSON document.
fn json_doc(output: &Output) -> serde_json::Value {
    let out = stdout(output);
    let mut stream = serde_json::Deserializer::from_str(&out).into_iter::<serde_json::Value>();
    let doc = stream
        .next()
        .unwrap_or_else(|| panic!("no JSON on stdout; stderr: {}", stderr(output)))
        .unwrap_or_else(|e| panic!("stdout is not JSON ({}): {:?}", e, out));
    assert!(
        stream.next().is_none(),
        "stdout has more than one document: {:?}",
        out
    );
    doc
}

/// A `--no-checkout` partial clone of the fixture with `origin` renamed, as
/// a sandbox runner would hand it over. Returns the clone and the commit.
fn runner_clone(dir: &TempDir, repo: &Path) -> (PathBuf, String) {
    let sha = stdout(&run_git(&["rev-parse", "HEAD"], repo))
        .trim()
        .to_string();
    let clone = dir.path().join("runner");
    run_git_ok(
        &[
            "clone",
            "--no-checkout",
            "--filter=blob:none",
            &format!("file://{}", repo.display()),
            clone.to_str().unwrap(),
        ],
        dir.path(),
    );
    run_git_ok(&["remote", "rename", "origin", "promisor"], &clone);
    (clone, sha)
}

/// All `submodule.*` keys in the repo and worktree config.
fn submodule_config(repo: &Path) -> String {
    let mut all = String::new();
    for scope in ["--local", "--worktree"] {
        let out = run_git(&["config", scope, "--get-regexp", r"^submodule\."], repo);
        all.push_str(&stdout(&out));
    }
    all
}

/// Apply before the first checkout: only the cone is ever written, with no
/// `origin` and no network beyond the promisor.
#[test]
fn test_apply_before_first_checkout() {
    let (dir, repo) = create_test_repo();
    let (clone, sha) = runner_clone(&dir, &repo);

    let output = run_workset(&["apply", "backend", "--rev", &sha], &clone);
    assert!(output.status.success(), "apply failed: {}", stderr(&output));
    run_git_ok(&["checkout", "--detach", &sha], &clone);

    assert!(clone.join("src/server/hello.txt").exists());
    assert!(clone.join("src/shared/hello.txt").exists());
    assert!(clone.join(".git-workset.toml").exists(), "root files stay");
    assert!(!clone.join("src/client/hello.txt").exists());
    assert!(!clone.join("assets/hello.txt").exists());
}

/// Requirement 1: apply --json reports each submodule's name, path and state,
/// with `skipped` winning over the cone.
#[test]
fn test_apply_json_reports_submodule_states() {
    let (dir, repo) = create_test_repo();
    let (clone, sha) = runner_clone(&dir, &repo);

    let output = run_workset(&["--json", "apply", "with-sub", "--rev", &sha], &clone);
    assert!(output.status.success(), "apply failed: {}", stderr(&output));
    let doc = json_doc(&output);
    assert_eq!(doc["schema"], "git-workset/apply@1");
    assert_eq!(doc["config_source"]["kind"], "rev");
    assert_eq!(doc["config_source"]["rev"], sha.as_str());
    assert_eq!(
        doc["submodules"],
        serde_json::json!([
            { "name": "ext/lib", "path": "ext/lib", "state": "in_cone" },
            { "name": "ext/lib2", "path": "ext/lib2", "state": "skipped" },
        ])
    );

    // backend skips both; frontend skips both; a profile outside ext/ that
    // skips nothing reports them out of the cone.
    let cfg = "[workset.server-only]\ninclude = [\"src/server\"]\n";
    let output = run_workset_stdin(&["--json", "-f", "-", "apply", "server-only"], &clone, cfg);
    assert!(output.status.success(), "apply failed: {}", stderr(&output));
    let states: Vec<String> = json_doc(&output)["submodules"]
        .as_array()
        .unwrap()
        .iter()
        .map(|s| s["state"].as_str().unwrap().to_string())
        .collect();
    assert_eq!(states, ["out_of_cone", "out_of_cone"]);
}

/// Requirement 2: report and ignore write no submodule config, so a deferred
/// submodule can still be initialised with plain git afterwards.
#[test]
fn test_apply_writes_no_submodule_config_and_deferred_init_works() {
    let (dir, repo) = create_test_repo();
    let (clone, sha) = runner_clone(&dir, &repo);

    for mode in ["report", "ignore"] {
        let output = run_workset(
            &["apply", "with-sub", "--rev", &sha, "--submodules", mode],
            &clone,
        );
        assert!(output.status.success(), "apply failed: {}", stderr(&output));
        assert_eq!(
            submodule_config(&clone),
            "",
            "--submodules={} wrote submodule config",
            mode
        );
    }
    run_git_ok(&["checkout", "--detach", &sha], &clone);
    assert!(
        !clone.join("ext/lib/.git").exists(),
        "apply must not clone submodules"
    );

    // ext/lib2 was reported skipped; the runner can still bring it in later.
    run_git_ok(&["submodule", "update", "--init", "--", "ext/lib2"], &clone);
    assert!(
        clone.join("ext/lib2/lib2.txt").exists(),
        "deferred init should materialise the skipped submodule"
    );
}

/// `--submodules=ignore` leaves the field out, so "none" and "didn't look"
/// differ.
#[test]
fn test_apply_ignore_omits_submodules_field() {
    let (dir, repo) = create_test_repo();
    let (clone, sha) = runner_clone(&dir, &repo);
    let output = run_workset(
        &[
            "--json",
            "apply",
            "with-sub",
            "--rev",
            &sha,
            "--submodules",
            "ignore",
        ],
        &clone,
    );
    assert!(output.status.success(), "apply failed: {}", stderr(&output));
    let doc = json_doc(&output);
    assert!(doc.get("submodules").is_none(), "{}", doc);
}

/// Unreachable submodule URLs do not matter: apply does no submodule I/O.
#[test]
fn test_apply_succeeds_with_unreachable_submodule_urls() {
    let (dir, repo) = create_test_repo();
    std::fs::rename(dir.path().join("subrepo"), dir.path().join("gone")).unwrap();
    let (clone, sha) = runner_clone(&dir, &repo);

    let output = run_workset(&["apply", "with-subs", "--rev", &sha], &clone);
    assert!(
        output.status.success(),
        "apply must not touch submodule remotes: {}",
        stderr(&output)
    );
}

/// Requirement 3: with --json stdout is one JSON document even though git
/// printed output, and git's output is on stderr.
#[test]
fn test_apply_json_stdout_is_pure() {
    let (dir, repo) = create_test_repo();
    run_git_ok(&["branch", "pure"], &repo);
    let wt = dir.path().join("wt-pure");
    let output = run_workset(
        &["carve", wt.to_str().unwrap(), "pure", "-w", "backend"],
        &repo,
    );
    assert!(output.status.success(), "carve failed: {}", stderr(&output));

    // An isolated `git submodule update` prints "Submodule path ... checked
    // out" on git's stdout; it must end up on ours as stderr.
    let output = run_workset(
        &[
            "--json",
            "--isolated-submodules",
            "apply",
            "with-sub",
            "--submodules",
            "manage",
            "--lfs",
            "configure",
        ],
        &wt,
    );
    assert!(output.status.success(), "apply failed: {}", stderr(&output));
    let doc = json_doc(&output);
    assert_eq!(doc["profile"], "with-sub");
    assert!(
        stderr(&output).contains("Submodule path 'ext/lib'"),
        "git's stdout should be forwarded to stderr: {}",
        stderr(&output)
    );
    assert!(
        wt.join("ext/lib/lib.txt").exists(),
        "manage clones the submodule"
    );
}

/// Errors under --json are an error@1 document on stdout with a distinct
/// exit code.
#[test]
fn test_json_errors_and_exit_codes() {
    let (dir, repo) = create_test_repo();
    let (clone, sha) = runner_clone(&dir, &repo);

    let output = run_workset(&["--json", "apply", "srever", "--rev", &sha], &clone);
    assert_eq!(output.status.code(), Some(5), "{}", stderr(&output));
    let doc = json_doc(&output);
    assert_eq!(doc["schema"], "git-workset/error@1");
    assert_eq!(doc["code"], "unknown_profile");
    assert_eq!(doc["details"]["requested"], "srever");
    assert!(doc["details"]["available"]
        .as_array()
        .unwrap()
        .contains(&serde_json::json!("backend")));

    // An empty segment in a+b is an unknown profile, not a crash.
    let output = run_workset(&["--json", "apply", "backend+", "--rev", &sha], &clone);
    assert_eq!(output.status.code(), Some(5));
    assert_eq!(json_doc(&output)["code"], "unknown_profile");

    let output = run_workset_stdin(
        &["--json", "-f", "-", "apply", "a"],
        &clone,
        "[workset.a]\nincldue = []\n",
    );
    assert_eq!(output.status.code(), Some(4));
    let doc = json_doc(&output);
    assert_eq!(doc["code"], "config_invalid");
    assert!(doc["message"].as_str().unwrap().contains("incldue"));

    let output = run_workset_stdin(
        &["--json", "-f", "-", "apply", "a"],
        &clone,
        "version = 9\n[workset.a]\n",
    );
    assert_eq!(output.status.code(), Some(4));

    // A commit with no .git-workset.toml.
    let empty = dir.path().join("bare-history");
    std::fs::create_dir_all(&empty).unwrap();
    run_git_ok(&["init", "-q"], &empty);
    run_git_ok(
        &[
            "-c",
            "user.email=a@b",
            "-c",
            "user.name=t",
            "commit",
            "--allow-empty",
            "-qm",
            "x",
        ],
        &empty,
    );
    let output = run_workset(&["--json", "apply", "a"], &empty);
    assert_eq!(output.status.code(), Some(3), "{}", stderr(&output));
    assert_eq!(json_doc(&output)["code"], "config_missing");

    // Bad flags are clap's: exit 2, empty stdout.
    let output = run_workset(&["--json", "apply", "a", "--submodules", "maybe"], &clone);
    assert_eq!(output.status.code(), Some(2));
    assert_eq!(stdout(&output), "");
}

/// LFS report needs no git-lfs and writes no lfs.* keys.
#[test]
fn test_apply_lfs_report_needs_no_git_lfs() {
    let (dir, repo) = create_test_repo();
    let (clone, sha) = runner_clone(&dir, &repo);
    let home = dir.path().join("home");
    std::fs::create_dir_all(&home).unwrap();

    // A PATH holding only git itself. Git for Windows needs its DLLs and
    // finds git-lfs through its exec-path regardless of PATH, so the
    // restriction is only meaningful (and only applied) on Unix.
    let path = if cfg!(windows) {
        None
    } else {
        let git = stdout(&run_git(&["--exec-path"], &repo)).trim().to_string();
        let bin = dir.path().join("only-git");
        std::fs::create_dir_all(&bin).unwrap();
        std::fs::copy(Path::new(&git).join("git"), bin.join("git")).unwrap();
        Some(bin.display().to_string())
    };

    let cfg = "[workset.art]\ninclude = [\"assets\"]\ninclude_lfs = [\"*.png\"]\nexclude_lfs = [\"*.psd\"]\n";
    let cfg_file = dir.path().join("lfs.toml");
    std::fs::write(&cfg_file, cfg).unwrap();
    let output = run_workset_isolated(
        &["--json", "-f", cfg_file.to_str().unwrap(), "apply", "art"],
        &clone,
        &home,
        path.as_deref(),
    );
    assert!(output.status.success(), "apply failed: {}", stderr(&output));
    let doc = json_doc(&output);
    assert_eq!(doc["lfs"]["include"], serde_json::json!(["*.png"]));
    assert_eq!(doc["lfs"]["exclude"], serde_json::json!(["*.psd"]));
    let lfs = run_git(&["config", "--get-regexp", r"^lfs\."], &clone);
    assert_eq!(stdout(&lfs), "", "no lfs.* key may be written");
    drop(sha);
}

/// apply never writes global config.
#[test]
fn test_apply_writes_no_global_config() {
    let (dir, repo) = create_test_repo();
    let (clone, sha) = runner_clone(&dir, &repo);
    let home = dir.path().join("empty-home");
    std::fs::create_dir_all(&home).unwrap();

    let output = run_workset_isolated(&["apply", "with-sub", "--rev", &sha], &clone, &home, None);
    assert!(output.status.success(), "apply failed: {}", stderr(&output));
    assert!(
        !home.join(".gitconfig").exists(),
        "apply created a global config"
    );
}

/// `--no-marker` leaves no workset state behind; the default records it.
#[test]
fn test_apply_marker_is_optional() {
    let (dir, repo) = create_test_repo();
    let (clone, sha) = runner_clone(&dir, &repo);
    let marker = clone.join(".git/workset");

    let output = run_workset(&["apply", "backend", "--rev", &sha, "--no-marker"], &clone);
    assert!(output.status.success(), "apply failed: {}", stderr(&output));
    assert!(!marker.exists(), "--no-marker wrote the marker");

    let output = run_workset(&["apply", "backend", "--rev", &sha], &clone);
    assert!(output.status.success(), "apply failed: {}", stderr(&output));
    assert_eq!(std::fs::read_to_string(&marker).unwrap(), "backend");
}

/// `-C` targets another checkout; config comes from that checkout's HEAD.
#[test]
fn test_apply_dash_c_reads_target_head() {
    let (dir, repo) = create_test_repo();
    let wt = dir.path().join("wt-dash-c");
    carve_ok(&repo, &wt, "dash-c", "backend");
    let cfg = std::fs::read_to_string(wt.join(".git-workset.toml"))
        .unwrap()
        .replace(
            "include = [\"src/client\", \"src/shared\"]",
            "include = [\"assets\"]",
        );
    std::fs::write(wt.join(".git-workset.toml"), cfg).unwrap();
    run_git_ok(&["commit", "-am", "branch-local frontend"], &wt);

    let output = run_workset(&["apply", "frontend", "-C", wt.to_str().unwrap()], &repo);
    assert!(output.status.success(), "apply failed: {}", stderr(&output));
    assert!(wt.join("assets/hello.txt").exists());
    assert!(!wt.join("src/client/hello.txt").exists());
}

/// `profiles -f` works outside any repository and keeps file order.
#[test]
fn test_profiles_outside_repo_in_file_order() {
    let dir = TempDir::new().unwrap();
    let cfg = "[workset.zeta]\ndescription = \"last alphabetically\"\ninclude = [\"z\"]\n\
               [workset.alpha]\ninclude = [\"a\"]\n[workset.alpha.submodules]\nskip = [\"x\"]\n";
    let output = run_workset_stdin(&["--json", "-f", "-", "profiles"], dir.path(), cfg);
    assert!(
        output.status.success(),
        "profiles failed: {}",
        stderr(&output)
    );
    let doc = json_doc(&output);
    assert_eq!(doc["schema"], "git-workset/profiles@1");
    assert_eq!(doc["config_version"], 1);
    let names: Vec<&str> = doc["profiles"]
        .as_array()
        .unwrap()
        .iter()
        .map(|p| p["name"].as_str().unwrap())
        .collect();
    assert_eq!(names, ["zeta", "alpha"], "file order, not alphabetical");
    assert_eq!(doc["profiles"][0]["description"], "last alphabetically");
    assert_eq!(
        doc["profiles"][1]["submodules"]["skip"],
        serde_json::json!(["x"])
    );
}

/// `profiles --rev` reads a commit; a missing config is exit 3.
#[test]
fn test_profiles_from_rev() {
    let (dir, repo) = create_test_repo();
    let (clone, sha) = runner_clone(&dir, &repo);
    let output = run_workset(&["--json", "profiles", "--rev", &sha], &clone);
    assert!(
        output.status.success(),
        "profiles failed: {}",
        stderr(&output)
    );
    let first = &json_doc(&output)["profiles"][0];
    assert_eq!(first["name"], "backend");
    assert_eq!(
        first["include"],
        serde_json::json!(["src/server", "src/shared"])
    );
}

// ---- v0.6.1: submodules.shallow default ----

/// A profile with no `[workset.x.submodules]` table clones submodules
/// shallow, as documented.
#[test]
fn test_shallow_defaults_to_true_without_submodules_table() {
    let (dir, repo) = create_test_repo();
    // Give ext/lib two commits so depth 1 actually cuts history.
    let sub_repo = dir.path().join("subrepo");
    std::fs::write(sub_repo.join("lib.txt"), "second").unwrap();
    run_git_ok(&["commit", "-am", "sub second"], &sub_repo);
    let checkout = repo.join("ext/lib");
    run_git_ok(&["pull", "-q", "origin", "main"], &checkout);
    run_git_ok(&["commit", "-am", "bump ext/lib"], &repo);

    // `with-subs` defines no submodules table.
    run_git_ok(&["branch", "shallow-default"], &repo);
    let wt = dir.path().join("wt-shallow-default");
    let output = run_workset(
        &[
            "--isolated-submodules",
            "carve",
            wt.to_str().unwrap(),
            "shallow-default",
            "-w",
            "with-subs",
        ],
        &repo,
    );
    assert!(output.status.success(), "carve failed: {}", stderr(&output));
    let shallow = stdout(&run_git(
        &["rev-parse", "--is-shallow-repository"],
        &wt.join("ext/lib"),
    ));
    assert_eq!(
        shallow.trim(),
        "true",
        "submodule should be a shallow clone"
    );

    let output = run_workset(&["--json", "profiles"], &repo);
    assert!(output.status.success(), "{}", stderr(&output));
    let doc = json_doc(&output);
    let with_subs = doc["profiles"]
        .as_array()
        .unwrap()
        .iter()
        .find(|p| p["name"] == "with-subs")
        .unwrap();
    assert_eq!(with_subs["submodules"]["shallow"], true);
}
