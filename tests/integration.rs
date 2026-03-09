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

    // Add it as a submodule (using a relative path so it resolves locally)
    let sub_url = format!("file://{}", sub_repo.display());
    run_git_ok(&["submodule", "add", &sub_url, "ext/lib"], &repo);

    // Create .git-workset.toml
    let config = r#"
[workset.backend]
description = "Backend services"
include = ["src/server", "src/shared"]

[workset.backend.submodules]
skip = ["ext/lib"]

[workset.frontend]
description = "Frontend client"
include = ["src/client", "src/shared"]

[workset.frontend.submodules]
skip = ["ext/lib"]

[workset.all]
description = "Everything"
include = ["src/server", "src/client", "src/shared", "assets", "ext"]
"#;
    std::fs::write(repo.join(".git-workset.toml"), config).unwrap();

    run_git_ok(&["add", "-A"], &repo);
    run_git_ok(&["commit", "-m", "initial commit"], &repo);

    (dir, repo)
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
