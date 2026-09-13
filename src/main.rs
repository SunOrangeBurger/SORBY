mod cgroups;
mod container;
mod filesystem;
mod namespaces;

use std::path::PathBuf;

use anyhow::Result;
use clap::{Parser, Subcommand};

/// contain-rs: a minimal, educational OCI-inspired container runtime.
#[derive(Parser)]
#[command(name = "sorby", version, about)]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Run a command inside an isolated container.
    Run {
        /// Path to the root filesystem to use (e.g. an extracted Alpine tarball).
        #[arg(long)]
        rootfs: PathBuf,

        /// Memory ceiling, e.g. "50M", "1G". Omit for no limit.
        #[arg(long)]
        memory: Option<String>,

        /// Fraction of a single CPU core, e.g. 0.2 for 20%. Omit for no limit.
        #[arg(long)]
        cpu: Option<f64>,

        /// Command to run inside the container, e.g. `-- /bin/sh`.
        #[arg(last = true, required = true)]
        command: Vec<String>,
    },

    /// Internal: re-executed by `run` inside new namespaces. Not meant to be
    /// invoked directly by users.
    #[command(hide = true)]
    ChildInit {
        #[arg(long)]
        rootfs: PathBuf,

        #[arg(last = true, required = true)]
        command: Vec<String>,
    },
}

fn main() -> Result<()> {
    // Respects RUST_LOG if set (e.g. `RUST_LOG=debug`), otherwise defaults
    // to showing info-level-and-above messages.
    tracing_subscriber::fmt::init();

    let cli = Cli::parse();

    match cli.command {
        Commands::Run {
            rootfs,
            memory,
            cpu,
            command,
        } => container::run(rootfs, memory, cpu, command),
        Commands::ChildInit { rootfs, command } => container::child_init(rootfs, command),
    }
}
