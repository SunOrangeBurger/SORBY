//! Filesystem sandboxing: pivot_root into the container's rootfs and mount
//! fresh, namespace-scoped /proc and /sys.

use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use nix::mount::{mount, umount2, MntFlags, MsFlags};
use nix::unistd;

/// Mark the whole mount tree as private (MS_PRIVATE|MS_REC) so that mount/
/// unmount events inside the container's mount namespace never propagate
/// back out to the host, and vice versa.
pub fn make_root_private() -> Result<()> {
    mount(
        None::<&str>,
        "/",
        None::<&str>,
        MsFlags::MS_PRIVATE | MsFlags::MS_REC,
        None::<&str>,
    )
    .context("remounting / as MS_PRIVATE")?;
    Ok(())
}

/// Swap the process's root filesystem to `rootfs` using pivot_root, which
/// (unlike chroot) actually changes the root mount rather than just a path
/// pointer, so the old root can be fully detached afterward.
pub fn pivot_root_into(rootfs: &Path) -> Result<()> {
    let rootfs = fs::canonicalize(rootfs)
        .with_context(|| format!("resolving rootfs path {:?}", rootfs))?;

    // pivot_root requires new_root to be a mount point in its own right, so
    // bind-mount it onto itself. (`nix`'s path-taking functions are generic
    // over `NixPath` and don't auto-deref `&PathBuf` -> `&Path`, so we pass
    // `.as_path()` explicitly throughout this function.)
    mount(
        Some(rootfs.as_path()),
        rootfs.as_path(),
        None::<&str>,
        MsFlags::MS_BIND | MsFlags::MS_REC,
        None::<&str>,
    )
    .context("bind-mounting rootfs onto itself")?;

    let old_root = rootfs.join(".old_root");
    fs::create_dir_all(&old_root).context("creating .old_root staging dir")?;

    unistd::pivot_root(rootfs.as_path(), old_root.as_path()).context("pivot_root failed")?;

    // After pivot_root, the new root is "/" and the old root is mounted at
    // /.old_root (paths are relative to the *new* root now).
    unistd::chdir("/").context("chdir to new root")?;

    let old_root_new_path = PathBuf::from("/.old_root");
    umount2(old_root_new_path.as_path(), MntFlags::MNT_DETACH)
        .context("lazily unmounting old root")?;
    fs::remove_dir(&old_root_new_path).context("removing .old_root mountpoint")?;

    Ok(())
}

/// Mount fresh /proc and /sys inside the container. Must be called *after*
/// the process is actually running inside the new PID namespace (i.e. after
/// the second fork in `container::child_init`), since /proc reflects the
/// caller's own PID namespace.
pub fn mount_proc_and_sys() -> Result<()> {
    fs::create_dir_all("/proc").ok();
    mount(
        Some("proc"),
        "/proc",
        Some("proc"),
        MsFlags::empty(),
        None::<&str>,
    )
    .context("mounting /proc")?;

    fs::create_dir_all("/sys").ok();
    mount(
        Some("sysfs"),
        "/sys",
        Some("sysfs"),
        MsFlags::empty(),
        None::<&str>,
    )
    .context("mounting /sys")?;

    Ok(())
}
