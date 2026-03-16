use std::env;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};

pub const APP_CACHE_DIR: &str = "nodus";

pub fn resolve_cache_root(override_path: Option<&Path>) -> Result<PathBuf> {
    match override_path {
        Some(path) => absolutize(path),
        None => default_cache_root(),
    }
}

fn absolutize(path: &Path) -> Result<PathBuf> {
    if path.is_absolute() {
        return Ok(path.to_path_buf());
    }

    let cwd = env::current_dir().context("failed to determine the current directory")?;
    Ok(cwd.join(path))
}

fn default_cache_root() -> Result<PathBuf> {
    #[cfg(target_os = "macos")]
    {
        let home = env::var_os("HOME").ok_or_else(|| {
            anyhow::anyhow!("failed to determine the home directory for the default cache path")
        })?;
        Ok(PathBuf::from(home)
            .join("Library")
            .join("Caches")
            .join(APP_CACHE_DIR))
    }

    #[cfg(target_os = "windows")]
    {
        if let Some(local_app_data) = env::var_os("LOCALAPPDATA") {
            return Ok(PathBuf::from(local_app_data).join(APP_CACHE_DIR));
        }
        if let Some(app_data) = env::var_os("APPDATA") {
            return Ok(PathBuf::from(app_data).join(APP_CACHE_DIR));
        }
        bail!("failed to determine the default cache path on Windows");
    }

    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    {
        if let Some(xdg_cache_home) = env::var_os("XDG_CACHE_HOME") {
            return Ok(PathBuf::from(xdg_cache_home).join(APP_CACHE_DIR));
        }
        let home = env::var_os("HOME").ok_or_else(|| {
            anyhow::anyhow!("failed to determine the home directory for the default cache path")
        })?;
        Ok(PathBuf::from(home).join(".cache").join(APP_CACHE_DIR))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolves_relative_overrides_from_the_current_directory() {
        let original_cwd = env::current_dir().unwrap();
        let temp = tempfile::TempDir::new().unwrap();
        env::set_current_dir(temp.path()).unwrap();

        let resolved = resolve_cache_root(Some(Path::new("cache-root"))).unwrap();

        env::set_current_dir(original_cwd).unwrap();
        assert_eq!(resolved.file_name().unwrap(), "cache-root");
        assert_eq!(
            resolved.parent().unwrap().canonicalize().unwrap(),
            temp.path().canonicalize().unwrap()
        );
    }
}
