//! `apply` and `profiles`: commands for tools that drive git-workset from a
//! checkout they own (CI, container builds, agent sandboxes). They never
//! fetch, clone or create branches, and with `--json` print exactly one JSON
//! document on stdout.

use anyhow::{Context, Result};
use clap::ValueEnum;
use serde_json::{json, Value};
use std::path::{Path, PathBuf};

use crate::config::{Workset, WorksetsConfig};
use crate::git;

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum SubmoduleMode {
    /// Touch nothing; report each submodule's state
    Report,
    /// Touch nothing, report nothing
    Ignore,
    /// Clone wanted submodules and mark skipped ones inactive, as `switch` does
    Manage,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum LfsMode {
    /// Write nothing; report the resolved patterns
    Report,
    /// Write `lfs.fetchinclude`/`lfs.fetchexclude`, do not pull
    Configure,
    /// Configure and run `git lfs pull`, as `switch` does
    Pull,
    /// Neither write nor report anything
    Ignore,
}

/// Where the config comes from, for commands that may run outside a checkout.
pub struct ConfigSource {
    pub file: Option<PathBuf>,
    pub rev: Option<String>,
}

impl ConfigSource {
    /// `-f <file>` (`-` = stdin), else `.git-workset.toml` at `--rev`, else at
    /// the target worktree's `HEAD`. Never the main worktree's file and never
    /// the working-tree copy, which may not be checked out yet.
    fn load(&self, target: Option<&Path>) -> Result<(WorksetsConfig, Value)> {
        if let Some(file) = &self.file {
            let config = WorksetsConfig::load_from_path(file)?;
            let path = if file.as_os_str() == "-" {
                "-".to_string()
            } else {
                file.display().to_string()
            };
            return Ok((config, json!({ "kind": "file", "path": path })));
        }
        let target = target.context("--rev or HEAD needs a repository; pass -f <file>")?;
        git::show_toplevel(target).with_context(|| {
            format!(
                "{} is not inside a git checkout; pass -f <file> to read a config without one",
                target.display()
            )
        })?;
        let rev = self.rev.as_deref().unwrap_or("HEAD");
        let config = WorksetsConfig::load_from_git(target, rev)?;
        let sha = git::run_git_output(&["rev-parse", "--verify", rev], target)
            .unwrap_or_else(|_| rev.to_string());
        Ok((
            config,
            json!({ "kind": "rev", "rev": sha, "path": ".git-workset.toml" }),
        ))
    }
}

/// Emit a result: one JSON document on stdout, or a human summary on stderr.
fn emit(json: bool, doc: &Value, human: impl FnOnce()) {
    if json {
        println!("{}", doc);
    } else {
        human();
    }
}

fn profile_json(name: &str, ws: &Workset) -> Value {
    json!({
        "name": name,
        "description": ws.description,
        "include": ws.include,
        "exclude": ws.exclude,
        "sparse_cone": ws.sparse_cone,
        "include_lfs": ws.include_lfs,
        "exclude_lfs": ws.exclude_lfs,
        "submodules": { "skip": ws.submodules.skip, "shallow": ws.submodules.shallow },
    })
}

pub fn cmd_profiles(dir: Option<&Path>, source: &ConfigSource, json: bool) -> Result<()> {
    let target = match dir {
        Some(d) => Some(d.to_path_buf()),
        None if source.file.is_some() => None,
        None => Some(std::env::current_dir()?),
    };
    let (config, _) = source.load(target.as_deref())?;

    let profiles: Vec<Value> = config
        .workset
        .iter()
        .map(|(name, ws)| profile_json(name, ws))
        .collect();
    let doc = json!({
        "schema": "git-workset/profiles@1",
        "config_version": config.version.unwrap_or(1),
        "profiles": profiles,
    });
    emit(json, &doc, || {
        for (name, ws) in &config.workset {
            match &ws.description {
                Some(d) => eprintln!("{:<20} {}", name, d),
                None => eprintln!("{}", name),
            }
        }
    });
    Ok(())
}

pub struct ApplyOptions {
    pub submodules: SubmoduleMode,
    pub lfs: LfsMode,
    pub no_marker: bool,
}

pub fn cmd_apply(
    name: &str,
    dir: Option<&Path>,
    source: &ConfigSource,
    opts: &ApplyOptions,
    sharing_flag: Option<crate::config::SubmoduleSharing>,
    json: bool,
) -> Result<()> {
    let cwd = match dir {
        Some(d) => d.to_path_buf(),
        None => std::env::current_dir()?,
    };
    let target = git::show_toplevel(&cwd).with_context(|| {
        format!(
            "apply needs a git checkout, and {} is not inside one",
            cwd.display()
        )
    })?;
    let (config, config_source) = source.load(Some(&target))?;
    let workset = config.get_workset(name)?;
    let profiles: Vec<&str> = name.split('+').collect();

    eprintln!("Applying workset '{}' to {}", name, target.display());

    let sparse = git::build_sparse_args(&workset);
    git::apply_sparse_checkout(&target, &workset)?;

    // Submodules are read from the revision the config came from, or HEAD:
    // `.gitmodules` may not be checked out yet.
    let rev = source.rev.as_deref().unwrap_or("HEAD");
    let submodules = match opts.submodules {
        SubmoduleMode::Ignore => None,
        SubmoduleMode::Report => Some(submodule_report(&target, rev, &workset, sparse.as_ref())?),
        SubmoduleMode::Manage => {
            let repo_root = git::find_repo_root_from(&target)?;
            git::enable_worktree_config(&target)?;
            let sharing = crate::resolve_sharing(
                &repo_root,
                &config,
                sharing_flag,
                crate::persisted_sharing(&target),
            )?;
            let report = submodule_report(&target, rev, &workset, sparse.as_ref())?;
            git::init_submodules(&target, &repo_root, &workset, sharing)?;
            crate::record_sharing(&target, sharing)?;
            Some(report)
        }
    };

    match opts.lfs {
        LfsMode::Pull => git::configure_lfs(&target, &workset)?,
        LfsMode::Configure => git::configure_lfs_filters(&target, &workset)?,
        LfsMode::Report | LfsMode::Ignore => {}
    }

    if !opts.no_marker {
        git::store_workset_name(&target, name)?;
    }

    let (cone, patterns) = match &sparse {
        Some((cone, patterns)) => (*cone, patterns.clone()),
        None => (false, vec![]),
    };
    let mut doc = json!({
        "schema": "git-workset/apply@1",
        "profile": name,
        "profiles": profiles,
        "config_source": config_source,
        "sparse": { "enabled": sparse.is_some(), "cone": cone, "patterns": patterns },
    });
    if let Some(subs) = &submodules {
        doc["submodules"] = Value::Array(subs.clone());
    }
    if opts.lfs != LfsMode::Ignore {
        doc["lfs"] = json!({ "include": workset.include_lfs, "exclude": workset.exclude_lfs });
    }

    emit(json, &doc, || {
        if let Some(subs) = &submodules {
            for s in subs {
                eprintln!(
                    "  submodule {}: {}",
                    s["path"].as_str().unwrap_or_default(),
                    s["state"].as_str().unwrap_or_default()
                );
            }
        }
        eprintln!("Done! Applied workset '{}'", name);
    });
    Ok(())
}

/// Each `.gitmodules` entry as `skipped` (by `submodules.skip`, which wins),
/// `in_cone` or `out_of_cone`. Top-level submodules only.
fn submodule_report(
    target: &Path,
    rev: &str,
    workset: &Workset,
    sparse: Option<&(bool, Vec<String>)>,
) -> Result<Vec<Value>> {
    let entries = git::parse_submodule_entries_at(target, rev)?;
    let paths: Vec<String> = entries.iter().map(|(_, p)| p.clone()).collect();
    let in_cone = git::paths_in_sparse(target, sparse, &paths)?;
    Ok(entries
        .iter()
        .zip(in_cone)
        .map(|((name, path), inside)| {
            let state = if workset.submodules.skip.iter().any(|s| s == path) {
                "skipped"
            } else if inside {
                "in_cone"
            } else {
                "out_of_cone"
            };
            json!({ "name": name, "path": path, "state": state })
        })
        .collect())
}
