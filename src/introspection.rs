use std::os::fd::RawFd;
use std::path::PathBuf;

use crate::error::ProcessError;
use crate::os::linux;

pub fn foreground_process_group(fd: RawFd) -> Result<i32, ProcessError> {
    linux::foreground_process_group(fd).map_err(|err| ProcessError::current_dir("tcgetpgrp", err))
}

pub fn current_dir(fd: RawFd) -> Result<PathBuf, ProcessError> {
    linux::current_dir(fd).map_err(|err| ProcessError::current_dir("current_dir", err))
}
