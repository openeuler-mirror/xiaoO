//! CLI command handlers.

use clap::Subcommand;

/// Available CLI commands.
#[derive(Debug, Subcommand)]
pub enum Commands {
    /// Execute a command with policy enforcement.
    Exec {
        /// Command to execute.
        #[arg(trailing_var_arg = true)]
        args: Vec<String>,
    },

    /// Show execution history.
    History {
        /// Limit number of results.
        #[arg(short, long, default_value = "20")]
        limit: usize,

        /// Show results since (e.g., "7d", "24h").
        #[arg(short, long)]
        since: Option<String>,
    },

    /// Manage execution profiles.
    Profile {
        #[command(subcommand)]
        /// Profile subcommand to execute.
        command: ProfileCommands,
    },

    /// Install Cerberus host scaffolding for AI agent hosts.
    Init {
        /// Install Claude Code host scaffolding.
        #[arg(long, conflicts_with_all = ["codex", "opencode"])]
        claude: bool,

        /// Install Codex CLI host scaffolding.
        #[arg(long, conflicts_with_all = ["claude", "opencode"])]
        codex: bool,

        /// Install OpenCode host scaffolding.
        #[arg(long, conflicts_with_all = ["claude", "codex"])]
        opencode: bool,

        /// Show current host scaffolding status.
        #[arg(long)]
        show: bool,

        /// Uninstall host scaffolding.
        #[arg(long)]
        uninstall: bool,

        /// Force overwrite existing files.
        #[arg(long)]
        force: bool,
    },
}

/// Profile management commands.
#[derive(Debug, Subcommand)]
pub enum ProfileCommands {
    /// List available profiles.
    List,

    /// Show profile details.
    Show {
        /// Profile name.
        name: String,
    },
}
