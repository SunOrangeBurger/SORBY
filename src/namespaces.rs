//! Namespace isolation via the "self-exec" pattern.
//!
//! We can't safely `fork()` a multithreaded Rust runtime directly, so instead
//! the parent re-executes its own binary (`/proc/self/exe`) into the hidden
//! `child-init` subcommand, using `Command::pre_exec` to call `unshare()`
//! in the freshly-forked child *before* it execs.

use std::io;
use std::os::unix::process::CommandExt;
use std::path::Path;
use std::process::{Child, Command};

use anyhow::{Context, Result};
use nix::sched::CloneFlags;

/// Spawn `child-init --rootfs <rootfs> -- <command>` inside new PID, mount,
/// UTS, and IPC namespaces.
///
/// Note: `unshare(CLONE_NEWPID)` only affects processes forked *after* the
/// call — it does not move the calling process itself into the new PID
/// namespace, and this remains true even after that process execs into
/// `child-init`. `child-init` compensates by forking once more internally
/// (see `container::child_init`) so that the user's command actually lands
/// as PID 1 in the new namespace.
pub fn spawn_isolated(rootfs: &Path, command: &[String]) -> Result<Child> {
    let exe = std::env::current_exe().context("resolving path to our own binary")?;

    let mut cmd = Command::new(exe);
    cmd.arg("child-init")
        .arg("--rootfs")
        .arg(rootfs)
        .arg("--")
        .args(command);

    // Safety: the closure only calls async-signal-safe syscalls (unshare),
    // runs in the freshly-forked child before exec, and does not touch any
    // Rust runtime state shared with the parent.
    unsafe {
        cmd.pre_exec(|| {
            nix::sched::unshare(
                CloneFlags::CLONE_NEWPID
                    | CloneFlags::CLONE_NEWUTS
                    | CloneFlags::CLONE_NEWNS
                    | CloneFlags::CLONE_NEWIPC,
            )
            .map_err(|errno| io::Error::from_raw_os_error(errno as i32))?;
            Ok(())
        });
    }

    let child = cmd
        .spawn()
        .context("spawning child-init (are you running as root?)")?;
    Ok(child)
}
