//! prux: PTY/process management primitives for Unix-like systems.

#[cfg(not(target_os = "linux"))]
compile_error!("prux currently supports Linux only.");

pub mod error;
pub mod introspection;
pub mod pty;
pub mod reaper;
pub mod session;
pub mod spawn;

pub mod os {
    pub mod linux;
}

pub use error::ProcessError;
pub use session::{ProcessSession, ProcessSessionConfig};
