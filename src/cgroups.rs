//! Cgroups v2 resource limiting. All of this is plain file I/O against
//! cgroupfs — no special crate needed, which is the whole appeal of v2's
//! unified hierarchy.

use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use nix::unistd::Pid;

const CGROUP_ROOT: &str = "/sys/fs/cgroup";

/// Try to enable the memory and cpu controllers for children of the root
/// cgroup. This is required before writing memory.max/cpu.max in a child
/// cgroup will have any effect. Best-effort: on many distros (systemd-managed
/// cgroup trees) this is already enabled and the write may fail harmlessly
/// (e.g. because processes already live directly in the root cgroup) — we
/// don't treat that as fatal, since the limits still often apply once the
/// container's cgroup is created as a leaf.
pub fn enable_controllers() {
    let path = format!("{CGROUP_ROOT}/cgroup.subtree_control");
    if let Err(e) = fs::write(&path, "+memory +cpu") {
        tracing::debug!("could not enable memory/cpu controllers at {path}: {e} \
            (this is often already enabled by systemd; continuing)");
    }
}

/// Create `/sys/fs/cgroup/sorby_<pid>` and return its path.
pub fn create_cgroup(pid: Pid) -> Result<PathBuf> {
    let dir = PathBuf::from(format!("{CGROUP_ROOT}/sorby_{pid}"));
    fs::create_dir_all(&dir)
        .with_context(|| format!("creating cgroup dir {dir:?} (are you root?)"))?;
    Ok(dir)
}

/// Move `pid` into the given cgroup.
pub fn add_process(dir: &Path, pid: Pid) -> Result<()> {
    fs::write(dir.join("cgroup.procs"), pid.to_string())
        .with_context(|| format!("adding pid {pid} to cgroup {dir:?}"))?;
    Ok(())
}

/// Set a hard memory ceiling, e.g. "50M", "1.5G", "1024K", or a bare byte count.
/// Set a hard memory ceiling, e.g. "50M", "1.5G", "1024K", or a bare byte count.
///
/// Also caps `memory.swap.max` at 0 for this cgroup. Without this, a host
/// with swap enabled (e.g. Fedora's default zram swap) will happily push the
/// container's excess anonymous pages to swap instead of invoking the OOM
/// killer — so the limit appears to do nothing even though it's set
/// correctly. Docker does the equivalent by default (`--memory-swap` tracks
/// `--memory` unless overridden).
pub fn set_memory_limit(dir: &Path, memory: &str) -> Result<()> {
    let bytes = parse_size_to_bytes(memory)
        .with_context(|| format!("parsing memory limit {memory:?}"))?;
    fs::write(dir.join("memory.max"), bytes.to_string())
        .context("writing memory.max")?;

    // Best-effort: some kernels/configs don't expose memory.swap.max (e.g.
    // swap accounting disabled entirely), so don't treat failure as fatal.
    if let Err(e) = fs::write(dir.join("memory.swap.max"), "0") {
        tracing::debug!("could not cap memory.swap.max (swap accounting may be disabled): {e}");
    }

    Ok(())
}

/// Set a CPU bandwidth limit as a fraction of one core (e.g. 0.2 == 20%),
/// using a 100ms accounting period: quota = fraction * period.
pub fn set_cpu_limit(dir: &Path, fraction_of_core: f64) -> Result<()> {
    anyhow::ensure!(
        fraction_of_core > 0.0,
        "cpu limit must be a positive fraction of a core"
    );
    let period_us: u64 = 100_000;
    let quota_us = (fraction_of_core * period_us as f64).round() as u64;
    fs::write(dir.join("cpu.max"), format!("{quota_us} {period_us}"))
        .context("writing cpu.max")?;
    Ok(())
}

/// Remove the cgroup directory once the container has exited and no
/// processes remain in it.
pub fn cleanup(dir: &Path) {
    if let Err(e) = fs::remove_dir(dir) {
        tracing::warn!("could not remove cgroup dir {dir:?}: {e}");
    }
}

/// Parses sizes like "50M", "1.5G", "2048K", or a plain byte count into bytes.
pub(crate) fn parse_size_to_bytes(s: &str) -> Result<u64> {
    let s = s.trim();
    anyhow::ensure!(!s.is_empty(), "empty size string");

    let (num_part, multiplier) = match s.chars().last().unwrap().to_ascii_uppercase() {
        'G' => (&s[..s.len() - 1], 1024u64.pow(3)),
        'M' => (&s[..s.len() - 1], 1024u64.pow(2)),
        'K' => (&s[..s.len() - 1], 1024u64),
        'B' => (&s[..s.len() - 1], 1u64),
        c if c.is_ascii_digit() => (s, 1u64),
        c => anyhow::bail!("unrecognized size suffix '{c}' in {s:?}"),
    };

    let value: f64 = num_part
        .parse()
        .with_context(|| format!("invalid numeric size {num_part:?}"))?;
    anyhow::ensure!(value > 0.0, "size must be positive");

    Ok((value * multiplier as f64) as u64)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_megabytes() {
        assert_eq!(parse_size_to_bytes("50M").unwrap(), 50 * 1024 * 1024);
    }

    #[test]
    fn parses_gigabytes_fractional() {
        assert_eq!(parse_size_to_bytes("1.5G").unwrap(), (1.5 * 1024f64.powi(3)) as u64);
    }

    #[test]
    fn parses_bare_bytes() {
        assert_eq!(parse_size_to_bytes("2048").unwrap(), 2048);
    }

    #[test]
    fn rejects_garbage() {
        assert!(parse_size_to_bytes("nope").is_err());
    }
}
