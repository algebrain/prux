use std::ffi::CStr;
use std::fs;
use std::io;
use std::mem::MaybeUninit;
use std::os::fd::RawFd;
use std::os::raw::{c_char, c_int, c_short, c_ulong, c_void};
use std::path::PathBuf;
use std::ptr;

use crate::reaper::ChildState;

pub const SIGINT: c_int = 2;
pub const SIGKILL: c_int = 9;
pub const SIGTERM: c_int = 15;
pub const SIGHUP_SIGNAL: c_int = 1;
pub const SIGCONT_SIGNAL: c_int = 18;
pub const DEFAULT_INTR_BYTE: u8 = 0x03;

const EINTR: i32 = 4;
const EAGAIN: i32 = 11;
const EIO: i32 = 5;

const F_GETFL: c_int = 3;
const F_SETFL: c_int = 4;
const O_NONBLOCK: c_int = 0o4000;

const POLLIN: c_short = 0x001;
const POLLOUT: c_short = 0x004;

const TIOCSWINSZ: c_ulong = 0x5414;
const TIOCGSID: c_ulong = 0x5429;

const STDIN_FILENO: c_int = 0;
const WAIT_ANY: c_int = -1;

const WNOHANG: c_int = 1;
const WUNTRACED: c_int = 2;

#[repr(C)]
struct Winsize {
    ws_row: u16,
    ws_col: u16,
    ws_xpixel: u16,
    ws_ypixel: u16,
}

#[repr(C)]
struct PollFd {
    fd: c_int,
    events: c_short,
    revents: c_short,
}

#[repr(C)]
struct SigAction {
    sa_handler: usize,
    sa_flags: usize,
    sa_restorer: usize,
    sa_mask: SigSet,
}

unsafe extern "C" {
    #[link_name = "forkpty"]
    fn libc_forkpty(
        amaster: *mut c_int,
        name: *mut c_char,
        termp: *const c_void,
        winp: *const Winsize,
    ) -> c_int;
    #[link_name = "fcntl"]
    fn libc_fcntl(fd: c_int, cmd: c_int, ...) -> c_int;
    #[link_name = "close"]
    fn libc_close(fd: c_int) -> c_int;
    #[link_name = "read"]
    fn libc_read(fd: c_int, buf: *mut c_void, count: usize) -> isize;
    #[link_name = "write"]
    fn libc_write(fd: c_int, buf: *const c_void, count: usize) -> isize;
    #[link_name = "ioctl"]
    fn libc_ioctl(fd: c_int, request: c_ulong, ...) -> c_int;
    #[link_name = "tcgetpgrp"]
    fn libc_tcgetpgrp(fd: c_int) -> c_int;
    #[link_name = "waitpid"]
    fn libc_waitpid(pid: c_int, status: *mut c_int, options: c_int) -> c_int;
    #[link_name = "kill"]
    fn libc_kill(pid: c_int, sig: c_int) -> c_int;
    #[link_name = "killpg"]
    fn libc_killpg(pgrp: c_int, sig: c_int) -> c_int;
    #[link_name = "poll"]
    fn libc_poll(fds: *mut PollFd, nfds: usize, timeout: c_int) -> c_int;
    #[link_name = "chdir"]
    fn libc_chdir(path: *const c_char) -> c_int;
    #[link_name = "execve"]
    fn libc_execve(
        pathname: *const c_char,
        argv: *const *const c_char,
        envp: *const *const c_char,
    ) -> c_int;
    #[link_name = "execvpe"]
    fn libc_execvpe(
        file: *const c_char,
        argv: *const *const c_char,
        envp: *const *const c_char,
    ) -> c_int;
    #[link_name = "_exit"]
    fn libc__exit(status: c_int) -> !;
    #[link_name = "sigaction"]
    fn libc_sigaction(signum: c_int, act: *const SigAction, oldact: *mut SigAction) -> c_int;
    #[link_name = "sigemptyset"]
    fn libc_sigemptyset(set: *mut SigSet) -> c_int;
    #[link_name = "sigfillset"]
    fn libc_sigfillset(set: *mut SigSet) -> c_int;
    #[link_name = "sigprocmask"]
    fn libc_sigprocmask(how: c_int, set: *const SigSet, oldset: *mut SigSet) -> c_int;
    #[link_name = "tcgetattr"]
    fn libc_tcgetattr(fd: c_int, termios_p: *mut Termios) -> c_int;
    #[link_name = "tcsetattr"]
    fn libc_tcsetattr(fd: c_int, optional_actions: c_int, termios_p: *const Termios) -> c_int;
}

#[repr(C)]
pub struct SigSet {
    __val: [u64; 16],
}

#[repr(C)]
#[derive(Clone)]
struct Termios {
    c_iflag: u32,
    c_oflag: u32,
    c_cflag: u32,
    c_lflag: u32,
    c_line: u8,
    c_cc: [u8; 32],
    c_ispeed: u32,
    c_ospeed: u32,
}

#[derive(Clone)]
pub struct SavedTermios {
    termios: Termios,
}

const SIG_DFL: usize = 0;
const SIG_BLOCK: c_int = 0;
const SIG_SETMASK: c_int = 2;
const SIGHUP: c_int = SIGHUP_SIGNAL;
const SIGQUIT: c_int = 3;
const SIGPIPE: c_int = 13;
const SIGCHLD: c_int = 17;
const SIGCONT: c_int = SIGCONT_SIGNAL;
const SIGTSTP: c_int = 20;
const SIGTTIN: c_int = 21;
const SIGTTOU: c_int = 22;
const SIGUSR1: c_int = 10;
const SIGUSR2: c_int = 12;
const SIGWINCH: c_int = 28;
const TCSANOW: c_int = 0;
const IUTF8: u32 = 0o040000;
const SA_RESTART: usize = 0x10000000;
const VINTR_INDEX: usize = 0;
const VERASE_INDEX: usize = 2;

pub fn fork_pty(cols: u16, rows: u16) -> io::Result<(i32, RawFd)> {
    let mut master_fd = -1;
    let winsize = Winsize {
        ws_row: rows,
        ws_col: cols,
        ws_xpixel: 0,
        ws_ypixel: 0,
    };

    let pid = unsafe { libc_forkpty(&mut master_fd, ptr::null_mut(), ptr::null(), &winsize) };
    if pid < 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok((pid, master_fd))
    }
}

pub fn set_nonblocking(fd: RawFd) -> io::Result<()> {
    let flags = unsafe { libc_fcntl(fd, F_GETFL) };
    if flags < 0 {
        return Err(io::Error::last_os_error());
    }

    if unsafe { libc_fcntl(fd, F_SETFL, flags | O_NONBLOCK) } < 0 {
        return Err(io::Error::last_os_error());
    }

    Ok(())
}

pub fn read(fd: RawFd, buf: &mut [u8]) -> io::Result<usize> {
    let rc = unsafe { libc_read(fd, buf.as_mut_ptr().cast::<c_void>(), buf.len()) };
    if rc < 0 {
        Err(io_error_from_errno())
    } else {
        Ok(rc as usize)
    }
}

pub fn write(fd: RawFd, buf: &[u8]) -> io::Result<usize> {
    let rc = unsafe { libc_write(fd, buf.as_ptr().cast::<c_void>(), buf.len()) };
    if rc < 0 {
        Err(io_error_from_errno())
    } else {
        Ok(rc as usize)
    }
}

pub fn wait_writable(fd: RawFd) -> io::Result<()> {
    wait_fd(fd, POLLOUT)
}

pub fn wait_readable(fd: RawFd) -> io::Result<()> {
    wait_fd(fd, POLLIN)
}

fn wait_fd(fd: RawFd, events: c_short) -> io::Result<()> {
    let mut pollfd = PollFd {
        fd,
        events,
        revents: 0,
    };

    loop {
        let rc = unsafe { libc_poll(&mut pollfd, 1, 1_000) };
        if rc > 0 {
            return Ok(());
        }
        if rc == 0 {
            continue;
        }

        let err = io_error_from_errno();
        if err.kind() == io::ErrorKind::Interrupted {
            continue;
        }
        return Err(err);
    }
}

pub fn set_winsize(fd: RawFd, cols: u16, rows: u16) -> io::Result<()> {
    let winsize = Winsize {
        ws_row: rows,
        ws_col: cols,
        ws_xpixel: 0,
        ws_ypixel: 0,
    };

    if unsafe { libc_ioctl(fd, TIOCSWINSZ, &winsize) } < 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(())
    }
}

pub fn foreground_process_group(fd: RawFd) -> io::Result<i32> {
    let pgrp = unsafe { libc_tcgetpgrp(fd) };
    if pgrp < 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(pgrp)
    }
}

pub fn current_dir(fd: RawFd) -> io::Result<PathBuf> {
    let pgrp = foreground_process_group(fd)?;
    let proc_path = format!("/proc/{pgrp}/cwd");
    if let Ok(path) = fs::read_link(&proc_path) {
        return Ok(path);
    }

    let mut sid = MaybeUninit::<c_int>::uninit();
    if unsafe { libc_ioctl(fd, TIOCGSID, sid.as_mut_ptr()) } == 0 {
        let sid = unsafe { sid.assume_init() };
        let proc_path = format!("/proc/{sid}/cwd");
        if let Ok(path) = fs::read_link(&proc_path) {
            return Ok(path);
        }
    }

    Err(io::Error::new(
        io::ErrorKind::NotFound,
        "unable to resolve current working directory for PTY foreground process",
    ))
}

pub fn change_dir_with_fallback(path: &CStr) -> io::Result<String> {
    if change_dir(path.as_ptr()).is_ok() {
        return Ok(path.to_string_lossy().into_owned());
    }

    if let Ok(home) = std::env::var("HOME") {
        let home_c = std::ffi::CString::new(home.clone())
            .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "HOME contains NUL"))?;
        if change_dir(home_c.as_ptr()).is_ok() {
            return Ok(home);
        }
    }

    let root = std::ffi::CString::new("/").expect("static path contains no NUL");
    change_dir(root.as_ptr())?;
    Ok("/".to_string())
}

pub fn restore_working_dir(path: &PathBuf) -> io::Result<()> {
    let c_path = std::ffi::CString::new(path.as_os_str().as_encoded_bytes())
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "cwd contains NUL"))?;
    change_dir(c_path.as_ptr())
}

pub fn wait_pid_nonblocking(pid: i32) -> io::Result<Option<ChildState>> {
    let mut status = 0;
    let rc = unsafe { libc_waitpid(pid, &mut status, WNOHANG | WUNTRACED) };
    if rc < 0 {
        return Err(io::Error::last_os_error());
    }
    if rc == 0 {
        return Ok(None);
    }

    Ok(Some(decode_wait_status(status)))
}

pub fn reap_any_child_nonblocking() -> io::Result<Option<(i32, ChildState)>> {
    let mut status = 0;
    let rc = unsafe { libc_waitpid(WAIT_ANY, &mut status, WNOHANG | WUNTRACED) };
    if rc < 0 {
        let err = io::Error::last_os_error();
        if err.raw_os_error() == Some(10) {
            return Ok(None);
        }
        return Err(err);
    }
    if rc == 0 {
        return Ok(None);
    }

    Ok(Some((rc, decode_wait_status(status))))
}

fn decode_wait_status(status: i32) -> ChildState {
    if (status & 0x7f) == 0 {
        ChildState::Exited((status >> 8) & 0xff)
    } else if (status & 0xff) == 0x7f {
        ChildState::Stopped((status >> 8) & 0xff)
    } else {
        ChildState::Signaled(status & 0x7f)
    }
}

pub fn send_signal(pid: i32, sig: c_int) -> io::Result<()> {
    if unsafe { libc_kill(pid, sig) } < 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(())
    }
}

pub fn send_signal_to_group(pgrp: i32, sig: c_int) -> io::Result<()> {
    if unsafe { libc_killpg(pgrp, sig) } < 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(())
    }
}

pub fn resume_process_group_if_needed(pid: i32, stop_signal: i32) -> io::Result<()> {
    if stop_signal == SIGTTIN || stop_signal == SIGTTOU {
        return Ok(());
    }

    send_signal_to_group(pid, SIGCONT).or_else(|_| send_signal(pid, SIGCONT))
}

pub fn reset_signal_state_for_child() -> io::Result<()> {
    proc_clear_signals(true)?;

    let mut empty = SigSet { __val: [0; 16] };
    if unsafe { libc_sigemptyset(&mut empty) } < 0 {
        return Err(io::Error::last_os_error());
    }
    if unsafe { libc_sigprocmask(SIG_SETMASK, &empty, ptr::null_mut()) } < 0 {
        return Err(io::Error::last_os_error());
    }

    Ok(())
}

pub fn proc_clear_signals(defaults: bool) -> io::Result<()> {
    let mut mask = SigSet { __val: [0; 16] };
    if unsafe { libc_sigemptyset(&mut mask) } < 0 {
        return Err(io::Error::last_os_error());
    }

    let sa = SigAction {
        sa_handler: SIG_DFL,
        sa_flags: SA_RESTART,
        sa_restorer: 0,
        sa_mask: mask,
    };

    for sig in [SIGPIPE, SIGTSTP] {
        if unsafe { libc_sigaction(sig, &sa, ptr::null_mut()) } < 0 {
            return Err(io::Error::last_os_error());
        }
    }

    if defaults {
        for sig in [
            SIGINT, SIGQUIT, SIGHUP, SIGCHLD, SIGCONT, SIGTERM, SIGUSR1, SIGUSR2, SIGWINCH,
        ] {
            if unsafe { libc_sigaction(sig, &sa, ptr::null_mut()) } < 0 {
                return Err(io::Error::last_os_error());
            }
        }
    }

    Ok(())
}

pub fn block_all_signals() -> io::Result<SigSet> {
    let mut all = SigSet { __val: [0; 16] };
    if unsafe { libc_sigfillset(&mut all) } < 0 {
        return Err(io::Error::last_os_error());
    }

    let mut old = SigSet { __val: [0; 16] };
    if unsafe { libc_sigprocmask(SIG_BLOCK, &all, &mut old) } < 0 {
        return Err(io::Error::last_os_error());
    }

    Ok(old)
}

pub fn restore_signal_mask(old: &SigSet) -> io::Result<()> {
    if unsafe { libc_sigprocmask(SIG_SETMASK, old, ptr::null_mut()) } < 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(())
    }
}

pub fn capture_stdin_termios() -> io::Result<SavedTermios> {
    let mut termios = MaybeUninit::<Termios>::uninit();
    if unsafe { libc_tcgetattr(STDIN_FILENO, termios.as_mut_ptr()) } < 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(SavedTermios {
        termios: unsafe { termios.assume_init() },
    })
}

pub fn prepare_child_terminal(template: Option<&SavedTermios>) -> io::Result<()> {
    let mut termios = MaybeUninit::<Termios>::uninit();
    if unsafe { libc_tcgetattr(STDIN_FILENO, termios.as_mut_ptr()) } < 0 {
        return Err(io::Error::last_os_error());
    }
    let mut termios = unsafe { termios.assume_init() };
    if let Some(template) = template {
        termios.c_cc = template.termios.c_cc;
    }
    termios.c_iflag |= IUTF8;
    termios.c_cc[VERASE_INDEX] = 0x7f;
    if unsafe { libc_tcsetattr(STDIN_FILENO, TCSANOW, &termios) } < 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

pub fn interrupt_byte(fd: RawFd) -> io::Result<u8> {
    let mut termios = MaybeUninit::<Termios>::uninit();
    if unsafe { libc_tcgetattr(fd, termios.as_mut_ptr()) } < 0 {
        return Err(io::Error::last_os_error());
    }
    let termios = unsafe { termios.assume_init() };
    let byte = termios.c_cc[VINTR_INDEX];
    Ok(if byte == 0 { DEFAULT_INTR_BYTE } else { byte })
}

pub fn close_non_std_fds() {
    let Ok(entries) = fs::read_dir("/proc/self/fd") else {
        return;
    };

    let mut fds = Vec::new();
    for entry in entries.flatten() {
        let Ok(name) = entry.file_name().into_string() else {
            continue;
        };
        let Ok(fd) = name.parse::<c_int>() else {
            continue;
        };
        if fd > 2 {
            fds.push(fd);
        }
    }

    fds.sort_unstable();
    fds.dedup();
    for fd in fds {
        unsafe {
            libc_close(fd);
        }
    }
}

pub fn change_dir(path: *const c_char) -> io::Result<()> {
    if unsafe { libc_chdir(path) } < 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(())
    }
}

pub fn execve(
    program: *const c_char,
    argv: *const *const c_char,
    envp: *const *const c_char,
) -> io::Result<()> {
    if unsafe { libc_execve(program, argv, envp) } < 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(())
    }
}

pub fn execvpe(
    program: *const c_char,
    argv: *const *const c_char,
    envp: *const *const c_char,
) -> io::Result<()> {
    if unsafe { libc_execvpe(program, argv, envp) } < 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(())
    }
}

pub fn exit_immediately(status: i32) -> ! {
    unsafe { libc__exit(status) }
}

pub fn shell_path_from_env() -> String {
    std::env::var("SHELL").unwrap_or_else(|_| "/bin/sh".to_string())
}

pub fn shell_argv0(shell_path: &str, login: bool) -> io::Result<std::ffi::CString> {
    let tail = shell_path.rsplit('/').next().unwrap_or(shell_path);
    let argv0 = if login {
        format!("-{tail}")
    } else {
        tail.to_string()
    };
    std::ffi::CString::new(argv0)
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "shell argv0 contains NUL"))
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct ProcSummary {
    pub pid: i32,
    pub process_group_id: i32,
}

pub fn descendant_pids(root_pid: i32) -> Vec<i32> {
    let mut out = Vec::new();
    collect_descendant_pids(root_pid, &mut out);
    out
}

fn collect_descendant_pids(root_pid: i32, out: &mut Vec<i32>) {
    let path = PathBuf::from("/proc")
        .join(root_pid.to_string())
        .join("task")
        .join(root_pid.to_string())
        .join("children");
    let Ok(children) = fs::read_to_string(path) else {
        return;
    };
    for child in children.split_whitespace() {
        let Ok(pid) = child.parse::<i32>() else {
            continue;
        };
        out.push(pid);
        collect_descendant_pids(pid, out);
    }
}

pub fn lingering_tty_processes_for_interrupted_group(
    shell_pid: i32,
    shell_process_group_id: i32,
    interrupted_process_group_id: i32,
    descendants: &[i32],
) -> Vec<ProcSummary> {
    tty_attached_processes(shell_pid)
        .into_iter()
        .filter(|summary| summary.pid != shell_pid)
        .filter(|summary| summary.process_group_id != shell_process_group_id)
        .filter(|summary| summary.process_group_id == interrupted_process_group_id)
        .filter(|summary| !descendants.contains(&summary.pid))
        .collect()
}

fn tty_attached_processes(shell_pid: i32) -> Vec<ProcSummary> {
    let tty_path = PathBuf::from("/proc")
        .join(shell_pid.to_string())
        .join("fd")
        .join("0");
    let Ok(shell_tty_target) = fs::read_link(tty_path) else {
        return Vec::new();
    };

    let mut attached = Vec::new();
    let Ok(proc_entries) = fs::read_dir("/proc") else {
        return Vec::new();
    };

    for entry in proc_entries.flatten() {
        let Ok(file_name) = entry.file_name().into_string() else {
            continue;
        };
        let Ok(pid) = file_name.parse::<i32>() else {
            continue;
        };
        let fd0_path = entry.path().join("fd").join("0");
        let Ok(target) = fs::read_link(fd0_path) else {
            continue;
        };
        if target != shell_tty_target {
            continue;
        }
        if let Some(summary) = describe_process(pid) {
            attached.push(summary);
        }
    }

    attached.sort();
    attached
}

fn describe_process(pid: i32) -> Option<ProcSummary> {
    let proc_dir = PathBuf::from("/proc").join(pid.to_string());
    let stat = fs::read_to_string(proc_dir.join("stat")).ok()?;
    let close_paren_index = stat.rfind(") ")?;
    let remainder = stat.get(close_paren_index + 2..)?;
    let mut fields = remainder.split_whitespace();
    let _state = fields.next()?;
    let _parent_pid = fields.next()?;
    let process_group_id = fields.next()?.parse().ok()?;
    Some(ProcSummary {
        pid,
        process_group_id,
    })
}

pub fn is_pty_eio(err: &io::Error) -> bool {
    err.raw_os_error() == Some(EIO)
}

fn io_error_from_errno() -> io::Error {
    let err = io::Error::last_os_error();
    match err.raw_os_error() {
        Some(EINTR) => io::Error::from(io::ErrorKind::Interrupted),
        Some(EAGAIN) => io::Error::from(io::ErrorKind::WouldBlock),
        _ => err,
    }
}

#[allow(dead_code)]
pub fn c_str(ptr: *const c_char) -> String {
    unsafe { CStr::from_ptr(ptr) }
        .to_string_lossy()
        .into_owned()
}
