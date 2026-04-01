# tmux process subsystem: architecture notes for prux

## Purpose

This document records which parts of `tmux` matter for `prux`.

`prux` does not try to port the whole terminal multiplexer. It only cares about the low-level process and PTY behavior that makes `tmux` robust:

- spawning interactive workloads inside a PTY
- preparing the child terminal state before `exec`
- keeping child lifecycle tracking consistent through `waitpid`
- resolving the foreground workload attached to a PTY
- avoiding fragile signal and process-group behavior

## What tmux actually looks like

The relevant logic in `tmux` is not concentrated in one "process object". It is spread across several source files:

- `spawn.c`: PTY spawn path for panes and respawn operations
- `server.c`: central child reaping through `SIGCHLD` and `waitpid`
- `proc.c`: signal setup and server runtime plumbing
- `job.c`: separate background-job subsystem
- `window.c`: pane PTY I/O wiring
- `format.c` and `osdep-*.c`: current-path, pid, and related introspection helpers

This matters because `prux` cannot be a file-by-file clone of `tmux`. The low-level mechanics can be ported literally, but the public library shape has to be smaller and library-oriented.

## The critical tmux behaviors

### Spawn path

For `prux`, the most important `tmux` logic lives in the pane spawn path:

- compute the launch cwd
- temporarily switch the parent process cwd before `forkpty`
- block signals around the fork boundary
- create the child attached to the PTY
- restore parent cwd after fork
- clear inherited signal handlers in the child
- prepare child `termios`
- construct the final environment
- dispatch one of the `exec` modes

The important point is not style. It is the exact order of operations. In PTY code, changing that order can easily reintroduce broken interrupt and terminal behavior.

### Child lifecycle

`tmux` does not let each pane manage its own child reaping independently. Child exit and stop events are funneled through a central path:

- `SIGCHLD` reaches the server runtime
- the server loops on `waitpid(..., WNOHANG | WUNTRACED)`
- exited and stopped children are recorded centrally
- stopped children may be continued to avoid a wedged server-side state

For `prux`, this is the key model to preserve. The library does not need a full `tmux` server, but it does need the same disciplined reaping behavior.

### PTY foreground introspection

`tmux` does not define "current pane directory" as "cwd of the shell pid". Instead it tracks the foreground process group attached to the PTY and resolves cwd from that foreground workload.

That is why `prux::current_dir()` is defined as a best-effort lookup of the current foreground job attached to the PTY. This is closer to actual terminal semantics than shell-pid-based guessing.

## What prux ports literally

The following ideas are the ones worth preserving as literally as possible:

- signal masking around fork
- parent-side cwd switching before `forkpty`
- child-side signal reset before `exec`
- child terminal preparation before `exec`
- `waitpid(..., WNOHANG | WUNTRACED)` as the basis of reaping
- stopped-child handling consistent with `tmux`
- foreground-process-group-based PTY introspection on Linux

These are the areas where "slightly cleaner" alternative implementations most often drift into broken PTY behavior.

## What prux does not port literally

`prux` intentionally does not reproduce:

- pane and window lifecycle
- `tmux` server/client architecture
- terminal parsing and screen model
- control mode
- the full `job.c` subsystem
- multi-platform osdep support beyond Linux

Those pieces are real parts of `tmux`, but they are not required for a compact process-session library.

## How this maps to the current prux code

The current repository reflects the `tmux` analysis in these modules:

- `src/spawn.rs`: PTY spawn path and exec dispatch
- `src/os/linux.rs`: Linux low-level helpers, signal reset, termios prep, cwd switching, and introspection support
- `src/reaper.rs`: centralized child-state tracking around `waitpid`
- `src/session.rs`: high-level session object built on top of the low-level pieces
- `src/introspection.rs`: foreground process-group and cwd lookup helpers

The corresponding regression coverage lives in `tests/process_session.rs`.

## Testing implications

`tmux` mainly validates this subsystem through end-to-end regressions. `prux` follows the same spirit, but in a smaller form:

- spawn a shell in a PTY
- read and write through the PTY master
- deliver interrupt through PTY semantics
- verify the shell remains usable after interrupt
- verify resize
- verify reaping and dead-state detection
- verify foreground cwd tracking
- verify late-interrupt and stopped-child regressions

For this project, those behavior-level tests are more valuable than isolated unit tests over tiny helpers.
