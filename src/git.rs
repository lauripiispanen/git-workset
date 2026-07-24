use anyhow::{bail, Context, Result};
use std::path::{Path, PathBuf};
use std::process::Command;

use crate::config::{SubmoduleSharing, Workset};

/// Oldest git that honours `extensions.worktreeConfig`.
const MIN_WORKTREE_CONFIG_VERSION: (u32, u32) = (2, 20);

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
    let root = if common_path.ends_with(".git") {
        let parent = common_path
            .parent()
            .map(|p| p.to_path_buf())
            .unwrap_or_else(|| PathBuf::from("."));
        // When .git is relative (common case), parent is "" — normalize to "."
        if parent.as_os_str().is_empty() {
            PathBuf::from(".")
        } else {
            parent
        }
    } else {
        // bare repo or worktree — resolve to absolute
        let abs = std::fs::canonicalize(&common_path)?;
        abs.parent().map(|p| p.to_path_buf()).unwrap_or(abs)
    };

    // Always hand back an absolute path: submodule sharing builds paths that
    // are used from other working directories.
    Ok(abs_path(&root))
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

pub(crate) fn run_git(args: &[&str], cwd: &Path) -> Result<()> {
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

/// Like `run_git`, but capture output instead of inheriting stdio. Used for
/// steps that are allowed to fail and fall back, so a failed attempt does not
/// spray git's error text over the console before we print our own warning.
pub(crate) fn try_git(args: &[&str], cwd: &Path) -> Result<()> {
    let display_args = args.join(" ");
    eprintln!("  git {}", display_args);
    let output = Command::new("git")
        .args(args)
        .env("GIT_LFS_SKIP_SMUDGE", "1")
        .current_dir(cwd)
        .output()
        .with_context(|| format!("Failed to run: git {}", display_args))?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        bail!("git {} failed: {}", display_args, stderr.trim());
    }
    Ok(())
}

/// Run git against an explicit gitdir. Submodule gitdirs are addressed this
/// way so the commands work even when the main checkout of the submodule is
/// missing.
pub(crate) fn gitdir_args<'a>(gitdir: &'a str, args: &[&'a str]) -> Vec<&'a str> {
    let mut v = vec!["--git-dir", gitdir];
    v.extend_from_slice(args);
    v
}

pub(crate) fn run_git_output(args: &[&str], cwd: &Path) -> Result<String> {
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

/// How to handle branch creation when adding a worktree.
pub enum WorktreeBranch {
    /// Check out an existing branch or commit.
    Existing(String),
    /// Create a new branch (`-b`).
    Create(String),
    /// Create or reset a branch (`-B`).
    ForceCreate(String),
    /// Let git decide (default: new branch named after the path basename).
    Auto,
}

/// Create a worktree, skipping LFS smudge.
pub fn add_worktree(path: &Path, branch: WorktreeBranch, commit_ish: Option<&str>) -> Result<()> {
    let path_str = path.to_str().context("Invalid path")?;

    let mut args: Vec<String> = vec!["worktree".into(), "add".into()];

    match &branch {
        WorktreeBranch::Create(name) => {
            args.push("-b".into());
            args.push(name.clone());
        }
        WorktreeBranch::ForceCreate(name) => {
            args.push("-B".into());
            args.push(name.clone());
        }
        _ => {}
    }

    args.push(path_str.into());

    match &branch {
        WorktreeBranch::Existing(b) => args.push(b.clone()),
        WorktreeBranch::Create(_) | WorktreeBranch::ForceCreate(_) => {
            if let Some(c) = commit_ish {
                args.push(c.into());
            }
        }
        WorktreeBranch::Auto => {}
    }

    let display_args = args.join(" ");
    eprintln!("  GIT_LFS_SKIP_SMUDGE=1 git {}", display_args);
    let arg_refs: Vec<&str> = args.iter().map(|s| s.as_str()).collect();
    let status = Command::new("git")
        .env("GIT_LFS_SKIP_SMUDGE", "1")
        .args(&arg_refs)
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
pub(crate) fn parse_submodule_entries(worktree_path: &Path) -> Result<Vec<(String, String)>> {
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

/// The installed git version as (major, minor).
pub fn git_version() -> Result<(u32, u32)> {
    let out = run_git_output(&["--version"], &std::env::current_dir()?)?;
    // "git version 2.43.0" / "git version 2.39.3 (Apple Git-145)"
    let nums = out
        .split_whitespace()
        .find(|w| w.chars().next().is_some_and(|c| c.is_ascii_digit()))
        .with_context(|| format!("Could not parse git version from '{}'", out))?;
    let mut parts = nums.split('.');
    let major: u32 = parts
        .next()
        .and_then(|s| s.parse().ok())
        .with_context(|| format!("Could not parse git version from '{}'", out))?;
    let minor: u32 = parts.next().and_then(|s| s.parse().ok()).unwrap_or(0);
    Ok((major, minor))
}

/// Absolute path of the main clone's `.git` directory.
pub(crate) fn main_git_dir(main_repo: &Path) -> Result<PathBuf> {
    Ok(PathBuf::from(run_git_output(
        &["rev-parse", "--absolute-git-dir"],
        main_repo,
    )?))
}

/// Absolute gitdir backing a submodule checkout in the main worktree.
pub fn submodule_gitdir(main_repo: &Path, sub_path: &str) -> Result<PathBuf> {
    let checkout = main_repo.join(sub_path);
    if !checkout.join(".git").exists() {
        bail!(
            "submodule '{}' is not checked out in the main worktree",
            sub_path
        );
    }
    Ok(PathBuf::from(run_git_output(
        &["rev-parse", "--absolute-git-dir"],
        &checkout,
    )?))
}

/// Locate the shared gitdir for a submodule, initializing it in the main
/// worktree if it does not exist yet.
pub fn ensure_submodule_gitdir(
    main_repo: &Path,
    name: &str,
    sub_path: &str,
    shallow: bool,
) -> Result<PathBuf> {
    if let Ok(dir) = submodule_gitdir(main_repo, sub_path) {
        if dir.join("HEAD").exists() {
            return Ok(dir);
        }
    }

    // The checkout may be deinit'd while the object store is still there.
    let guess = main_git_dir(main_repo)?.join("modules").join(name);
    if guess.join("HEAD").exists() {
        return Ok(guess);
    }

    eprintln!("  initializing shared submodule store for {}", sub_path);
    let mut args = vec!["submodule", "update", "--init"];
    if shallow {
        args.push("--depth");
        args.push("1");
    }
    args.push("--");
    args.push(sub_path);
    try_git(&args, main_repo)?;

    submodule_gitdir(main_repo, sub_path)
}

/// An absolute path safe to hand to git as a config value.
///
/// `fs::canonicalize` returns verbatim (`\\?\C:\...`) paths on Windows, which
/// git does not understand. Strip that prefix for plain drive paths.
pub(crate) fn abs_path(path: &Path) -> PathBuf {
    let resolved = std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
    strip_verbatim(resolved)
}

#[cfg(windows)]
fn strip_verbatim(path: PathBuf) -> PathBuf {
    let text = path.to_string_lossy().to_string();
    match text.strip_prefix(r"\\?\") {
        // \\?\UNC\server\share has no plain equivalent — leave it alone.
        Some(rest) if !rest.starts_with("UNC\\") => PathBuf::from(rest),
        _ => path,
    }
}

#[cfg(not(windows))]
fn strip_verbatim(path: PathBuf) -> PathBuf {
    path
}

/// A working directory outside any repository.
///
/// `git config -f <file>` still performs repository discovery from the cwd,
/// and setup dies with `Invalid path` if the discovered repo has a broken
/// `core.worktree` — which is exactly the state we are trying to repair. Run
/// these commands from somewhere git will find nothing.
fn neutral_cwd() -> PathBuf {
    let tmp = std::env::temp_dir();
    if tmp.is_dir() {
        tmp
    } else {
        PathBuf::from("/")
    }
}

fn config_file_command(file: &str) -> Command {
    let cwd = neutral_cwd();
    let mut cmd = Command::new("git");
    cmd.env("GIT_CEILING_DIRECTORIES", &cwd)
        .current_dir(cwd)
        .args(["config", "-f", file]);
    cmd
}

pub(crate) fn config_file_get(file: &Path, key: &str) -> Option<String> {
    if !file.exists() {
        return None;
    }
    let output = config_file_command(file.to_str()?)
        .args(["--get", key])
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

pub(crate) fn config_file_set(file: &Path, key: &str, value: &str) -> Result<()> {
    let file_str = file.to_str().context("Invalid config path")?;
    eprintln!("  git config -f {} {} {}", file_str, key, value);
    let status = config_file_command(file_str)
        .args([key, value])
        .status()
        .with_context(|| format!("Failed to set {} in {}", key, file_str))?;
    if !status.success() {
        bail!("git config -f {} {} failed", file_str, key);
    }
    Ok(())
}

pub(crate) fn config_file_unset(file: &Path, key: &str) -> Result<()> {
    let file_str = file.to_str().context("Invalid config path")?;
    eprintln!("  git config -f {} --unset {}", file_str, key);
    let status = config_file_command(file_str)
        .args(["--unset", key])
        .status()
        .with_context(|| format!("Failed to unset {} in {}", key, file_str))?;
    if !status.success() {
        bail!("git config -f {} --unset {} failed", file_str, key);
    }
    Ok(())
}

/// Move `core.worktree` out of a submodule's shared `config` and into
/// per-worktree config, so that a plain `git submodule update` run inside any
/// one checkout can no longer redirect the others.
///
/// Idempotent: safe to call on an already-hardened gitdir. When
/// `main_worktree` is given and no `core.worktree` is recorded anywhere, the
/// main checkout's value is (re)written so the main clone keeps resolving.
pub fn harden_submodule_config(subgit: &Path, main_worktree: Option<&Path>) -> Result<()> {
    let shared = subgit.join("config");
    let per_worktree = subgit.join("config.worktree");

    if !shared.exists() {
        bail!("not a git directory: {}", subgit.display());
    }

    config_file_set(&shared, "extensions.worktreeConfig", "true")?;

    match config_file_get(&shared, "core.worktree") {
        Some(value) => {
            if config_file_get(&per_worktree, "core.worktree").is_none() {
                config_file_set(&per_worktree, "core.worktree", &value)?;
            }
            config_file_unset(&shared, "core.worktree")?;
        }
        None => {
            if config_file_get(&per_worktree, "core.worktree").is_none() {
                if let Some(main_wt) = main_worktree {
                    if main_wt.exists() {
                        let abs = abs_path(main_wt);
                        config_file_set(
                            &per_worktree,
                            "core.worktree",
                            abs.to_str().context("Invalid worktree path")?,
                        )?;
                    }
                }
            }
        }
    }

    Ok(())
}

fn object_present(subgit: &str, pin: &str) -> bool {
    Command::new("git")
        .args(gitdir_args(
            subgit,
            &["cat-file", "-e", &format!("{}^{{commit}}", pin)],
        ))
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

/// Make sure the pinned commit exists in the shared store, fetching it if not.
pub fn ensure_pin_present(subgit: &Path, pin: &str, shallow: bool) -> Result<()> {
    let subgit_str = subgit.to_str().context("Invalid gitdir path")?;
    if object_present(subgit_str, pin) {
        return Ok(());
    }

    eprintln!(
        "  fetching missing submodule commit {}",
        &pin[..8.min(pin.len())]
    );
    let mut fetch: Vec<&str> = vec!["fetch"];
    if shallow {
        fetch.push("--depth");
        fetch.push("1");
    }
    fetch.push("origin");
    fetch.push(pin);
    let cwd = subgit.to_path_buf();
    let _ = try_git(&gitdir_args(subgit_str, &fetch), &cwd);

    if !object_present(subgit_str, pin) {
        // Some servers refuse to serve an arbitrary SHA; fall back to all refs.
        let _ = try_git(&gitdir_args(subgit_str, &["fetch", "origin"]), &cwd);
    }

    if !object_present(subgit_str, pin) {
        bail!("commit {} is not available in the submodule remote", pin);
    }
    Ok(())
}

/// Attach a workset's submodule checkout as a worktree of the shared gitdir.
pub fn attach_submodule_worktree(
    main_repo: &Path,
    name: &str,
    sub_path: &str,
    workset_path: &Path,
    pin: &str,
    shallow: bool,
) -> Result<()> {
    let subgit = ensure_submodule_gitdir(main_repo, name, sub_path, shallow)?;
    harden_submodule_config(&subgit, Some(&main_repo.join(sub_path)))?;
    ensure_pin_present(&subgit, pin, shallow)?;

    let subgit_str = subgit.to_str().context("Invalid gitdir path")?;
    let target = workset_path.join(sub_path);
    let target_str = target.to_str().context("Invalid submodule path")?;

    if let Some(parent) = target.parent() {
        std::fs::create_dir_all(parent)?;
    }

    // Clear stale registrations so re-carving at a path that was rm -rf'd works.
    let _ = try_git(&gitdir_args(subgit_str, &["worktree", "prune"]), main_repo);

    if let Err(e) = try_git(
        &gitdir_args(
            subgit_str,
            &["worktree", "add", "--detach", target_str, pin],
        ),
        main_repo,
    ) {
        // Leave nothing half-attached behind for the isolated fallback.
        let _ = try_git(
            &gitdir_args(subgit_str, &["worktree", "remove", "--force", target_str]),
            main_repo,
        );
        let _ = try_git(&gitdir_args(subgit_str, &["worktree", "prune"]), main_repo);
        return Err(e);
    }

    pin_worktree_core_worktree(&target)?;
    Ok(())
}

/// Record an absolute `core.worktree` in a linked worktree's own config, so a
/// clobbered value in the shared config cannot break this checkout.
fn pin_worktree_core_worktree(checkout: &Path) -> Result<()> {
    let wt_gitdir = PathBuf::from(run_git_output(
        &["rev-parse", "--absolute-git-dir"],
        checkout,
    )?);
    let abs = abs_path(checkout);
    config_file_set(
        &wt_gitdir.join("config.worktree"),
        "core.worktree",
        abs.to_str().context("Invalid worktree path")?,
    )
}

/// Detach a workset's submodule checkout from the shared gitdir.
pub fn detach_submodule_worktree(
    main_repo: &Path,
    sub_path: &str,
    workset_sub_path: &Path,
) -> Result<()> {
    let subgit = submodule_gitdir(main_repo, sub_path)?;
    let subgit_str = subgit.to_str().context("Invalid gitdir path")?;
    let target = workset_sub_path
        .to_str()
        .context("Invalid submodule path")?;
    try_git(
        &gitdir_args(subgit_str, &["worktree", "remove", "--force", target]),
        main_repo,
    )
}

/// Is this checkout a worktree of the shared submodule gitdir already?
fn is_shared_checkout(subgit: &Path, checkout: &Path) -> bool {
    if !checkout.join(".git").exists() {
        return false;
    }
    match run_git_output(&["rev-parse", "--absolute-git-dir"], checkout) {
        Ok(dir) => PathBuf::from(dir).starts_with(subgit.join("worktrees")),
        Err(_) => false,
    }
}

fn is_dirty(checkout: &Path) -> bool {
    match run_git_output(&["status", "--porcelain"], checkout) {
        Ok(out) => !out.trim().is_empty(),
        Err(_) => false,
    }
}

/// Counts for the per-carve summary line.
#[derive(Debug, Default)]
pub struct SubmoduleOutcome {
    pub shared: usize,
    pub cloned: usize,
    pub skipped: usize,
}

fn same_path(a: &Path, b: &Path) -> bool {
    match (std::fs::canonicalize(a), std::fs::canonicalize(b)) {
        (Ok(x), Ok(y)) => x == y,
        _ => a == b,
    }
}

/// Initialize submodules according to workset config and the resolved
/// object-store sharing mode.
pub fn init_submodules(
    worktree_path: &Path,
    main_repo: &Path,
    workset: &Workset,
    sharing: SubmoduleSharing,
) -> Result<SubmoduleOutcome> {
    let entries = parse_submodule_entries(worktree_path)?;
    let mut outcome = SubmoduleOutcome::default();

    let mut wanted: Vec<(String, String)> = Vec::new();

    for (name, path) in &entries {
        if workset.submodules.skip.iter().any(|s| s == path) {
            eprintln!("  skipping submodule: {}", path);
            outcome.skipped += 1;
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
            wanted.push((name.clone(), path.clone()));
        }
    }

    if wanted.is_empty() {
        return Ok(outcome);
    }

    // The main worktree *is* the shared store — there is nothing to attach to.
    let mut mode = sharing;
    if mode == SubmoduleSharing::Shared && same_path(worktree_path, main_repo) {
        mode = SubmoduleSharing::Isolated;
    }
    if mode == SubmoduleSharing::Shared {
        match git_version() {
            Ok(v) if v < MIN_WORKTREE_CONFIG_VERSION => {
                eprintln!(
                    "  warning: git {}.{} does not support extensions.worktreeConfig \
                     (needs {}.{}); falling back to isolated submodules",
                    v.0, v.1, MIN_WORKTREE_CONFIG_VERSION.0, MIN_WORKTREE_CONFIG_VERSION.1
                );
                mode = SubmoduleSharing::Isolated;
            }
            _ => {}
        }
    }

    let mut isolated: Vec<String> = Vec::new();

    if mode == SubmoduleSharing::Shared {
        // Refuse the whole operation before deleting anything if a duplicate
        // clone we would migrate has uncommitted work in it.
        let dupes = duplicate_gitdirs(worktree_path, &wanted)?;
        for (path, _) in &dupes {
            let checkout = worktree_path.join(path);
            if is_dirty(&checkout) {
                bail!(
                    "submodule '{}' has local modifications in {} and cannot be migrated \
                     to a shared object store.\nCommit or stash them, or re-run with \
                     --isolated-submodules.",
                    path,
                    checkout.display()
                );
            }
        }

        for (name, path) in &wanted {
            match share_submodule(
                worktree_path,
                main_repo,
                name,
                path,
                workset.submodules.shallow,
            ) {
                Ok(()) => outcome.shared += 1,
                Err(e) => {
                    eprintln!(
                        "  warning: sharing submodule '{}' failed ({}); using an isolated clone",
                        path, e
                    );
                    isolated.push(path.clone());
                }
            }
        }
    } else {
        isolated = wanted.iter().map(|(_, p)| p.clone()).collect();
    }

    if !isolated.is_empty() {
        // Init and update only the wanted submodules by passing explicit paths.
        // This is necessary because worktrees share .git/config with the main
        // worktree, so submodules initialized there would otherwise all get cloned.
        let mut args = vec!["submodule", "update", "--init"];
        if workset.submodules.shallow {
            args.push("--depth");
            args.push("1");
        }
        args.push("--");
        let refs: Vec<&str> = isolated.iter().map(|s| s.as_str()).collect();
        args.extend(&refs);
        run_git(&args, worktree_path)?;
        outcome.cloned += isolated.len();

        // Hardening is beneficial in isolated mode too: it protects both this
        // clone and the main one from a stray `git submodule update`.
        for path in &isolated {
            if let Ok(dir) = submodule_gitdir(worktree_path, path) {
                let _ = harden_submodule_config(&dir, Some(&worktree_path.join(path)));
            }
            if let Ok(dir) = submodule_gitdir(main_repo, path) {
                let _ = harden_submodule_config(&dir, Some(&main_repo.join(path)));
            }
        }
    }

    Ok(outcome)
}

/// Duplicate (isolated) submodule gitdirs living under this worktree's own
/// gitdir — the v0.3.x layout, and what `sync` migrates away from.
fn duplicate_gitdirs(
    worktree_path: &Path,
    wanted: &[(String, String)],
) -> Result<Vec<(String, PathBuf)>> {
    let wt_gitdir = worktree_git_dir(worktree_path)?;
    let modules = wt_gitdir.join("modules");
    if !modules.exists() {
        return Ok(vec![]);
    }
    Ok(wanted
        .iter()
        .filter_map(|(name, path)| {
            let dir = modules.join(name);
            if dir.join("HEAD").exists() {
                Some((path.clone(), dir))
            } else {
                None
            }
        })
        .collect())
}

/// Attach one submodule to the shared object store, migrating away from an
/// isolated clone if this worktree still has one.
fn share_submodule(
    worktree_path: &Path,
    main_repo: &Path,
    name: &str,
    sub_path: &str,
    shallow: bool,
) -> Result<()> {
    let checkout = worktree_path.join(sub_path);

    // Already attached (e.g. a repeated `sync`): just make sure the checkout
    // keeps resolving, and leave whatever the user has checked out alone.
    if let Ok(subgit) = submodule_gitdir(main_repo, sub_path) {
        if is_shared_checkout(&subgit, &checkout) {
            harden_submodule_config(&subgit, Some(&main_repo.join(sub_path)))?;
            pin_worktree_core_worktree(&checkout)?;
            return Ok(());
        }
    }

    let wt_gitdir = worktree_git_dir(worktree_path)?;
    let duplicate = wt_gitdir.join("modules").join(name);
    if duplicate.join("HEAD").exists() {
        eprintln!("  migrating '{}' to the shared object store", sub_path);
        if checkout.exists() {
            std::fs::remove_dir_all(&checkout)
                .with_context(|| format!("Failed to remove {}", checkout.display()))?;
        }
        std::fs::remove_dir_all(&duplicate)
            .with_context(|| format!("Failed to remove {}", duplicate.display()))?;
        std::fs::create_dir_all(&checkout)?;
    }

    let pin = run_git_output(&["rev-parse", &format!("HEAD:{}", sub_path)], worktree_path)
        .with_context(|| format!("Could not resolve the pinned commit for '{}'", sub_path))?;

    attach_submodule_worktree(main_repo, name, sub_path, worktree_path, &pin, shallow)
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
    list_worktrees_in(&std::env::current_dir()?)
}

/// List all worktrees of the repo containing `cwd`.
pub fn list_worktrees_in(cwd: &Path) -> Result<Vec<(PathBuf, String)>> {
    let output = run_git_output(&["worktree", "list", "--porcelain"], cwd)?;
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

/// Remove a worktree, detaching its submodule worktrees first.
///
/// `git worktree remove` refuses outright on any worktree that contains
/// submodules, so the submodule checkouts have to be unregistered before the
/// superproject worktree goes. That in turn leaves the submodule directories
/// missing, which reads as a modification — hence the forced removal in step 2.
pub fn remove_worktree(main_repo: &Path, path: &Path, force: bool) -> Result<()> {
    let path_str = path.to_str().context("Invalid path")?;

    // Read .gitmodules while the worktree still exists.
    let entries = parse_submodule_entries(path).unwrap_or_default();

    if !force {
        let dirty = run_git_output(&["status", "--porcelain", "--ignore-submodules=all"], path)
            .unwrap_or_default();
        if !dirty.trim().is_empty() {
            bail!(
                "{} contains modified or untracked files. \
                 Commit or stash them, or re-run with --force.",
                path.display()
            );
        }
    }

    // 1. detach each submodule worktree
    for (_, sub_path) in &entries {
        let sub_checkout = path.join(sub_path);
        if sub_checkout.exists() {
            if let Err(e) = detach_submodule_worktree(main_repo, sub_path, &sub_checkout) {
                eprintln!("  note: could not detach submodule '{}': {}", sub_path, e);
            }
        }
    }

    // 2. now the superproject worktree no longer "contains submodules"
    run_git(&["worktree", "remove", "--force", path_str], main_repo)?;

    // 3. belt and braces — clear both registries
    let _ = run_git(&["worktree", "prune"], main_repo);
    for (_, sub_path) in &entries {
        if let Ok(subgit) = submodule_gitdir(main_repo, sub_path) {
            if let Some(s) = subgit.to_str() {
                let _ = try_git(&gitdir_args(s, &["worktree", "prune"]), main_repo);
            }
        }
    }

    Ok(())
}
