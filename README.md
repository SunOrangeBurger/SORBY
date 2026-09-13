# SORBY

**S**ORBY **O**perates **R**ust **B**inary **Y**ards — named in proper linux tradition.

SORBY is an educational(maybe), dependency-minimal container runtime written from
scratch in Rust(that's the selling point). It explores the low-level Linux kernel features that power
modern containerization platforms like Docker, containerd, and runc — by
talking to the kernel directly instead of wrapping an existing daemon.

```
sudo sorby run \
  --rootfs ./alpine-rootfs \
  --memory 50M \
  --cpu 0.2 \
  --max-pids 64 \
  -- /bin/sh
```

`--max-pids` defaults to 64 even if you omit it — see "Fork bomb / PID limit" below.

## Key capabilities

- **Namespace isolation** — partitions PID, mount, UTS (hostname), and IPC
  namespaces via `unshare`/`clone`.
- **Hard resource ceilings** — Linux Cgroups v2 enforce memory quotas (kernel
  OOM-kills on violation), CPU bandwidth throttling, and a process-count cap.
- **Filesystem sandboxing** — `pivot_root` traps the process inside an
  isolated rootfs (e.g. Alpine Linux) with a fresh, private `/proc` and `/sys`.
- **Zero-daemon architecture** — a single static-ish CLI binary using the
  self-exec fork pattern to manage the process lifecycle safely in Rust.

## Requirements

- Linux kernel ≥ 5.8 (Cgroups v2 support), most modern distro.
- Root / sudo.
- Rust (`rustup` — stable toolchain is fine).

## Setup

```bash
# 1. Get a minimal rootfs to run inside
mkdir -p ./alpine-rootfs
curl -o alpine.tar.gz https://dl-cdn.alpinelinux.org/alpine/v3.19/releases/x86_64/alpine-minirootfs-3.19.1-x86_64.tar.gz
tar -xzf alpine.tar.gz -C ./alpine-rootfs

# 2. Build
cargo build --release
```

## Run

```bash
sudo ./target/release/sorby run \
  --rootfs ./alpine-rootfs \
  --memory 50M \
  --cpu 0.2 \
  -- /bin/sh
```

## Validate isolation

All of the following have been run and confirmed on my lovely linux machine(Fedora
Workstation). We caught a few bugs along the way and sorted them out.

**PID namespace** — inside the shell, `ps aux` should show only your shell
(as PID 1) and nothing from the host:

```
/ # ps aux
PID   USER     TIME  COMMAND
    1 root      0:00 /bin/sh
    2 root      0:00 ps aux
```

**Hostname namespace**:

```
/ # hostname
isolated-box
```

**Cgroups memory enforcement** — start with `--memory 20M`, then inside the
container try to allocate more than that:

```
/ # echo "nameserver 8.8.8.8" > /etc/resolv.conf   # see "DNS resolution" note below
/ # apk add python3
/ # python3 -c 'x = "a" * (50 * 1024 * 1024)'
Killed
```

**CPU bandwidth throttling** — run with `--cpu 0.2`, then inside spin a
busy loop in the background and watch it from the host:

```
/ # yes > /dev/null &
```

```bash
# on the host, in a separate terminal
top
```

The `yes` process should sit around 20% CPU, not 100% — cgroups v2 doesn't
hide the process from the host's `ps`/`top`, only namespaces the container's
own view.

**Fork bomb / PID limit** — with the default `--max-pids 64`, a fork bomb
inside the container hits `EAGAIN` well before it can take down the host.
BusyBox `ash` rejects the classic `:(){ :|:& };:` one-liner (`:` is a
reserved builtin name there), so use a named function instead:

```
/ # bomb(){ bomb|bomb& }; bomb
...
-sh: can't fork
```

**Mount namespace isolation** — mount events inside the container shouldn't
propagate to the host's mount table:

```
/ # mount -t tmpfs tmpfs /mnt
```

```bash
# on the host
findmnt | grep tmpfs
```

The tmpfs you just created inside the container should not appear in the
host's list.

The (ignored-by-default) tests in `tests/integration.rs` automate the PID,
hostname, and memory checks — run them explicitly with:

```bash
sudo -E cargo test -- --ignored --nocapture
```

They're `#[ignore]`d because creating new namespaces needs `CAP_SYS_ADMIN`,
which most CI runners and sandboxed containers don't grant.


## Security posture / threat model — read this before trusting it with anything untrusted

SORBY isolates *well-behaved-but-resource-hungry* code well. It is **not** a
sandbox suitable for deliberately malicious binaries, and shouldn't be
treated as one. Specifically, as of this version(this where we open up future scope):

- The container process runs as **real root on the host** — there's no
  `CLONE_NEWUSER`/UID remapping, so "root inside the container" is
  indistinguishable from "root on the host" as far as the kernel's
  permission checks are concerned. A namespace escape (a well-studied bug
  category, not a hypothetical) means full host compromise, not a contained
  failure.
- **No Linux capabilities are dropped.** The process retains `CAP_SYS_ADMIN`,
  `CAP_SYS_PTRACE`, and everything else root normally has — plenty to cause
  damage even without an escape.
- **No seccomp filtering** — every syscall is available. (Still a planned
  "Extending it further" item, not yet built.)
- **No network isolation** — no `CLONE_NEWNET`; the container shares the
  host's network stack outright.
- **No disk-space quota** — nothing stops a process from filling the host
  disk via writes into the mounted rootfs.
- **Resource ceilings ARE real**, per the tests above: memory (incl. swap),
  CPU bandwidth, and process count are all enforced by the kernel's cgroups
  v2 controllers, not just measured.

If the goal is "run code I don't fully trust without risking the host,"
resource limits alone don't get you there — the missing pieces above
(user namespaces, capability dropping, seccomp, network isolation, disk
quotas) are what separate this from something like gVisor, Firecracker
microVMs, or Docker run with `--security-opt`, `--cap-drop`, and rootless
mode. Treat SORBY as a teaching tool and a resource-governor for code you
already trust not to be adversarial, not as a malware sandbox(yet).

## Repository layout

```
sorby/
├── Cargo.toml
├── src/
│   ├── main.rs         # CLI entry point (Run vs ChildInit)
│   ├── container.rs    # Core process orchestration (run + child_init)
│   ├── namespaces.rs   # unshare/self-exec abstraction
│   ├── filesystem.rs   # mounts, pivot_root, /proc + /sys setup
│   └── cgroups.rs      # Cgroups v2 file reader/writer
├── tests/
│   └── integration.rs  # Isolation + limit-enforcement tests (--ignored)
└── README.md
```

## Extending it further

Ranked roughly by how much closer each gets you to a genuine "safe mode
launcher" for untrusted code, and how much work each is:

1. **`pids.max` fork-bomb limit** — done (`--max-pids`, default 64).
2. **Seccomp filters** — via the `seccomp` crate, allowlist or denylist
   syscalls (block `reboot`, `swapon`, `mount`, `ptrace`, `kexec_load`, etc.).
   Moderate effort, meaningfully raises the bar against exploitation.
3. **Capability dropping** — before exec, drop everything except the
   minimal set the user's command actually needs (`capset`/`prctl`). Low-to-
   moderate effort, high value — most privilege-escalation paths need a
   capability SORBY currently just hands over for free.
4. **User namespaces (`CLONE_NEWUSER`)** — map container root to an
   unprivileged host UID. This is the single biggest step toward real
   security, and also the most invasive: it changes how cgroups delegation,
   file ownership, and `pivot_root` permissions all work, and typically needs
   `newuidmap`/`newgidmap` setuid helpers. High effort.
5. **OverlayFS (copy-on-write)** — mount an overlay with the extracted
   rootfs as a read-only lower layer and a scratch upper layer, discarded on
   exit, so the base image can't be corrupted and disk usage from a single
   run is bounded by the upper layer's size (which can itself be capped via
   a size-limited tmpfs).
6. **Network isolation (`CLONE_NEWNET`)** — cuts off host network access
   entirely by default; a veth pair can be added back deliberately if a
   container needs egress.
7. **Image puller** — fetch and unpack images from a Docker-compatible
   registry API without needing Docker installed. (Convenience, not security.)

## Where on earth are we know?

Currently we are waiting for some inspiration and need to strike us, and we will continue on our journey towards perfect containerization against malicious code, binaries, and basically creating a safe mode for any and all applications on linux, with the ability to control CPU, disk and network access, and so much more.