use std::env;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};

pub const APP_STORAGE_DIR: &str = "nodus";

pub fn resolve_store_root(override_path: Option<&Path>) -> Result<PathBuf> {
    match override_path {
        Some(path) => absolutize(path),
        None => default_store_root(),
    }
}

fn absolutize(path: &Path) -> Result<PathBuf> {
    if path.is_absolute() {
        return Ok(path.to_path_buf());
    }

    let cwd = env::current_dir().context("failed to determine the current directory")?;
    Ok(cwd.join(path))
}

fn default_store_root() -> Result<PathBuf> {
    #[cfg(target_os = "macos")]
    {
        let home = env::var_os("HOME").ok_or_else(|| {
            anyhow::anyhow!("failed to determine the home directory for the default storage path")
        })?;
        Ok(PathBuf::from(home)
            .join("Library")
            .join("Application Support")
            .join(APP_STORAGE_DIR))
    }

    #[cfg(target_os = "windows")]
    {
        if let Some(local_app_data) = env::var_os("LOCALAPPDATA") {
            return Ok(PathBuf::from(local_app_data).join(APP_STORAGE_DIR));
        }
        if let Some(app_data) = env::var_os("APPDATA") {
            return Ok(PathBuf::from(app_data).join(APP_STORAGE_DIR));
        }
        anyhow::bail!("failed to determine the default local application data path on Windows");
    }

    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    {
        let home = env::var_os("HOME").ok_or_else(|| {
            anyhow::anyhow!("failed to determine the home directory for the default storage path")
        })?;
        Ok(default_unix_store_root(
            Path::new(&home),
            env::var_os("XDG_STATE_HOME").as_deref().map(Path::new),
        ))
    }
}

#[cfg(not(any(target_os = "macos", target_os = "windows")))]
fn default_unix_store_root(home: &Path, xdg_state_home: Option<&Path>) -> PathBuf {
    if let Some(path) = xdg_state_home.filter(|path| path.is_absolute()) {
        return path.join(APP_STORAGE_DIR);
    }

    home.join(".local").join("state").join(APP_STORAGE_DIR)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolves_relative_overrides_from_the_current_directory() {
        let original_cwd = env::current_dir().unwrap();
        let temp = tempfile::TempDir::new().unwrap();
        env::set_current_dir(temp.path()).unwrap();

        let resolved = resolve_store_root(Some(Path::new("store-root"))).unwrap();

        env::set_current_dir(original_cwd).unwrap();
        assert_eq!(resolved.file_name().unwrap(), "store-root");
        assert_eq!(
            resolved.parent().unwrap().canonicalize().unwrap(),
            temp.path().canonicalize().unwrap()
        );
    }

    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    #[test]
    fn prefers_absolute_xdg_state_home_over_the_home_fallback() {
        let resolved = default_unix_store_root(
            Path::new("/home/tester"),
            Some(Path::new("/var/lib/tester-state")),
        );

        assert_eq!(resolved, PathBuf::from("/var/lib/tester-state/nodus"));
    }

    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    #[test]
    fn ignores_relative_xdg_state_home_values() {
        let resolved =
            default_unix_store_root(Path::new("/home/tester"), Some(Path::new("relative-state")));

        assert_eq!(resolved, PathBuf::from("/home/tester/.local/state/nodus"));
    }
}
