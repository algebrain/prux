use std::io;
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd, RawFd};

use crate::error::ProcessError;
use crate::os::linux;

pub struct PtyMaster {
    fd: OwnedFd,
}

impl PtyMaster {
    pub(crate) unsafe fn from_raw_fd(fd: RawFd) -> Self {
        Self {
            fd: unsafe { OwnedFd::from_raw_fd(fd) },
        }
    }

    pub fn as_raw_fd(&self) -> RawFd {
        self.fd.as_raw_fd()
    }

    pub fn resize(&self, cols: u16, rows: u16) -> Result<(), ProcessError> {
        linux::set_winsize(self.as_raw_fd(), cols, rows)
            .map_err(|err| ProcessError::io("set_winsize", err))
    }

    pub fn write_all(&self, bytes: &[u8]) -> Result<(), ProcessError> {
        let mut written = 0usize;
        while written < bytes.len() {
            match linux::write(self.as_raw_fd(), &bytes[written..]) {
                Ok(0) => {
                    return Err(ProcessError::Io(
                        "write_all: PTY closed while writing".to_string(),
                    ));
                }
                Ok(n) => written += n,
                Err(err) if err.kind() == io::ErrorKind::Interrupted => continue,
                Err(err) if err.kind() == io::ErrorKind::WouldBlock => {
                    linux::wait_writable(self.as_raw_fd())
                        .map_err(|wait_err| ProcessError::io("poll writable", wait_err))?;
                }
                Err(err) => return Err(ProcessError::io("write", err)),
            }
        }
        Ok(())
    }

    pub fn try_read(&self) -> Result<Vec<u8>, ProcessError> {
        let mut out = Vec::new();
        let mut buf = [0u8; 4096];
        loop {
            match linux::read(self.as_raw_fd(), &mut buf) {
                Ok(0) => break,
                Ok(n) => out.extend_from_slice(&buf[..n]),
                Err(err) if err.kind() == io::ErrorKind::Interrupted => continue,
                Err(err) if err.kind() == io::ErrorKind::WouldBlock => break,
                Err(err) if linux::is_pty_eio(&err) => break,
                Err(err) => return Err(ProcessError::io("read", err)),
            }
        }
        Ok(out)
    }
}
