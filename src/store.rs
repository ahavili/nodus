use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use tempfile::NamedTempFile;

use crate::resolver::Resolution;

pub const STORE_ROOT: &str = "store/sha256";

#[derive(Debug, Clone)]
pub struct StoredPackage {
    pub digest: String,
    pub snapshot_root: PathBuf,
}

pub fn snapshot_resolution(
    cache_root: &Path,
    resolution: &Resolution,
) -> Result<Vec<StoredPackage>> {
    let store_root = cache_root.join(STORE_ROOT);
    fs::create_dir_all(&store_root)
        .with_context(|| format!("failed to create store root {}", store_root.display()))?;

    let mut stored_packages = Vec::new();
    for package in &resolution.packages {
        let snapshot_root = snapshot_package(&store_root, resolution, package)?;
        stored_packages.push(StoredPackage {
            digest: package.digest.clone(),
            snapshot_root,
        });
    }
    Ok(stored_packages)
}

pub fn write_atomic(path: &Path, contents: &[u8]) -> Result<()> {
    let parent = path
        .parent()
        .ok_or_else(|| anyhow::anyhow!("cannot atomically write {}", path.display()))?;
    fs::create_dir_all(parent)
        .with_context(|| format!("failed to create parent directory {}", parent.display()))?;

    let mut temp = NamedTempFile::new_in(parent)
        .with_context(|| format!("failed to create temp file in {}", parent.display()))?;
    temp.write_all(contents)
        .with_context(|| format!("failed to write temp file for {}", path.display()))?;
    temp.flush()
        .with_context(|| format!("failed to flush temp file for {}", path.display()))?;
    temp.persist(path)
        .map_err(|error| error.error)
        .with_context(|| {
            format!(
                "failed to persist atomically written file to {}",
                path.display()
            )
        })?;

    Ok(())
}

fn snapshot_package(
    store_root: &Path,
    _resolution: &Resolution,
    package: &crate::resolver::ResolvedPackage,
) -> Result<PathBuf> {
    let digest_dir = store_root.join(digest_directory_name(&package.digest)?);
    let files = package.manifest.package_files()?;
    if digest_dir.exists() {
        if snapshot_is_complete(&digest_dir, &package.manifest.root, &files)? {
            return Ok(digest_dir);
        }

        fs::remove_dir_all(&digest_dir).with_context(|| {
            format!(
                "failed to remove incomplete snapshot {}",
                digest_dir.display()
            )
        })?;
    }

    if digest_dir.exists() {
        return Ok(digest_dir);
    }

    let staging_root = store_root.join(format!(
        ".tmp-{}",
        digest_directory_name(&package.digest)?.replace('/', "_")
    ));
    if staging_root.exists() {
        fs::remove_dir_all(&staging_root).with_context(|| {
            format!(
                "failed to clean stale staging dir {}",
                staging_root.display()
            )
        })?;
    }
    fs::create_dir_all(&staging_root)
        .with_context(|| format!("failed to create staging dir {}", staging_root.display()))?;

    for file in files {
        let relative = file.strip_prefix(&package.manifest.root).with_context(|| {
            format!("failed to make {} relative to package root", file.display())
        })?;
        let target = staging_root.join(relative);
        if let Some(parent) = target.parent() {
            fs::create_dir_all(parent).with_context(|| {
                format!("failed to create snapshot directory {}", parent.display())
            })?;
        }
        fs::copy(&file, &target).with_context(|| {
            format!(
                "failed to copy {} into snapshot {}",
                file.display(),
                target.display()
            )
        })?;
    }

    if let Some(parent) = digest_dir.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("failed to create store parent {}", parent.display()))?;
    }

    match fs::rename(&staging_root, &digest_dir) {
        Ok(()) => Ok(digest_dir),
        Err(error) if digest_dir.exists() => {
            fs::remove_dir_all(&staging_root).with_context(|| {
                format!(
                    "failed to remove redundant staging dir {} after {}",
                    staging_root.display(),
                    error
                )
            })?;
            Ok(digest_dir)
        }
        Err(error) => Err(error).with_context(|| {
            format!(
                "failed to promote snapshot {} into {}",
                staging_root.display(),
                digest_dir.display()
            )
        }),
    }
}

fn snapshot_is_complete(
    snapshot_root: &Path,
    package_root: &Path,
    files: &[PathBuf],
) -> Result<bool> {
    for file in files {
        let relative = file.strip_prefix(package_root).with_context(|| {
            format!("failed to make {} relative to package root", file.display())
        })?;
        if !snapshot_root.join(relative).is_file() {
            return Ok(false);
        }
    }

    Ok(true)
}

fn digest_directory_name(digest: &str) -> Result<&str> {
    digest
        .strip_prefix("sha256:")
        .ok_or_else(|| anyhow::anyhow!("unsupported digest format `{digest}`"))
}

#[cfg(test)]
mod tests {
    use std::io::Write;

    use tempfile::TempDir;

    use super::*;
    use crate::resolver::resolve_project_for_sync;

    fn write_file(path: &Path, contents: &str) {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).unwrap();
        }
        let mut file = fs::File::create(path).unwrap();
        file.write_all(contents.as_bytes()).unwrap();
    }

    #[test]
    fn snapshots_package_contents_into_the_local_store() {
        let temp = TempDir::new().unwrap();
        let cache = TempDir::new().unwrap();
        write_file(
            &temp.path().join("skills/review/SKILL.md"),
            "---\nname: Review\ndescription: Example.\n---\n# Review\n",
        );

        let resolution = resolve_project_for_sync(temp.path(), cache.path()).unwrap();
        let stored = snapshot_resolution(cache.path(), &resolution).unwrap();

        assert_eq!(stored.len(), 1);
        assert!(
            stored[0]
                .snapshot_root
                .starts_with(cache.path().join(STORE_ROOT))
        );
        assert!(!stored[0].snapshot_root.starts_with(temp.path()));
        assert!(
            stored[0]
                .snapshot_root
                .join("skills/review/SKILL.md")
                .exists()
        );
    }

    #[test]
    fn recreates_incomplete_snapshots() {
        let temp = TempDir::new().unwrap();
        let cache = TempDir::new().unwrap();
        write_file(
            &temp.path().join("skills/review/SKILL.md"),
            "---\nname: Review\ndescription: Example.\n---\n# Review\n",
        );
        write_file(
            &temp.path().join("rules/common/coding-style.md"),
            "be consistent\n",
        );

        let resolution = resolve_project_for_sync(temp.path(), cache.path()).unwrap();
        let stored = snapshot_resolution(cache.path(), &resolution).unwrap();
        let snapshot_root = &stored[0].snapshot_root;

        fs::remove_file(snapshot_root.join("rules/common/coding-style.md")).unwrap();
        let rebuilt = snapshot_resolution(cache.path(), &resolution).unwrap();

        assert_eq!(rebuilt[0].snapshot_root, *snapshot_root);
        assert!(
            rebuilt[0]
                .snapshot_root
                .join("rules/common/coding-style.md")
                .exists()
        );
    }

    #[test]
    fn atomically_writes_files() {
        let temp = TempDir::new().unwrap();
        let target = temp.path().join("nested/output.txt");

        write_atomic(&target, b"hello").unwrap();

        assert_eq!(fs::read_to_string(target).unwrap(), "hello");
    }
}
