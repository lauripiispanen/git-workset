use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::Path;

#[derive(Debug, Serialize, Deserialize)]
pub struct WorksetsConfig {
    #[serde(default)]
    pub workset: BTreeMap<String, Workset>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Workset {
    #[serde(default)]
    pub description: Option<String>,
    /// Directories to include in sparse checkout
    pub include: Vec<String>,
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
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
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
        let config_path = repo_root.join(".git-workset.toml");
        let content = std::fs::read_to_string(&config_path)
            .with_context(|| format!("Failed to read {}", config_path.display()))?;
        toml::from_str(&content).context("Failed to parse .git-workset.toml")
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
        toml::from_str(&content).context("Failed to parse .git-workset.toml from git tree")
    }

    pub fn get_workset(&self, name: &str) -> Result<Workset> {
        // Support composite worksets with "+" separator
        let names: Vec<&str> = name.split('+').collect();

        if names.len() == 1 {
            return self.workset.get(name).cloned().with_context(|| {
                let available: Vec<&str> = self.workset.keys().map(|s| s.as_str()).collect();
                format!(
                    "Workset '{}' not found. Available: {}",
                    name,
                    available.join(", ")
                )
            });
        }

        // Merge multiple worksets
        let mut merged: Option<Workset> = None;
        for n in &names {
            let ws = self
                .workset
                .get(*n)
                .with_context(|| format!("Workset '{}' not found", n))?;

            merged = Some(match merged {
                None => ws.clone(),
                Some(mut m) => {
                    // Union includes
                    for dir in &ws.include {
                        if !m.include.contains(dir) {
                            m.include.push(dir.clone());
                        }
                    }
                    // Union LFS includes
                    for pat in &ws.include_lfs {
                        if !m.include_lfs.contains(pat) {
                            m.include_lfs.push(pat.clone());
                        }
                    }
                    // Union LFS excludes
                    for pat in &ws.exclude_lfs {
                        if !m.exclude_lfs.contains(pat) {
                            m.exclude_lfs.push(pat.clone());
                        }
                    }
                    // Union submodule skips
                    for s in &ws.submodules.skip {
                        if !m.submodules.skip.contains(s) {
                            m.submodules.skip.push(s.clone());
                        }
                    }
                    // shallow is true if either wants it
                    m.submodules.shallow = m.submodules.shallow || ws.submodules.shallow;
                    m.description = Some(format!("Composite: {}", names.join("+")));
                    m
                }
            });
        }
        merged.context("No worksets to merge")
    }

    pub fn template() -> Self {
        let mut worksets = BTreeMap::new();
        worksets.insert(
            "server".to_string(),
            Workset {
                description: Some("Backend server development".to_string()),
                include: vec!["src/server".to_string(), "src/shared".to_string()],
                exclude_lfs: vec!["*.psd".to_string(), "*.fbx".to_string()],
                include_lfs: vec!["*.json".to_string(), "*.toml".to_string()],
                submodules: SubmoduleConfig {
                    shallow: true,
                    skip: vec!["third_party/art-pipeline".to_string()],
                },
                sparse_cone: true,
            },
        );
        worksets.insert(
            "client".to_string(),
            Workset {
                description: Some("Game client work".to_string()),
                include: vec![
                    "src/client".to_string(),
                    "src/shared".to_string(),
                    "src/rendering".to_string(),
                ],
                exclude_lfs: vec![],
                include_lfs: vec!["*.png".to_string(), "*.atlas".to_string()],
                submodules: SubmoduleConfig {
                    shallow: true,
                    skip: vec![],
                },
                sparse_cone: true,
            },
        );
        WorksetsConfig { workset: worksets }
    }
}
