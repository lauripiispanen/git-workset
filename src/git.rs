use anyhow::{bail, Context, Result};
use std::path::{Path, PathBuf};
use std::process::Command;

use crate::config::Workset;

/// Find the root of the main git repository (not a worktree).
pub fn find_repo_root() -> Result<PathBuf> {
    let output = Command::new("git")
        .args(["rev-parse", "--git-common-dir"])
        .output()
        .context("Failed to run git")?;
    if !output.status.success() {
        bail!("Not inside a git repository");
    }
    let git_common_dir = String::from_utf8(output.stdout)?.trim().to_string();
    let common_path = PathBuf::from(&git_common_dir);

    // git-common-dir returns the .git directory; we want the parent
    if common_path.ends_with(".git") {
        let parent = common_path
            .parent()
            .map(|p| p.to_path_buf())
            .unwrap_or_else(|| PathBuf::from("."));
        // When .git is relative (common case), parent is "" — normalize to "."
        Ok(if parent.as_os_str().is_empty() {
            PathBuf::from(".")
        } else {
            parent
        })
    } else {
        // bare repo or worktree — resolve to absolute
        let abs = std::fs::canonicalize(&common_path)?;
        Ok(abs.parent().map(|p| p.to_path_buf()).unwrap_or(abs))
    }
}

/// Get the current worktree's git dir (e.g. .git/worktrees/<name>)
pub fn worktree_git_dir(worktree_path: &Path) -> Result<PathBuf> {
    let output = Command::new("git")
        .args(["rev-parse", "--git-dir"])
        .current_dir(worktree_path)
        .output()
        .context("Failed to run git")?;
    if !output.status.success() {
        bail!("Not a git worktree: {}", worktree_path.display());
    }
    let dir = String::from_utf8(output.stdout)?.trim().to_string();
    let path = PathBuf::from(&dir);
    if path.is_absolute() {
        Ok(path)
    } else {
        Ok(worktree_path.join(path))
    }
}

fn run_git(args: &[&str], cwd: &Path) -> Result<()> {
    let display_args = args.join(" ");
    eprintln!("  git {}", display_args);
    let status = Command::new("git")
        .args(args)
        .current_dir(cwd)
        .status()
        .with_context(|| format!("Failed to run: git {}", display_args))?;
    if !status.success() {
        bail!(
            "git {} failed with exit code {:?}",
            display_args,
            status.code()
        );
    }
    Ok(())
}

fn run_git_output(args: &[&str], cwd: &Path) -> Result<String> {
    let output = Command::new("git")
        .args(args)
        .current_dir(cwd)
        .output()
        .with_context(|| format!("Failed to run: git {}", args.join(" ")))?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        bail!("git {} failed: {}", args.join(" "), stderr.trim());
    }
    Ok(String::from_utf8(output.stdout)?.trim().to_string())
}

/// Clone via init → sparse → fetch → checkout to avoid processing excluded
/// files. This is much faster than clone → sparse → checkout because sparse
/// checkout is configured before any checkout happens, so git never iterates
/// the full tree through smudge filters.
pub fn sparse_clone(
    url: &str,
    path: &Path,
    branch: Option<&str>,
    depth: Option<u32>,
    workset: &Workset,
) -> Result<()> {
    let path_str = path.to_str().context("Invalid path")?;

    // 1. git init
    run_git(&["init", path_str], &std::env::current_dir()?)?;

    // 2. Add remote
    run_git(&["remote", "add", "origin", url], path)?;

    // 3. Configure sparse checkout BEFORE any fetch/checkout
    //    Skip if both include and exclude are empty (full tree).
    if !workset.include.is_empty() || !workset.exclude.is_empty() {
        let (use_cone, patterns) = build_sparse_args(workset);

        if use_cone {
            run_git(&["sparse-checkout", "init", "--cone"], path)?;
        } else {
            run_git(&["sparse-checkout", "init"], path)?;
        }

        let mut sparse_args: Vec<&str> = vec!["sparse-checkout", "set"];
        let pattern_refs: Vec<&str> = patterns.iter().map(|s| s.as_str()).collect();
        sparse_args.extend(&pattern_refs);
        if !use_cone {
            sparse_args.push("--no-cone");
        }
        run_git(&sparse_args, path)?;
    }

    // 4. Fetch (optionally shallow)
    let depth_str;
    let mut fetch_args = vec!["fetch"];
    if let Some(d) = depth {
        depth_str = d.to_string();
        fetch_args.push("--depth");
        fetch_args.push(&depth_str);
    }
    let refspec = branch.unwrap_or("HEAD");
    fetch_args.push("origin");
    fetch_args.push(refspec);

    let fetch_status = Command::new("git")
        .env("GIT_LFS_SKIP_SMUDGE", "1")
        .args(&fetch_args)
        .current_dir(path)
        .status()
        .context("Failed to fetch")?;
    if !fetch_status.success() {
        bail!("git fetch failed");
    }

    // 5. Set up branch tracking and checkout
    if let Some(b) = branch {
        // Create local branch tracking the remote
        let remote_ref = format!("origin/{}", b);
        run_git(&["checkout", "-b", b, &remote_ref], path)?;
    } else {
        run_git(&["checkout", "FETCH_HEAD"], path)?;
    }

    Ok(())
}

/// Deepen a shallow clone or fully unshallow it.
pub fn deepen(repo_path: &Path, depth: Option<u32>) -> Result<()> {
    match depth {
        Some(n) => {
            let n_str = n.to_string();
            run_git(&["fetch", "--deepen", &n_str], repo_path)
        }
        None => run_git(&["fetch", "--unshallow"], repo_path),
    }
}

/// Create a worktree, skipping LFS smudge.
pub fn add_worktree(path: &Path, branch: &str) -> Result<()> {
    let path_str = path.to_str().context("Invalid path")?;
    eprintln!(
        "  GIT_LFS_SKIP_SMUDGE=1 git worktree add {} {}",
        path_str, branch
    );
    let status = Command::new("git")
        .env("GIT_LFS_SKIP_SMUDGE", "1")
        .args(["worktree", "add", path_str, branch])
        .status()
        .context("Failed to create worktree")?;
    if !status.success() {
        bail!("git worktree add failed");
    }
    Ok(())
}

/// Build the sparse-checkout set arguments for a workset.
/// When excludes are present, forces --no-cone mode and generates negated patterns.
fn build_sparse_args(workset: &Workset) -> (bool, Vec<String>) {
    let use_cone = workset.sparse_cone && workset.exclude.is_empty();

    let mut patterns: Vec<String> = workset.include.to_vec();

    if !workset.exclude.is_empty() {
        // In no-cone mode, ensure we have a catch-all include
        if patterns.is_empty() {
            patterns.push("/*".to_string());
        }
        for dir in &workset.exclude {
            patterns.push(format!("!/{}/", dir));
        }
    }

    (use_cone, patterns)
}

/// Apply sparse checkout configuration to a worktree.
/// If both include and exclude are empty, sparse checkout is skipped (full tree).
pub fn apply_sparse_checkout(worktree_path: &Path, workset: &Workset) -> Result<()> {
    if workset.include.is_empty() && workset.exclude.is_empty() {
        // No sparse checkout — disable it if it was previously enabled
        let _ = run_git(&["sparse-checkout", "disable"], worktree_path);
        return Ok(());
    }

    let (use_cone, patterns) = build_sparse_args(workset);

    if use_cone {
        run_git(&["sparse-checkout", "init", "--cone"], worktree_path)?;
    } else {
        run_git(&["sparse-checkout", "init"], worktree_path)?;
    }

    let mut args: Vec<&str> = vec!["sparse-checkout", "set"];
    let pattern_refs: Vec<&str> = patterns.iter().map(|s| s.as_str()).collect();
    args.extend(&pattern_refs);

    if !use_cone {
        args.push("--no-cone");
    }

    run_git(&args, worktree_path)?;
    Ok(())
}

/// Enable worktree-scoped config so we can set per-worktree settings
/// without affecting the main repo's .git/config.
pub fn enable_worktree_config(worktree_path: &Path) -> Result<()> {
    run_git(
        &["config", "extensions.worktreeConfig", "true"],
        worktree_path,
    )?;
    // Override any global/repo submodule.recurse=true so that fetch only
    // recurses into active submodules, not all registered ones.
    run_git(
        &[
            "config",
            "--worktree",
            "fetch.recurseSubmodules",
            "on-demand",
        ],
        worktree_path,
    )?;
    run_git(
        &["config", "--worktree", "submodule.recurse", "false"],
        worktree_path,
    )
}

/// Parse .gitmodules and return (name, path) pairs for all submodules.
fn parse_submodule_entries(worktree_path: &Path) -> Result<Vec<(String, String)>> {
    if !worktree_path.join(".gitmodules").exists() {
        return Ok(vec![]);
    }

    let output = run_git_output(
        &[
            "config",
            "--file",
            ".gitmodules",
            "--get-regexp",
            r"submodule\..*\.path",
        ],
        worktree_path,
    )?;

    Ok(output
        .lines()
        .filter_map(|line| {
            // Format: "submodule.<name>.path <path>"
            let (key, path) = line.split_once(' ')?;
            let name = key.strip_prefix("submodule.")?.strip_suffix(".path")?;
            Some((name.to_string(), path.to_string()))
        })
        .collect())
}

/// Initialize submodules according to workset config.
pub fn init_submodules(worktree_path: &Path, workset: &Workset) -> Result<()> {
    let entries = parse_submodule_entries(worktree_path)?;

    let mut wanted_paths: Vec<String> = Vec::new();

    for (name, path) in &entries {
        if workset.submodules.skip.iter().any(|s| s == path) {
            eprintln!("  skipping submodule: {}", path);
            // Mark as inactive in worktree-scoped config so git pull/fetch
            // won't try to access it
            run_git(
                &[
                    "config",
                    "--worktree",
                    &format!("submodule.{}.active", name),
                    "false",
                ],
                worktree_path,
            )?;
        } else {
            wanted_paths.push(path.clone());
        }
    }

    if wanted_paths.is_empty() {
        return Ok(());
    }

    // Init and update only the wanted submodules by passing explicit paths.
    // This is necessary because worktrees share .git/config with the main
    // worktree, so submodules initialized there would otherwise all get cloned.
    let mut args = vec!["submodule", "update", "--init"];
    if workset.submodules.shallow {
        args.push("--depth");
        args.push("1");
    }
    args.push("--");
    let refs: Vec<&str> = wanted_paths.iter().map(|s| s.as_str()).collect();
    args.extend(&refs);
    run_git(&args, worktree_path)?;

    Ok(())
}

/// Configure LFS fetch include/exclude and optionally pull.
/// Uses --worktree scoped config so the main repo is unaffected.
pub fn configure_lfs(worktree_path: &Path, workset: &Workset) -> Result<()> {
    if !workset.include_lfs.is_empty() {
        let include_val = workset.include_lfs.join(",");
        run_git(
            &["config", "--worktree", "lfs.fetchinclude", &include_val],
            worktree_path,
        )?;
    }

    if !workset.exclude_lfs.is_empty() {
        let exclude_val = workset.exclude_lfs.join(",");
        run_git(
            &["config", "--worktree", "lfs.fetchexclude", &exclude_val],
            worktree_path,
        )?;
    }

    // Pull LFS content matching the filters
    if !workset.include_lfs.is_empty() || !workset.exclude_lfs.is_empty() {
        run_git(&["lfs", "pull"], worktree_path)?;
    }

    Ok(())
}

/// Store which workset name is active in this worktree.
pub fn store_workset_name(worktree_path: &Path, workset_name: &str) -> Result<()> {
    let git_dir = worktree_git_dir(worktree_path)?;
    let marker = git_dir.join("workset");
    std::fs::write(&marker, workset_name)
        .with_context(|| format!("Failed to write {}", marker.display()))?;
    Ok(())
}

/// Read the active workset name for a worktree.
pub fn read_workset_name(worktree_path: &Path) -> Result<Option<String>> {
    let git_dir = worktree_git_dir(worktree_path)?;
    let marker = git_dir.join("workset");
    match std::fs::read_to_string(&marker) {
        Ok(name) => Ok(Some(name.trim().to_string())),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e).context("Failed to read workset marker"),
    }
}

/// List all worktrees with their paths and branches.
pub fn list_worktrees() -> Result<Vec<(PathBuf, String)>> {
    let output = run_git_output(
        &["worktree", "list", "--porcelain"],
        &std::env::current_dir()?,
    )?;
    let mut worktrees = Vec::new();
    let mut current_path: Option<PathBuf> = None;
    let mut current_branch = String::new();

    for line in output.lines() {
        if let Some(path) = line.strip_prefix("worktree ") {
            current_path = Some(PathBuf::from(path));
            current_branch.clear();
        } else if let Some(branch) = line.strip_prefix("branch refs/heads/") {
            current_branch = branch.to_string();
        } else if line.starts_with("HEAD ") {
            // detached HEAD — use the SHA
            if current_branch.is_empty() {
                if let Some(sha) = line.strip_prefix("HEAD ") {
                    current_branch = format!("(detached {})", &sha[..8.min(sha.len())]);
                }
            }
        } else if line.is_empty() {
            if let Some(path) = current_path.take() {
                worktrees.push((path, std::mem::take(&mut current_branch)));
            }
        }
    }
    // Handle last entry if no trailing blank line
    if let Some(path) = current_path {
        worktrees.push((path, current_branch));
    }

    Ok(worktrees)
}

/// Remove a worktree.
pub fn remove_worktree(path: &Path) -> Result<()> {
    let path_str = path.to_str().context("Invalid path")?;
    run_git(&["worktree", "remove", path_str], &std::env::current_dir()?)
}
