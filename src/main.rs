mod config;
mod git;

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use std::path::PathBuf;

use config::WorksetsConfig;

#[derive(Parser)]
#[command(name = "git-workset", version, about = "Named sparse-checkout profiles for git worktrees")]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Create a .git-workset.toml template in the current repo
    Init,

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
}

fn main() -> Result<()> {
    let cli = Cli::parse();

    match cli.command {
        Commands::Init => cmd_init(),
        Commands::Carve { path, branch, workset } => cmd_carve(&path, &branch, &workset),
        Commands::Sync => cmd_sync(),
        Commands::List => cmd_list(),
        Commands::Switch { name } => cmd_switch(&name),
        Commands::Remove { path } => cmd_remove(&path),
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

fn cmd_carve(path: &PathBuf, branch: &str, workset_name: &str) -> Result<()> {
    let repo_root = git::find_repo_root()?;
    let config = WorksetsConfig::load(&repo_root)?;
    let workset = config.get_workset(workset_name)?;

    let abs_path = if path.is_absolute() {
        path.clone()
    } else {
        std::env::current_dir()?.join(path)
    };

    eprintln!("Creating worktree at {} on branch '{}'", abs_path.display(), branch);
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

        println!(
            "{:<50} {:<25} [{}]",
            path.display(),
            branch,
            workset,
        );
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
