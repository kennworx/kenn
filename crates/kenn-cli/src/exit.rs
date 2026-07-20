//! Stable exit codes per `index-store-cli` spec.

use std::process::ExitCode;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExitCodes {
    Ok = 0,
    Generic = 1,
    Usage = 2,
    NoWorkspace = 3,
    LockHeld = 4,
    IndexerFailed = 5,
}

impl From<ExitCodes> for ExitCode {
    fn from(c: ExitCodes) -> Self {
        Self::from(c as u8)
    }
}
