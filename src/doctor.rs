//! `git workset doctor` — detect (and optionally repair) the ways a repo's
//! submodule plumbing can get into a bad state, including the damage left
//! behind by earlier versions of git-workset.

use anyhow::{Context, Result};
use std::path::{Path, PathBuf};

use crate::config::SubmoduleSharing;
use crate::git;

pub struct Issue {
    pub code: &'static str,
    pub message: String,
    pub fixed: bool,
}

impl Issue {
    fn new(code: &'static str, message: String) -> Self {
        Issue {
            code,
            message,
            fixed: false,
        }
    }
}

/// One registered worktree of a submodule gitdir.
struct SubWorktree {
    /// The checkout directory.
    path: PathBuf,
    /// The gitdir for *this* worktree (the shared dir for the main checkout,
    /// `<subgit>/worktrees/<id>` for a linked one).
    gitdir: PathBuf,
    linked: bool,
}

impl SubWorktree {
    fn config_worktree_file(&self) -> PathBuf {
        self.gitdir.join("config.worktree")
    }
}

fn submodule_worktrees(subgit: &Path, main_checkout: &Path) -> Vec<SubWorktree> {
    let mut out = vec![SubWorktree {
        path: main_checkout.to_path_buf(),
        gitdir: subgit.to_path_buf(),
        linked: false,
    }];

    let worktrees = subgit.join("worktrees");
    let Ok(entries) = std::fs::read_dir(&worktrees) else {
        return out;
    };
    for entry in entries.flatten() {
        let dir = entry.path();
        let Ok(target) = std::fs::read_to_string(dir.join("gitdir")) else {
            continue;
        };
        // The file points at "<checkout>/.git"
        let dot_git = PathBuf::from(target.trim());
        let Some(checkout) = dot_git.parent() else {
            continue;
        };
        out.push(SubWorktree {
            path: checkout.to_path_buf(),
            gitdir: dir,
            linked: true,
        });
    }
    out
}

/// The `core.worktree` value git will actually use for a checkout, honouring
/// `extensions.worktreeConfig`.
fn effective_core_worktree(subgit: &Path, wt: &SubWorktree) -> Option<String> {
    let shared = subgit.join("config");
    let per_worktree_enabled = git::config_file_get(&shared, "extensions.worktreeConfig")
        .map(|v| v == "true")
        .unwrap_or(false);

    let own = if per_worktree_enabled {
        git::config_file_get(&wt.config_worktree_file(), "core.worktree")
    } else {
        None
    };
    own.or_else(|| git::config_file_get(&shared, "core.worktree"))
}

fn resolves_to(value: &str, base: &Path, expected: &Path) -> bool {
    let candidate = if Path::new(value).is_absolute() {
        PathBuf::from(value)
    } else {
        base.join(value)
    };
    match (
        std::fs::canonicalize(&candidate),
        std::fs::canonicalize(expected),
    ) {
        (Ok(a), Ok(b)) => a == b,
        _ => false,
    }
}

fn prunable_worktrees(args_gitdir: Option<&str>, cwd: &Path) -> Vec<String> {
    let base = ["worktree", "list", "--porcelain"];
    let args: Vec<&str> = match args_gitdir {
        Some(dir) => git::gitdir_args(dir, &base),
        None => base.to_vec(),
    };
    let Ok(out) = git::run_git_output(&args, cwd) else {
        return vec![];
    };

    let mut prunable = Vec::new();
    let mut current: Option<String> = None;
    for line in out.lines() {
        if let Some(path) = line.strip_prefix("worktree ") {
            current = Some(path.to_string());
        } else if line.starts_with("prunable") {
            if let Some(p) = current.clone() {
                prunable.push(p);
            }
        }
    }
    prunable
}

fn dir_size(path: &Path) -> u64 {
    let mut total = 0;
    let Ok(entries) = std::fs::read_dir(path) else {
        return 0;
    };
    for entry in entries.flatten() {
        let Ok(meta) = entry.metadata() else { continue };
        if meta.is_dir() {
            total += dir_size(&entry.path());
        } else {
            total += meta.len();
        }
    }
    total
}

fn human_bytes(n: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KiB", "MiB", "GiB", "TiB"];
    let mut value = n as f64;
    let mut unit = 0;
    while value >= 1024.0 && unit < UNITS.len() - 1 {
        value /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{} {}", n, UNITS[0])
    } else {
        format!("{:.1} {}", value, UNITS[unit])
    }
}

/// Run all checks. Returns every issue found; those repaired when `fix` is set
/// are marked `fixed`.
pub fn run(main_repo: &Path, sharing: SubmoduleSharing, fix: bool) -> Result<Vec<Issue>> {
    let mut issues: Vec<Issue> = Vec::new();

    // D6 — version gate
    if sharing == SubmoduleSharing::Shared {
        if let Ok((major, minor)) = git::git_version() {
            if (major, minor) < (2, 20) {
                issues.push(Issue::new(
                    "D6",
                    format!(
                        "git {}.{} predates extensions.worktreeConfig (2.20). Shared submodule \
                         stores are unavailable — set submodules.sharing = \"isolated\".",
                        major, minor
                    ),
                ));
            }
        }
    }

    let entries = git::parse_submodule_entries(main_repo).unwrap_or_default();
    let common = git::main_git_dir(main_repo)?;

    // Superproject registry
    let superproject_prunable = prunable_worktrees(None, main_repo);
    if !superproject_prunable.is_empty() {
        let mut issue = Issue::new(
            "D3",
            format!(
                "{} orphaned superproject worktree registration(s): {}",
                superproject_prunable.len(),
                superproject_prunable.join(", ")
            ),
        );
        if fix {
            git::run_git(&["worktree", "prune"], main_repo)?;
            issue.fixed = true;
        }
        issues.push(issue);
    }

    let mut repair_needed = false;

    for (name, sub_path) in &entries {
        let subgit = common.join("modules").join(name);
        if !subgit.join("config").exists() {
            continue;
        }
        let subgit_str = subgit.to_str().context("Invalid gitdir path")?;
        let main_checkout = main_repo.join(sub_path);
        let worktrees = submodule_worktrees(&subgit, &main_checkout);

        // D1 — unhardened gitdir shared by more than one checkout
        let shared_cfg = subgit.join("config");
        let shared_core_worktree = git::config_file_get(&shared_cfg, "core.worktree");
        if shared_core_worktree.is_some() && worktrees.len() > 1 {
            let mut issue = Issue::new(
                "D1",
                format!(
                    "submodule '{}' has core.worktree in its shared config while {} checkouts \
                     use it — any `git submodule update` will redirect the others",
                    sub_path,
                    worktrees.len()
                ),
            );
            if fix {
                git::harden_submodule_config(&subgit, Some(&main_checkout))?;
                issue.fixed = true;
            }
            issues.push(issue);
        }

        // D2 — clobbered core.worktree
        for wt in &worktrees {
            if !wt.path.exists() {
                continue;
            }
            let Some(value) = effective_core_worktree(&subgit, wt) else {
                continue;
            };
            if resolves_to(&value, &wt.gitdir, &wt.path) {
                continue;
            }
            let mut issue = Issue::new(
                "D2",
                format!(
                    "core.worktree for {} resolves to '{}' instead of the checkout itself",
                    wt.path.display(),
                    value
                ),
            );
            if fix {
                git::config_file_set(&shared_cfg, "extensions.worktreeConfig", "true")?;
                let abs = git::abs_path(&wt.path);
                git::config_file_set(
                    &wt.config_worktree_file(),
                    "core.worktree",
                    abs.to_str().context("Invalid worktree path")?,
                )?;
                // Drop the offending shared value so other checkouts that have
                // no per-worktree override stop inheriting it.
                if !wt.linked && git::config_file_get(&shared_cfg, "core.worktree").is_some() {
                    let _ = git::config_file_unset(&shared_cfg, "core.worktree");
                }
                issue.fixed = true;
            }
            issues.push(issue);
        }

        // D3 — orphaned submodule worktree registrations
        let prunable = prunable_worktrees(Some(subgit_str), main_repo);
        if !prunable.is_empty() {
            let mut issue = Issue::new(
                "D3",
                format!(
                    "submodule '{}' has {} orphaned worktree registration(s): {}",
                    sub_path,
                    prunable.len(),
                    prunable.join(", ")
                ),
            );
            if fix {
                git::try_git(
                    &git::gitdir_args(subgit_str, &["worktree", "prune"]),
                    main_repo,
                )?;
                issue.fixed = true;
            }
            issues.push(issue);
        }

        // D4 — a checkout that exists but no longer resolves
        for wt in &worktrees {
            if !wt.path.exists() || !wt.path.join(".git").exists() {
                continue;
            }
            if git::run_git_output(&["rev-parse", "--show-toplevel"], &wt.path).is_err() {
                repair_needed = true;
                let mut issue = Issue::new(
                    "D4",
                    format!(
                        "submodule checkout {} does not resolve — its gitdir link is broken",
                        wt.path.display()
                    ),
                );
                if fix {
                    let _ = git::run_git(&["worktree", "repair"], main_repo);
                    let _ = git::try_git(
                        &git::gitdir_args(subgit_str, &["worktree", "repair"]),
                        main_repo,
                    );
                    issue.fixed = true;
                }
                issues.push(issue);
            }
        }
    }

    // D4 — superproject worktrees whose links are broken
    if !repair_needed {
        for (path, _) in crate::git::list_worktrees_in(main_repo).unwrap_or_default() {
            if path.exists()
                && git::run_git_output(&["rev-parse", "--show-toplevel"], &path).is_err()
            {
                let mut issue = Issue::new(
                    "D4",
                    format!(
                        "worktree {} does not resolve — run `git worktree repair`",
                        path.display()
                    ),
                );
                if fix {
                    let _ = git::run_git(&["worktree", "repair"], main_repo);
                    issue.fixed = true;
                }
                issues.push(issue);
            }
        }
    }

    // D5 — reclaimable duplicate object stores
    if sharing == SubmoduleSharing::Shared {
        let mut reclaimable = 0u64;
        let mut dupes: Vec<String> = Vec::new();
        if let Ok(worktrees) = std::fs::read_dir(common.join("worktrees")) {
            for wt in worktrees.flatten() {
                let modules = wt.path().join("modules");
                if modules.exists() {
                    reclaimable += dir_size(&modules);
                    dupes.push(format!("{}", modules.display()));
                }
            }
        }
        if !dupes.is_empty() {
            issues.push(Issue::new(
                "D5",
                format!(
                    "{} duplicate submodule object store(s) using {}. --fix cannot reclaim \
                     these; run `git workset sync --shared-submodules` in each worktree to \
                     migrate them:\n    {}",
                    dupes.len(),
                    human_bytes(reclaimable),
                    dupes.join("\n    ")
                ),
            ));
        }
    }

    Ok(issues)
}
