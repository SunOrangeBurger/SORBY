//! End-to-end isolation tests.
//!
//! These require: Linux, root privileges, cgroups v2, and an extracted
//! rootfs at `./alpine-rootfs` (see README). They also can't run in most
//! sandboxed CI containers, which typically lack CAP_SYS_ADMIN for creating
//! new namespaces — hence `#[ignore]`. Run explicitly with:
//!
//!   sudo -E cargo test -- --ignored --nocapture

use std::process::Command;

fn sorby_bin() -> &'static str {
    env!("CARGO_BIN_EXE_sorby")
}

#[test]
#[ignore]
fn pid_namespace_isolation_reports_pid_1() {
    let output = Command::new(sorby_bin())
        .args([
            "run",
            "--rootfs",
            "./alpine-rootfs",
            "--",
            "/bin/sh",
            "-c",
            "echo $$",
        ])
        .output()
        .expect("failed to run contain-rs");

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert_eq!(stdout.trim(), "1", "container's shell should see itself as PID 1");
}

#[test]
#[ignore]
fn hostname_isolation_does_not_leak() {
    let output = Command::new(sorby_bin())
        .args(["run", "--rootfs", "./alpine-rootfs", "--", "hostname"])
        .output()
        .expect("failed to run contain-rs");

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert_eq!(stdout.trim(), "isolated-box");
}

#[test]
#[ignore]
fn memory_limit_triggers_oom_kill() {
    let status = Command::new(sorby_bin())
        .args([
            "run",
            "--rootfs",
            "./alpine-rootfs",
            "--memory",
            "20M",
            "--",
            "python3",
            "-c",
            "x = 'a' * (50 * 1024 * 1024)",
        ])
        .status()
        .expect("failed to run contain-rs");

    // OOM-killed processes exit via SIGKILL; our exit code convention is 128+signal.
    assert_eq!(status.code(), Some(128 + 9), "expected the process to be OOM-killed");
}
