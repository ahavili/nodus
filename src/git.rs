use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context, Result, anyhow, bail};

use crate::manifest::{
    DependencySpec, MANIFEST_FILE, PackageRole, load_dependency_from_dir, load_from_dir,
    write_manifest,
};
use crate::resolver::sync_in_dir;

pub const DEPS_ROOT: &str = ".agen/deps";

#[derive(Debug, Clone)]
pub struct GitCheckout {
    pub path: PathBuf,
    pub url: String,
    pub tag: String,
    pub rev: String,
}

pub fn add_dependency(url: &str, tag: Option<&str>) -> Result<()> {
    let cwd = std::env::current_dir().context("failed to determine the current directory")?;
    add_dependency_in_dir(&cwd, url, tag)
}

pub fn add_dependency_in_dir(project_root: &Path, url: &str, tag: Option<&str>) -> Result<()> {
    let normalized_url = normalize_git_url(url);
    let alias = normalize_alias_from_url(&normalized_url)?;
    let checkout = ensure_git_dependency(project_root, &alias, &normalized_url, tag, true)?;
    load_dependency_from_dir(&checkout.path)
        .with_context(|| format!("dependency `{alias}` does not match the Agen package layout"))?;

    let mut root = load_from_dir(project_root, PackageRole::Root)?;
    if root.manifest.dependencies.contains_key(&alias) {
        bail!(
            "dependency `{alias}` already exists in {}",
            project_root.display()
        );
    }
    root.manifest.dependencies.insert(
        alias,
        DependencySpec {
            url: Some(checkout.url.clone()),
            path: None,
            tag: Some(checkout.tag.clone()),
        },
    );

    write_manifest(&project_root.join(MANIFEST_FILE), &root.manifest)?;
    sync_in_dir(project_root, false, false)
}

pub fn ensure_git_dependency(
    project_root: &Path,
    alias: &str,
    url: &str,
    tag: Option<&str>,
    allow_network: bool,
) -> Result<GitCheckout> {
    let normalized_url = normalize_git_url(url);
    let deps_root = project_root.join(DEPS_ROOT);
    fs::create_dir_all(&deps_root)
        .with_context(|| format!("failed to create {}", deps_root.display()))?;
    let checkout_path = deps_root.join(alias);

    if checkout_path.exists() {
        let remote_url = git_output(&checkout_path, ["remote", "get-url", "origin"])?;
        if remote_url.trim() != normalized_url {
            bail!(
                "dependency `{alias}` already exists at {} with remote `{}`",
                checkout_path.display(),
                remote_url.trim()
            );
        }
        if allow_network {
            git_run(&checkout_path, ["fetch", "--tags", "origin"])?;
        }
    } else {
        if !allow_network {
            bail!(
                "missing git dependency `{alias}` at {}",
                checkout_path.display()
            );
        }
        git_run(
            project_root,
            [
                "clone",
                &normalized_url,
                checkout_path.to_string_lossy().as_ref(),
            ],
        )?;
    }

    let resolved_tag = match tag {
        Some(value) => value.to_string(),
        None => latest_tag(&checkout_path)?,
    };
    let rev = resolve_tag_to_rev(&checkout_path, &resolved_tag)?;
    if allow_network {
        git_run(&checkout_path, ["checkout", "--detach", &rev])?;
    }

    Ok(GitCheckout {
        path: checkout_path,
        url: normalized_url,
        tag: resolved_tag,
        rev,
    })
}

pub fn current_rev(path: &Path) -> Result<String> {
    git_output(path, ["rev-parse", "HEAD"])
}

pub fn resolve_tag_to_rev(path: &Path, tag: &str) -> Result<String> {
    git_output(path, ["rev-parse", &format!("{tag}^{{commit}}")])
}

pub fn latest_tag(path: &Path) -> Result<String> {
    let tags = git_output(
        path,
        [
            "for-each-ref",
            "--sort=-v:refname",
            "--format=%(refname:strip=2)",
            "refs/tags",
        ],
    )?;
    tags.lines()
        .find(|line| !line.trim().is_empty())
        .map(|line| line.trim().to_string())
        .ok_or_else(|| anyhow!("no git tags found in {}", path.display()))
}

pub fn normalize_git_url(url: &str) -> String {
    let trimmed = url.trim();
    if trimmed.starts_with("http://")
        || trimmed.starts_with("https://")
        || trimmed.starts_with("git@")
        || trimmed.starts_with("ssh://")
        || trimmed.starts_with('/')
        || trimmed.starts_with("./")
        || trimmed.starts_with("../")
    {
        return trimmed.to_string();
    }

    if let Some((owner, repo)) = trimmed.split_once('/')
        && !owner.is_empty()
        && !repo.is_empty()
        && !repo.contains('/')
    {
        return format!("https://github.com/{owner}/{repo}");
    }

    trimmed.to_string()
}

pub fn normalize_alias_from_url(url: &str) -> Result<String> {
    let normalized = normalize_git_url(url);
    let trimmed = normalized.trim_end_matches('/').trim_end_matches(".git");
    let tail = trimmed
        .rsplit('/')
        .next()
        .filter(|value| !value.is_empty())
        .ok_or_else(|| anyhow!("failed to infer a dependency alias from `{url}`"))?;

    let mut alias = String::new();
    for character in tail.chars() {
        if character.is_ascii_alphanumeric() {
            alias.push(character.to_ascii_lowercase());
        } else if !alias.ends_with('_') {
            alias.push('_');
        }
    }

    let alias = alias.trim_matches('_').to_string();
    if alias.is_empty() {
        bail!("failed to derive a valid dependency alias from `{url}`");
    }
    Ok(alias)
}

fn git_run<const N: usize>(cwd: &Path, args: [&str; N]) -> Result<()> {
    let output = Command::new("git")
        .args(args)
        .current_dir(cwd)
        .output()
        .with_context(|| format!("failed to run git in {}", cwd.display()))?;
    if !output.status.success() {
        bail!(
            "git {:?} failed in {}: {}",
            args,
            cwd.display(),
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    Ok(())
}

fn git_output<const N: usize>(cwd: &Path, args: [&str; N]) -> Result<String> {
    let output = Command::new("git")
        .args(args)
        .current_dir(cwd)
        .output()
        .with_context(|| format!("failed to run git in {}", cwd.display()))?;
    if !output.status.success() {
        bail!(
            "git {:?} failed in {}: {}",
            args,
            cwd.display(),
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::io::Write;
    use std::process::Command;

    use tempfile::TempDir;

    fn write_file(path: &Path, contents: &str) {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).unwrap();
        }
        let mut file = fs::File::create(path).unwrap();
        file.write_all(contents.as_bytes()).unwrap();
    }

    fn init_git_repo(path: &Path) {
        let run = |args: &[&str]| {
            let output = Command::new("git")
                .args(args)
                .current_dir(path)
                .output()
                .unwrap();
            assert!(
                output.status.success(),
                "{}",
                String::from_utf8_lossy(&output.stderr)
            );
        };

        run(&["init"]);
        run(&["config", "user.email", "test@example.com"]);
        run(&["config", "user.name", "Test User"]);
        run(&["add", "."]);
        run(&["commit", "-m", "initial"]);
    }

    #[test]
    fn normalizes_repo_names_into_aliases() {
        assert_eq!(
            normalize_alias_from_url("https://github.com/wenext-limited/playbook-ios").unwrap(),
            "playbook_ios"
        );
        assert_eq!(
            normalize_alias_from_url("git@github.com:foo/bar_baz.git").unwrap(),
            "bar_baz"
        );
        assert_eq!(
            normalize_alias_from_url("wenext-limited/playbook-ios").unwrap(),
            "playbook_ios"
        );
    }

    #[test]
    fn expands_github_shortcuts() {
        assert_eq!(
            normalize_git_url("wenext-limited/playbook-ios"),
            "https://github.com/wenext-limited/playbook-ios"
        );
        assert_eq!(
            normalize_git_url("https://github.com/wenext-limited/playbook-ios"),
            "https://github.com/wenext-limited/playbook-ios"
        );
    }

    #[test]
    fn picks_latest_tag_by_version_sort() {
        let temp = TempDir::new().unwrap();
        write_file(&temp.path().join("README.md"), "hello\n");
        init_git_repo(temp.path());

        for tag in ["v0.1.0", "v1.2.0", "v0.9.0"] {
            let output = Command::new("git")
                .args(["tag", tag])
                .current_dir(temp.path())
                .output()
                .unwrap();
            assert!(
                output.status.success(),
                "{}",
                String::from_utf8_lossy(&output.stderr)
            );
        }

        assert_eq!(latest_tag(temp.path()).unwrap(), "v1.2.0");
    }
}
