use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::io::Read;
use std::path::Path;

/// Newest `.git-workset.toml` schema version this binary understands.
pub const CONFIG_VERSION: u32 = 1;

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WorksetsConfig {
    /// Config schema version. Absent means 1.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version: Option<u32>,
    /// Repo-wide submodule settings. Deliberately top-level, not per-workset:
    /// object-store layout is a property of the repo, not of a profile.
    #[serde(default)]
    pub submodules: RepoSubmoduleConfig,
    #[serde(default)]
    pub workset: BTreeMap<String, Workset>,
}

/// Repo-wide `[submodules]` table.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RepoSubmoduleConfig {
    #[serde(default)]
    pub sharing: SubmoduleSharing,
}

/// How submodule object stores are laid out across worksets.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SubmoduleSharing {
    /// Each workset's submodule checkout is a worktree of the main clone's
    /// submodule gitdir — one object store per submodule, repo-wide.
    #[default]
    Shared,
    /// Each workset gets its own submodule clone.
    Isolated,
}

impl SubmoduleSharing {
    pub fn as_str(self) -> &'static str {
        match self {
            SubmoduleSharing::Shared => "shared",
            SubmoduleSharing::Isolated => "isolated",
        }
    }

    pub fn parse(s: &str) -> Result<Self> {
        match s.trim() {
            "shared" => Ok(SubmoduleSharing::Shared),
            "isolated" => Ok(SubmoduleSharing::Isolated),
            other => anyhow::bail!(
                "invalid submodule sharing mode '{}' (valid options: shared, isolated)",
                other
            ),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Workset {
    #[serde(default)]
    pub description: Option<String>,
    /// Directories to include in sparse checkout
    #[serde(default)]
    pub include: Vec<String>,
    /// Directories to exclude from sparse checkout (forces --no-cone mode)
    #[serde(default)]
    pub exclude: Vec<String>,
    /// LFS patterns to exclude from download
    #[serde(default)]
    pub exclude_lfs: Vec<String>,
    /// LFS patterns to include for download (if set, only these are fetched)
    #[serde(default)]
    pub include_lfs: Vec<String>,
    /// Submodule configuration
    #[serde(default)]
    pub submodules: SubmoduleConfig,
    /// Use cone mode for sparse checkout (faster, directory-based)
    #[serde(default = "default_true")]
    pub sparse_cone: bool,
    /// Paths re-included after `exclude` when composing profiles: one
    /// profile's exclude must not remove what another explicitly includes.
    #[serde(skip)]
    pub reinclude: Vec<String>,
}

impl Workset {
    /// No include and no exclude: sparse checkout is disabled.
    pub fn is_full_tree(&self) -> bool {
        self.include.is_empty() && self.exclude.is_empty()
    }

    /// Does this profile check out everything under `path`?
    fn covers(&self, path: &str) -> bool {
        let included = self.include.is_empty() || self.include.iter().any(|i| is_within(path, i));
        included && !self.exclude.iter().any(|e| is_within(path, e))
    }
}

/// `path` is `base` or lies below it.
fn is_within(path: &str, base: &str) -> bool {
    let path = path.trim_matches('/');
    let base = base.trim_matches('/');
    path == base || path.starts_with(&format!("{}/", base))
}

fn push_unique(list: &mut Vec<String>, item: &str) {
    if !list.iter().any(|x| x == item) {
        list.push(item.to_string());
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SubmoduleConfig {
    /// Clone submodules with --depth 1
    #[serde(default = "default_true")]
    pub shallow: bool,
    /// Submodule paths to skip entirely
    #[serde(default)]
    pub skip: Vec<String>,
}

fn default_true() -> bool {
    true
}

impl WorksetsConfig {
    pub fn load(repo_root: &Path) -> Result<Self> {
        Self::load_from_path(&repo_root.join(".git-workset.toml"))
    }

    /// Load from a file, or from stdin when `path` is `-`.
    pub fn load_from_path(path: &Path) -> Result<Self> {
        if path.as_os_str() == "-" {
            let mut content = String::new();
            std::io::stdin()
                .read_to_string(&mut content)
                .context("Failed to read config from stdin")?;
            return Self::parse(&content, "<stdin>");
        }
        let content = std::fs::read_to_string(path)
            .with_context(|| format!("Failed to read {}", path.display()))?;
        Self::parse(&content, &path.display().to_string())
    }

    pub fn parse(content: &str, source: &str) -> Result<Self> {
        let config: Self =
            toml::from_str(content).with_context(|| format!("Failed to parse {}", source))?;
        let version = config.version.unwrap_or(1);
        if version == 0 || version > CONFIG_VERSION {
            bail!(
                "{}: unsupported config version {} (this git-workset supports version 1 to {})",
                source,
                version,
                CONFIG_VERSION
            );
        }
        Ok(config)
    }

    /// Load config directly from the git tree without checking the file out.
    /// Uses `git show <rev>:.git-workset.toml`.
    pub fn load_from_git(repo_path: &Path, rev: &str) -> Result<Self> {
        let spec = format!("{}:.git-workset.toml", rev);
        let output = std::process::Command::new("git")
            .args(["show", &spec])
            .current_dir(repo_path)
            .output()
            .context("Failed to run git show")?;
        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            anyhow::bail!(
                "No .git-workset.toml found at '{}' in remote. Is the config committed?\n{}",
                rev,
                stderr.trim()
            );
        }
        let content =
            String::from_utf8(output.stdout).context("Invalid UTF-8 in .git-workset.toml")?;
        Self::parse(&content, &format!("{}:.git-workset.toml", rev))
    }

    fn not_found(&self, name: &str) -> anyhow::Error {
        let available: Vec<&str> = self.workset.keys().map(|s| s.as_str()).collect();
        if available.is_empty() {
            anyhow::anyhow!(
                "Workset '{}' not found: the config defines no worksets",
                name
            )
        } else {
            anyhow::anyhow!(
                "Workset '{}' not found. Available: {}",
                name,
                available.join(", ")
            )
        }
    }

    /// Resolve a profile name, composing `a+b` as the union of the parts.
    pub fn get_workset(&self, name: &str) -> Result<Workset> {
        let names: Vec<&str> = name.split('+').collect();
        let mut parts: Vec<&Workset> = Vec::new();
        for n in &names {
            match self.workset.get(*n) {
                Some(ws) if !n.is_empty() => parts.push(ws),
                _ => return Err(self.not_found(n)),
            }
        }

        if let [single] = parts.as_slice() {
            return Ok((*single).clone());
        }
        Ok(compose(&parts, name))
    }

    pub fn template() -> Self {
        let mut worksets = BTreeMap::new();
        worksets.insert(
            "all".to_string(),
            Workset {
                description: Some("Everything".to_string()),
                include: vec![],
                exclude: vec![],
                exclude_lfs: vec![],
                include_lfs: vec![],
                submodules: SubmoduleConfig {
                    shallow: true,
                    skip: vec![],
                },
                sparse_cone: true,
                reinclude: vec![],
            },
        );
        WorksetsConfig {
            version: None,
            submodules: RepoSubmoduleConfig::default(),
            workset: worksets,
        }
    }
}

/// Compose profiles so the result checks out the union of what each part
/// checks out:
/// - any full-tree part makes the result full-tree;
/// - includes are unioned, and a part with only excludes includes everything;
/// - an exclude is dropped when another part covers that path entirely, and
///   paths another part includes below it are re-included;
/// - cone mode only if every part allows it.
fn compose(parts: &[&Workset], name: &str) -> Workset {
    let mut merged = Workset {
        description: Some(format!("Composite: {}", name)),
        include: vec![],
        exclude: vec![],
        exclude_lfs: vec![],
        include_lfs: vec![],
        submodules: SubmoduleConfig {
            shallow: false,
            skip: vec![],
        },
        sparse_cone: parts.iter().all(|p| p.sparse_cone),
        reinclude: vec![],
    };

    for p in parts {
        for pat in &p.include_lfs {
            push_unique(&mut merged.include_lfs, pat);
        }
        for pat in &p.exclude_lfs {
            push_unique(&mut merged.exclude_lfs, pat);
        }
        for s in &p.submodules.skip {
            push_unique(&mut merged.submodules.skip, s);
        }
        // shallow is true if any part wants it
        merged.submodules.shallow |= p.submodules.shallow;
    }

    if parts.iter().any(|p| p.is_full_tree()) {
        return merged;
    }

    let includes_everything = parts.iter().any(|p| p.include.is_empty());
    if !includes_everything {
        for p in parts {
            for dir in &p.include {
                push_unique(&mut merged.include, dir);
            }
        }
    }

    for (i, p) in parts.iter().enumerate() {
        let others = || {
            parts
                .iter()
                .enumerate()
                .filter(move |(j, _)| *j != i)
                .map(|(_, o)| *o)
        };
        for ex in &p.exclude {
            if others().any(|o| o.covers(ex)) {
                continue;
            }
            push_unique(&mut merged.exclude, ex);
            for o in others() {
                for inc in &o.include {
                    if is_within(inc, ex) && o.covers(inc) {
                        push_unique(&mut merged.reinclude, inc);
                    }
                }
            }
        }
    }

    merged
}
