# prux

`prux` is a Rust library for running interactive programs inside a PTY with low-level process semantics taken from the critical process-management paths of `tmux`.

The project exists because PTY code is easy to get almost right and still end up with broken terminal state after `SIGINT`, orphaned process groups, or fragile child reaping. For the critical Linux process path, `prux` prefers a literal or near-literal translation of the relevant `tmux` mechanics over a "cleaner" abstraction that changes observable behavior.

## Goals

- Preserve the critical PTY/process invariants that make `tmux` robust.
- Expose a small library-facing API instead of a full terminal multiplexer architecture.
- Stay Linux-first until the behavior is well proven.
- Be sufficient for implementing a `ProcessSession`-style runtime in higher-level applications.

## Current Scope

`prux` currently supports Linux only and focuses on interactive PTY sessions:

- spawning a child process inside a PTY
- nonblocking PTY reads and writes
- PTY resize
- interrupt delivery through terminal semantics
- child reaping with stopped/exited handling
- best-effort current working directory introspection for the foreground PTY workload

This crate does not currently implement the broader `tmux` job subsystem, terminal emulation, pane/window management, or non-Linux platform support.

## Public Interface

The main entry point is [`ProcessSession`](/src/session.rs), configured with [`ProcessSessionConfig`](/src/session.rs).

```rust
use std::collections::BTreeMap;
use std::path::PathBuf;

use prux::{ProcessSession, ProcessSessionConfig};

let mut session = ProcessSession::spawn(ProcessSessionConfig {
    program: PathBuf::from("/bin/sh"),
    args: Vec::new(),
    initial_cwd: std::env::current_dir().unwrap(),
    debug_log_path: None,
    env: BTreeMap::new(),
})?;

session.write_all(b"printf 'ready\\n'\n")?;
let output = session.try_read()?;
session.send_interrupt()?;
session.resize(120, 40)?;
let cwd = session.current_dir()?;
let alive = session.is_alive()?;
session.terminate()?;
# Ok::<(), prux::ProcessError>(())
```

`ProcessSessionConfig::debug_log_path` is currently accepted for compatibility with downstream callers but is not used by `prux` itself.

## Design Notes

- The child is spawned through a PTY-backed path with signal masking around fork and child-side signal cleanup before `exec`.
- Interrupt delivery writes the PTY interrupt byte instead of using a naive `kill(pid, SIGINT)` shortcut.
- Child lifecycle tracking is centralized around `waitpid(..., WNOHANG | WUNTRACED)`.
- `current_dir()` is defined as a best-effort lookup of the foreground workload attached to the PTY, not as "the shell's original cwd".

## Repository Layout

- [`src/session.rs`](/src/session.rs): high-level session API
- [`src/spawn.rs`](/src/spawn.rs): PTY spawn path and exec dispatch
- [`src/reaper.rs`](/src/reaper.rs): child lifecycle tracking
- [`src/introspection.rs`](/src/introspection.rs): current foreground process-group lookup helpers
- [`src/os/linux.rs`](/src/os/linux.rs): Linux-specific low-level operations
- [`tests/process_session.rs`](/tests/process_session.rs): integration and regression tests
- [`docs/engineering/idea-engineering.md`](/docs/engineering/idea-engineering.md): engineering intent and scope
- [`docs/engineering/tmux-process-subsystem-architecture.md`](/docs/engineering/tmux-process-subsystem-architecture.md): source-analysis notes about the relevant `tmux` subsystem

## Status

The current implementation covers the Linux-first process-session core. The repository also contains engineering notes and a local planning area that is intentionally excluded from Git.
