use std::collections::BTreeMap;
use std::env;
use std::ffi::CString;
use std::io;
use std::os::raw::c_char;
use std::path::PathBuf;
use std::sync::{Mutex, OnceLock};

use crate::error::ProcessError;
use crate::os::linux;
use crate::pty::PtyMaster;

#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub enum SpawnMode {
    ExplicitArgv,
    ShellCommand,
    LoginShell,
}

pub struct SpawnConfig {
    pub program: PathBuf,
    pub args: Vec<String>,
    pub cwd: PathBuf,
    pub env: BTreeMap<String, String>,
    pub termios_template: Option<linux::SavedTermios>,
    pub mode: SpawnMode,
}

pub struct SpawnedProcess {
    pub master: PtyMaster,
    pub child_pid: i32,
}

fn spawn_critical_section() -> &'static Mutex<()> {
    static SPAWN_MUTEX: OnceLock<Mutex<()>> = OnceLock::new();
    SPAWN_MUTEX.get_or_init(|| Mutex::new(()))
}

pub fn spawn_process(config: SpawnConfig) -> Result<SpawnedProcess, ProcessError> {
    let _guard = spawn_critical_section()
        .lock()
        .expect("spawn critical section mutex poisoned");
    let program = cstring_from_path(&config.program)
        .map_err(|err| ProcessError::Spawn(format!("program path contains NUL: {err}")))?;
    let cwd = cstring_from_path(&config.cwd)
        .map_err(|err| ProcessError::Spawn(format!("cwd contains NUL: {err}")))?;
    let parent_cwd = env::current_dir().ok();

    let mut argv_cstrings = Vec::with_capacity(config.args.len() + 1);
    argv_cstrings.push(program.clone());
    for arg in &config.args {
        argv_cstrings.push(
            CString::new(arg.as_str())
                .map_err(|err| ProcessError::Spawn(format!("argument contains NUL: {err}")))?,
        );
    }
    let mut argv: Vec<*const c_char> = argv_cstrings.iter().map(|s| s.as_ptr()).collect();
    argv.push(std::ptr::null());

    let blocked = linux::block_all_signals()
        .map_err(|err| ProcessError::spawn_io("sigprocmask(SIG_BLOCK)", err))?;
    let actual_cwd = linux::change_dir_with_fallback(&cwd)
        .map_err(|err| ProcessError::spawn_io("prepare cwd", err))?;

    let env_cstrings = build_environment(&actual_cwd, &config.env)?;
    let mut envp: Vec<*const c_char> = env_cstrings.iter().map(|s| s.as_ptr()).collect();
    envp.push(std::ptr::null());

    let (child_pid, master_fd) =
        linux::fork_pty(80, 24).map_err(|err| ProcessError::spawn_io("forkpty", err))?;

    if child_pid == 0 {
        let _ = linux::restore_signal_mask(&blocked);
        let status = unsafe {
            child_exec(
                &config,
                program.as_ptr(),
                argv.as_ptr(),
                envp.as_ptr(),
                config.termios_template.as_ref(),
            )
        };
        linux::exit_immediately(status);
    }

    if let Some(parent_cwd) = parent_cwd.as_ref() {
        let _ = linux::restore_working_dir(parent_cwd);
    }
    linux::restore_signal_mask(&blocked)
        .map_err(|err| ProcessError::spawn_io("sigprocmask(SIG_SETMASK)", err))?;
    linux::set_nonblocking(master_fd)
        .map_err(|err| ProcessError::spawn_io("set_nonblocking", err))?;
    let master = unsafe { PtyMaster::from_raw_fd(master_fd) };
    Ok(SpawnedProcess { master, child_pid })
}

unsafe fn child_exec(
    config: &SpawnConfig,
    program: *const c_char,
    argv: *const *const c_char,
    envp: *const *const c_char,
    termios_template: Option<&linux::SavedTermios>,
) -> i32 {
    let _ = linux::reset_signal_state_for_child();
    let _ = linux::prepare_child_terminal(termios_template);
    linux::close_non_std_fds();
    match config.mode {
        SpawnMode::ExplicitArgv => {
            let _ = linux::execvpe(program, argv, envp);
        }
        SpawnMode::ShellCommand => {
            if config.args.len() != 1 {
                return 127;
            }
            let shell = linux::shell_path_from_env();
            let shell_c = match CString::new(shell.as_str()) {
                Ok(value) => value,
                Err(_) => return 127,
            };
            let argv0 = match linux::shell_argv0(&shell, false) {
                Ok(value) => value,
                Err(_) => return 127,
            };
            let dash_c = c"-c";
            let command = match CString::new(config.args[0].as_str()) {
                Ok(value) => value,
                Err(_) => return 127,
            };
            let argv_local = [
                argv0.as_ptr(),
                dash_c.as_ptr(),
                command.as_ptr(),
                std::ptr::null(),
            ];
            let _ = linux::execve(shell_c.as_ptr(), argv_local.as_ptr(), envp);
        }
        SpawnMode::LoginShell => {
            let shell = linux::shell_path_from_env();
            let shell_c = match CString::new(shell.as_str()) {
                Ok(value) => value,
                Err(_) => return 127,
            };
            let argv0 = match linux::shell_argv0(&shell, true) {
                Ok(value) => value,
                Err(_) => return 127,
            };
            let argv_local = [argv0.as_ptr(), std::ptr::null()];
            let _ = linux::execve(shell_c.as_ptr(), argv_local.as_ptr(), envp);
        }
    }
    127
}

fn build_environment(
    cwd: &str,
    extra_env: &BTreeMap<String, String>,
) -> Result<Vec<CString>, ProcessError> {
    let mut env_map = BTreeMap::<String, String>::new();
    for (key, value) in env::vars() {
        env_map.insert(key, value);
    }
    for (key, value) in extra_env {
        env_map.insert(key.clone(), value.clone());
    }
    env_map.insert("PWD".to_string(), cwd.to_string());

    let mut envp = Vec::with_capacity(env_map.len());
    for (key, value) in env_map {
        envp.push(
            CString::new(format!("{key}={value}"))
                .map_err(|err| ProcessError::Spawn(format!("environment contains NUL: {err}")))?,
        );
    }
    Ok(envp)
}

fn cstring_from_path(path: &PathBuf) -> Result<CString, io::Error> {
    CString::new(path.as_os_str().as_encoded_bytes())
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "path contains NUL"))
}
