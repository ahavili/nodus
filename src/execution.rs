use std::path::PathBuf;

use crate::paths::display_path;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExecutionMode {
    Apply,
    DryRun,
}

impl ExecutionMode {
    pub const fn is_dry_run(self) -> bool {
        matches!(self, Self::DryRun)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PreviewChange {
    Create(PathBuf),
    Write(PathBuf),
    Remove(PathBuf),
    Relay(PathBuf),
    PersistLocalConfig(PathBuf),
}

impl PreviewChange {
    pub fn describe(&self) -> String {
        match self {
            Self::Create(path) => format!("would create {}", display_path(path)),
            Self::Write(path) => format!("would write {}", display_path(path)),
            Self::Remove(path) => format!("would remove {}", display_path(path)),
            Self::Relay(path) => format!("would relay {}", display_path(path)),
            Self::PersistLocalConfig(path) => {
                format!("would persist local config {}", display_path(path))
            }
        }
    }
}
