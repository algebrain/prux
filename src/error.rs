use std::fmt;
use std::io;

#[derive(Debug)]
pub enum ProcessError {
    Spawn(String),
    Io(String),
    Signal(String),
    CurrentDir(String),
    Unsupported(String),
}

impl ProcessError {
    pub(crate) fn spawn_io(context: &str, err: io::Error) -> Self {
        Self::Spawn(format!("{context}: {err}"))
    }

    pub(crate) fn io(context: &str, err: io::Error) -> Self {
        Self::Io(format!("{context}: {err}"))
    }

    pub(crate) fn signal(context: &str, err: io::Error) -> Self {
        Self::Signal(format!("{context}: {err}"))
    }

    pub(crate) fn current_dir(context: &str, err: io::Error) -> Self {
        Self::CurrentDir(format!("{context}: {err}"))
    }
}

impl fmt::Display for ProcessError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Spawn(message)
            | Self::Io(message)
            | Self::Signal(message)
            | Self::CurrentDir(message)
            | Self::Unsupported(message) => f.write_str(message),
        }
    }
}

impl std::error::Error for ProcessError {}
