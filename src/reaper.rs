use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};

use crate::error::ProcessError;
use crate::os::linux;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ChildState {
    Running,
    Exited(i32),
    Signaled(i32),
    Stopped(i32),
}

fn reaped_states() -> &'static Mutex<HashMap<i32, ChildState>> {
    static REAPED: OnceLock<Mutex<HashMap<i32, ChildState>>> = OnceLock::new();
    REAPED.get_or_init(|| Mutex::new(HashMap::new()))
}

fn reap_all_available_children() -> Result<(), ProcessError> {
    loop {
        match linux::reap_any_child_nonblocking()
            .map_err(|err| ProcessError::signal("waitpid(WAIT_ANY)", err))?
        {
            Some((pid, ChildState::Stopped(signal))) => {
                linux::resume_process_group_if_needed(pid, signal)
                    .map_err(|err| ProcessError::signal("resume stopped child", err))?;
                reaped_states()
                    .lock()
                    .expect("reaped child state map poisoned")
                    .insert(pid, ChildState::Stopped(signal));
            }
            Some((pid, state)) => {
                reaped_states()
                    .lock()
                    .expect("reaped child state map poisoned")
                    .insert(pid, state);
            }
            None => return Ok(()),
        }
    }
}

pub struct Reaper {
    pid: i32,
    state: ChildState,
}

impl Reaper {
    pub fn new(pid: i32) -> Self {
        Self {
            pid,
            state: ChildState::Running,
        }
    }

    pub fn pid(&self) -> i32 {
        self.pid
    }

    pub fn state(&self) -> ChildState {
        self.state
    }

    pub fn refresh(&mut self) -> Result<ChildState, ProcessError> {
        if !matches!(self.state, ChildState::Running) {
            return Ok(self.state);
        }

        reap_all_available_children()?;

        if let Some(state) = reaped_states()
            .lock()
            .expect("reaped child state map poisoned")
            .get(&self.pid)
            .copied()
        {
            self.state = state;
        }

        Ok(self.state)
    }
}
