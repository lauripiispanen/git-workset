mod config;
mod git;

use anyhow::{bail, Context, Result};
use clap::{Parser, Subcommand};
use std::path::PathBuf;
use std::process::Command;

use config::WorksetsConfig;

#[derive(Parser)]
#[command(
    name = "git-workset",
    version,
    about = "Named sparse-checkout profiles for git worktrees"
)]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Create a .git-workset.toml template in the current repo
    Init,

    /// Clone a repo with only a workset's files (no full checkout)
    Clone {
        /// Repository URL
        url: String,
        /// Directory to clone into
        path: PathBuf,
        /// Workset profile name (use "a+b" to compose multiple)
        #[arg(short, long)]
        workset: String,
        /// Branch to clone
        #[arg(short, long)]
        branch: Option<String>,
        /// Clone with limited history depth
        #[arg(long)]
        depth: Option<u32>,
        /// Shorthand for --depth 1
        #[arg(long, conflicts_with = "depth")]
        shallow: bool,
    },

    /// Create a new worktree with a workset profile applied
    Carve {
        /// Path for the new worktree
        path: PathBuf,
        /// Branch to check out
        branch: String,
        /// Workset profile name (use "a+b" to compose multiple)
        #[arg(short, long)]
        workset: String,
    },

    /// Re-apply the active workset profile to the current worktree
    Sync,

    /// List all worktrees and their active workset profiles
    List,

    /// Switch the workset profile on the current worktree
    Switch {
        /// Workset profile name to switch to
        name: String,
    },

    /// Remove a worktree
    Remove {
        /// Path of the worktree to remove
        path: PathBuf,
    },

    /// Fetch more history for a shallow clone
    Deepen {
        /// Number of additional commits to fetch (omit for full history)
        #[arg(long)]
        by: Option<u32>,
    },
}

fn main() -> Result<()> {
    let cli = Cli::parse();

    match cli.command {
        Commands::Init => cmd_init(),
        Commands::Clone {
            url,
            path,
            workset,
            branch,
            depth,
            shallow,
        } => {
            let effective_depth = if shallow { Some(1) } else { depth };
            cmd_clone(&url, &path, &workset, branch.as_deref(), effective_depth)
        }
        Commands::Carve {
            path,
            branch,
            workset,
        } => cmd_carve(&path, &branch, &workset),
        Commands::Sync => cmd_sync(),
        Commands::List => cmd_list(),
        Commands::Switch { name } => cmd_switch(&name),
        Commands::Remove { path } => cmd_remove(&path),
        Commands::Deepen { by } => cmd_deepen(by),
    }
}

fn cmd_init() -> Result<()> {
    let repo_root = git::find_repo_root()?;
    let config_path = repo_root.join(".git-workset.toml");

    if config_path.exists() {
        anyhow::bail!("{} already exists", config_path.display());
    }

    let template = WorksetsConfig::template();
    let content = toml::to_string_pretty(&template).context("Failed to serialize template")?;
    std::fs::write(&config_path, &content)
        .with_context(|| format!("Failed to write {}", config_path.display()))?;

    eprintln!("Created {}", config_path.display());
    eprintln!("Edit it to define your workset profiles, then use `git workset carve` to create worktrees.");
    Ok(())
}

fn cmd_clone(
    url: &str,
    path: &PathBuf,
    workset_name: &str,
    branch: Option<&str>,
    depth: Option<u32>,
) -> Result<()> {
    let abs_path = if path.is_absolute() {
        path.clone()
    } else {
        std::env::current_dir()?.join(path)
    };

    eprintln!("Cloning {} into {}", url, abs_path.display());

    // 1. Peek at the config from the remote before full clone.
    //    We need to know the workset to set up sparse checkout before checkout.
    eprintln!("Fetching workset config...");
    let config = {
        let tmp = abs_path.with_file_name(format!(
            ".{}-config-probe",
            abs_path.file_name().unwrap_or_default().to_string_lossy()
        ));
        // Minimal fetch: just enough to read the config file
        let mut probe_args = vec![
            "clone",
            "--depth",
            "1",
            "--no-checkout",
            "--filter=blob:none",
        ];
        if let Some(b) = branch {
            probe_args.push("--branch");
            probe_args.push(b);
        }
        probe_args.push(url);
        probe_args.push(tmp.to_str().unwrap());
        let probe_status = Command::new("git")
            .args(&probe_args)
            .env("GIT_LFS_SKIP_SMUDGE", "1")
            .status()
            .context("Failed to probe remote")?;
        if !probe_status.success() {
            bail!("Failed to probe remote for config");
        }
        let rev = branch.unwrap_or("HEAD");
        let result = WorksetsConfig::load_from_git(&tmp, rev);
        let _ = std::fs::remove_dir_all(&tmp);
        result?
    };
    let workset = config.get_workset(workset_name)?;

    if let Some(desc) = &workset.description {
        eprintln!("Workset: {} ({})", workset_name, desc);
    }

    // 2. Sparse clone: init → sparse checkout → fetch → checkout
    //    Configuring sparse checkout before the first checkout avoids
    //    processing 91K+ files through smudge filters.
    git::sparse_clone(url, &abs_path, branch, depth, &workset)?;

    // 3. Enable worktree-scoped config
    git::enable_worktree_config(&abs_path)?;

    // 4. Initialize submodules
    eprintln!("\nInitializing submodules...");
    git::init_submodules(&abs_path, &workset)?;

    // 5. Configure and pull LFS
    eprintln!("\nConfiguring LFS...");
    git::configure_lfs(&abs_path, &workset)?;

    // 6. Store workset marker
    git::store_workset_name(&abs_path, workset_name)?;

    eprintln!("\nDone! Sparse clone ready at {}", abs_path.display());
    Ok(())
}

fn cmd_carve(path: &PathBuf, branch: &str, workset_name: &str) -> Result<()> {
    let repo_root = git::find_repo_root()?;
    let config = WorksetsConfig::load_from_git(&repo_root, branch)?;
    let workset = config.get_workset(workset_name)?;

    let abs_path = if path.is_absolute() {
        path.clone()
    } else {
        std::env::current_dir()?.join(path)
    };

    eprintln!(
        "Creating worktree at {} on branch '{}'",
        abs_path.display(),
        branch
    );
    if let Some(desc) = &workset.description {
        eprintln!("Workset: {} ({})", workset_name, desc);
    }

    // 1. Create worktree with LFS smudge skipped
    git::add_worktree(&abs_path, branch)?;

    // 2. Enable worktree-scoped config so LFS and submodule settings
    //    don't leak into the main repo's .git/config
    git::enable_worktree_config(&abs_path)?;

    // 3. Apply sparse checkout
    eprintln!("\nConfiguring sparse checkout...");
    git::apply_sparse_checkout(&abs_path, &workset)?;

    // 4. Initialize submodules
    eprintln!("\nInitializing submodules...");
    git::init_submodules(&abs_path, &workset)?;

    // 5. Configure and pull LFS
    eprintln!("\nConfiguring LFS...");
    git::configure_lfs(&abs_path, &workset)?;

    // 5. Store workset marker
    git::store_workset_name(&abs_path, workset_name)?;

    eprintln!("\nDone! Worktree ready at {}", abs_path.display());
    Ok(())
}

fn cmd_sync() -> Result<()> {
    let cwd = std::env::current_dir()?;
    let repo_root = git::find_repo_root()?;

    let workset_name = git::read_workset_name(&cwd)?
        .context("No workset is active in this worktree. Use `worksets switch <name>` first.")?;

    let config = WorksetsConfig::load(&repo_root)?;
    let workset = config.get_workset(&workset_name)?;

    eprintln!("Syncing workset '{}' in {}", workset_name, cwd.display());

    git::enable_worktree_config(&cwd)?;
    git::apply_sparse_checkout(&cwd, &workset)?;
    git::init_submodules(&cwd, &workset)?;
    git::configure_lfs(&cwd, &workset)?;

    eprintln!("Done!");
    Ok(())
}

fn cmd_list() -> Result<()> {
    let worktrees = git::list_worktrees()?;

    if worktrees.is_empty() {
        eprintln!("No worktrees found.");
        return Ok(());
    }

    for (path, branch) in &worktrees {
        let workset = git::read_workset_name(path)
            .ok()
            .flatten()
            .unwrap_or_else(|| "-".to_string());

        println!("{:<50} {:<25} [{}]", path.display(), branch, workset,);
    }

    Ok(())
}

fn cmd_switch(name: &str) -> Result<()> {
    let cwd = std::env::current_dir()?;
    let repo_root = git::find_repo_root()?;
    let config = WorksetsConfig::load(&repo_root)?;
    let workset = config.get_workset(name)?;

    eprintln!("Switching to workset '{}' in {}", name, cwd.display());

    git::enable_worktree_config(&cwd)?;
    // Re-apply sparse checkout with new profile
    git::apply_sparse_checkout(&cwd, &workset)?;
    git::init_submodules(&cwd, &workset)?;
    git::configure_lfs(&cwd, &workset)?;

    git::store_workset_name(&cwd, name)?;

    eprintln!("Done! Switched to workset '{}'", name);
    Ok(())
}

fn cmd_remove(path: &PathBuf) -> Result<()> {
    let abs_path = if path.is_absolute() {
        path.clone()
    } else {
        std::env::current_dir()?.join(path)
    };

    eprintln!("Removing worktree at {}", abs_path.display());
    git::remove_worktree(&abs_path)?;
    eprintln!("Done!");
    Ok(())
}

fn cmd_deepen(by: Option<u32>) -> Result<()> {
    let cwd = std::env::current_dir()?;
    match by {
        Some(n) => eprintln!("Fetching {} more commits of history...", n),
        None => eprintln!("Fetching full history (unshallowing)..."),
    }
    git::deepen(&cwd, by)?;
    eprintln!("Done!");
    Ok(())
}
