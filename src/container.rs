//! Process lifecycle orchestration for both halves of the self-exec pattern:
//! `run` (host-side parent) and `child_init` (re-executed inside new
//! namespaces).

use std::ffi::CString;
use std::path::PathBuf;

use anyhow::{Context, Result};
use nix::sys::wait::{waitpid, WaitStatus};
use nix::unistd::{self, ForkResult, Pid};

use crate::{cgroups, filesystem, namespaces};

/// Host-side entrypoint for `contain-rs run`.
pub fn run(rootfs: PathBuf, memory: Option<String>, cpu: Option<f64>, command: Vec<String>) -> Result<()> {
    if !unistd::Uid::effective().is_root() {
        anyhow::bail!("contain-rs must be run as root, e.g.: sudo contain-rs run ...");
    }
    anyhow::ensure!(!command.is_empty(), "no command given (use `-- /bin/sh` etc.)");

    let rootfs = std::fs::canonicalize(&rootfs)
        .with_context(|| format!("resolving --rootfs {rootfs:?}"))?;

    cgroups::enable_controllers();

    let mut child = namespaces::spawn_isolated(&rootfs, &command)?;
    let pid = Pid::from_raw(child.id() as i32);

    // Wire up resource limits while the container's init process is starting
    // up (it's still busy doing mount/pivot_root work at this point).
    let cgroup_dir = cgroups::create_cgroup(pid)?;
    cgroups::add_process(&cgroup_dir, pid)
        .context("adding container process to its cgroup")?;
    if let Some(memory) = &memory {
        cgroups::set_memory_limit(&cgroup_dir, memory)?;
        tracing::info!("memory limit set to {memory}");
    }
    if let Some(cpu) = cpu {
        cgroups::set_cpu_limit(&cgroup_dir, cpu)?;
        tracing::info!("cpu limit set to {:.0}% of one core", cpu * 100.0);
    }

    let status = child.wait().context("waiting for container process")?;
    cgroups::cleanup(&cgroup_dir);

    std::process::exit(status.code().unwrap_or(1));
}

/// Re-executed inside new PID/UTS/MNT/IPC namespaces (see `namespaces::spawn_isolated`).
///
/// IMPORTANT: `unshare(CLONE_NEWPID)` does not move *this* process into the
/// new PID namespace — only processes it forks from here on out. So before
/// we can correctly become "PID 1" for the container, we must fork once
/// more. The grandchild below is the one that actually becomes PID 1 and
/// execs the user's command; this process just supervises and relays the
/// exit status, similar to how a real container's shim process works.
pub fn child_init(rootfs: PathBuf, command: Vec<String>) -> Result<()> {
    unistd::sethostname("isolated-box").context("setting container hostname")?;
    filesystem::make_root_private()?;
    filesystem::pivot_root_into(&rootfs)?;

    match unsafe { unistd::fork().context("forking container init process")? } {
        ForkResult::Parent { child } => {
            let status = waitpid(child, None).context("waiting for container init")?;
            std::process::exit(exit_code_from(status));
        }
        ForkResult::Child => {
            // We are now PID 1 inside the new PID namespace.
            filesystem::mount_proc_and_sys()?;
            exec_user_command(&command)?;
            unreachable!("execvp only returns on error, which is handled above");
        }
    }
}

fn exec_user_command(command: &[String]) -> Result<()> {
    anyhow::ensure!(!command.is_empty(), "no command to exec inside container");

    let program = CString::new(command[0].as_str())
        .context("command contains an interior NUL byte")?;
    let args: Result<Vec<CString>> = command
        .iter()
        .map(|s| CString::new(s.as_str()).context("argument contains an interior NUL byte"))
        .collect();
    let args = args?;

    // On success this never returns: the process image is replaced.
    unistd::execvp(&program, &args)
        .with_context(|| format!("exec failed for {:?} (does it exist inside the rootfs?)", command[0]))?;
    Ok(())
}

fn exit_code_from(status: WaitStatus) -> i32 {
    match status {
        WaitStatus::Exited(_, code) => code,
        WaitStatus::Signaled(_, signal, _) => 128 + signal as i32,
        _ => 1,
    }
}
