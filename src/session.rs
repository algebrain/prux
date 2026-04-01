use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::thread;
use std::time::{Duration, Instant};

use crate::error::ProcessError;
use crate::introspection;
use crate::pty::PtyMaster;
use crate::reaper::{ChildState, Reaper};
use crate::spawn::{spawn_process, SpawnConfig, SpawnMode};

#[derive(Debug, Clone)]
pub struct ProcessSessionConfig {
    pub program: PathBuf,
    pub args: Vec<String>,
    pub initial_cwd: PathBuf,
    pub debug_log_path: Option<PathBuf>,
    pub env: BTreeMap<String, String>,
}

pub struct ProcessSession {
    master: PtyMaster,
    reaper: Reaper,
}

impl ProcessSession {
    pub fn spawn(config: ProcessSessionConfig) -> Result<Self, ProcessError> {
        let spawn = spawn_process(SpawnConfig {
            program: config.program,
            args: config.args,
            cwd: config.initial_cwd,
            env: config.env,
            termios_template: crate::os::linux::capture_stdin_termios().ok(),
            mode: SpawnMode::ExplicitArgv,
        })?;

        Ok(Self {
            master: spawn.master,
            reaper: Reaper::new(spawn.child_pid),
        })
    }

    pub fn pid(&self) -> i32 {
        self.reaper.pid()
    }

    pub fn write_all(&mut self, bytes: &[u8]) -> Result<(), ProcessError> {
        self.master.write_all(bytes)
    }

    pub fn try_read(&mut self) -> Result<Vec<u8>, ProcessError> {
        let _ = self.reaper.refresh()?;
        self.master.try_read()
    }

    pub fn send_interrupt(&mut self) -> Result<(), ProcessError> {
        let shell_process_group = self.reaper.pid();
        let interrupted_process_group =
            introspection::foreground_process_group(self.master.as_raw_fd())
                .ok()
                .filter(|pgid| *pgid != shell_process_group);
        let intr = crate::os::linux::interrupt_byte(self.master.as_raw_fd())
            .unwrap_or(crate::os::linux::DEFAULT_INTR_BYTE);
        self.master
            .write_all(&[intr])
            .map_err(|err| ProcessError::Signal(err.to_string()))?;

        if let Some(interrupted_process_group) = interrupted_process_group {
            let _ = self.cleanup_lingering_tty_processes_after_interrupt(interrupted_process_group);
        }

        Ok(())
    }

    pub fn current_dir(&self) -> Result<PathBuf, ProcessError> {
        let deadline = Instant::now() + Duration::from_millis(250);
        loop {
            match introspection::current_dir(self.master.as_raw_fd()) {
                Ok(path) => return Ok(path),
                Err(err) if Instant::now() < deadline => {
                    if !matches!(err, ProcessError::CurrentDir(_)) {
                        return Err(err);
                    }
                    thread::sleep(Duration::from_millis(10));
                }
                Err(err) => return Err(err),
            }
        }
    }

    pub fn resize(&mut self, cols: u16, rows: u16) -> Result<(), ProcessError> {
        self.master.resize(cols, rows)
    }

    pub fn is_alive(&mut self) -> Result<bool, ProcessError> {
        Ok(matches!(
            self.reaper.refresh()?,
            ChildState::Running | ChildState::Stopped(_)
        ))
    }

    pub fn terminate(&mut self) -> Result<(), ProcessError> {
        if !self.is_alive()? {
            return Ok(());
        }

        let _ =
            crate::os::linux::send_signal_to_group(self.reaper.pid(), crate::os::linux::SIGTERM)
                .or_else(|_| {
                    crate::os::linux::send_signal(self.reaper.pid(), crate::os::linux::SIGTERM)
                });

        let deadline = Instant::now() + Duration::from_secs(2);
        while Instant::now() < deadline {
            if !self.is_alive()? {
                return Ok(());
            }
            thread::sleep(Duration::from_millis(20));
        }

        let _ =
            crate::os::linux::send_signal_to_group(self.reaper.pid(), crate::os::linux::SIGKILL)
                .or_else(|_| {
                    crate::os::linux::send_signal(self.reaper.pid(), crate::os::linux::SIGKILL)
                });

        let kill_deadline = Instant::now() + Duration::from_secs(1);
        while Instant::now() < kill_deadline {
            if !self.is_alive()? {
                return Ok(());
            }
            thread::sleep(Duration::from_millis(10));
        }

        Err(ProcessError::Signal(
            "terminate: process did not exit after SIGTERM/SIGKILL".to_string(),
        ))
    }

    fn cleanup_lingering_tty_processes_after_interrupt(
        &mut self,
        interrupted_process_group: i32,
    ) -> Result<(), ProcessError> {
        let shell_process_group = self.reaper.pid();
        for _ in 0..6 {
            thread::sleep(Duration::from_millis(25));

            let shell_is_foreground =
                introspection::foreground_process_group(self.master.as_raw_fd()).ok()
                    == Some(shell_process_group);
            let descendants = crate::os::linux::descendant_pids(self.reaper.pid());
            let lingering = crate::os::linux::lingering_tty_processes_for_interrupted_group(
                self.reaper.pid(),
                shell_process_group,
                interrupted_process_group,
                &descendants,
            );

            if !shell_is_foreground || lingering.is_empty() {
                continue;
            }

            let _ = crate::os::linux::send_signal_to_group(
                interrupted_process_group,
                crate::os::linux::SIGHUP_SIGNAL,
            );
            let _ = crate::os::linux::send_signal_to_group(
                interrupted_process_group,
                crate::os::linux::SIGCONT_SIGNAL,
            );
            thread::sleep(Duration::from_millis(50));

            let descendants = crate::os::linux::descendant_pids(self.reaper.pid());
            let still_lingering = crate::os::linux::lingering_tty_processes_for_interrupted_group(
                self.reaper.pid(),
                shell_process_group,
                interrupted_process_group,
                &descendants,
            );

            if still_lingering.is_empty() {
                return Ok(());
            }

            let _ = crate::os::linux::send_signal_to_group(
                interrupted_process_group,
                crate::os::linux::SIGTERM,
            );
            return Ok(());
        }

        Ok(())
    }
}

impl Drop for ProcessSession {
    fn drop(&mut self) {
        let _ = self.terminate();
    }
}

pub fn default_shell_config(cwd: impl AsRef<Path>) -> ProcessSessionConfig {
    ProcessSessionConfig {
        program: PathBuf::from("/bin/sh"),
        args: Vec::new(),
        initial_cwd: cwd.as_ref().to_path_buf(),
        debug_log_path: None,
        env: BTreeMap::new(),
    }
}
