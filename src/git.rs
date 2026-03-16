use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context, Result, anyhow, bail};

use crate::manifest::{
    DependencySpec, MANIFEST_FILE, PackageRole, load_dependency_from_dir, load_from_dir,
    write_manifest,
};

pub const DEPS_ROOT: &str = ".agen/deps";

#[derive(Debug, Clone)]
pub struct GitCheckout {
    pub path: PathBuf,
    pub url: String,
    pub tag: String,
    pub rev: String,
}

pub fn add_dependency(url: &str, tag: &str) -> Result<()> {
    let cwd = std::env::current_dir().context("failed to determine the current directory")?;
    add_dependency_in_dir(&cwd, url, tag)
}

pub fn add_dependency_in_dir(project_root: &Path, url: &str, tag: &str) -> Result<()> {
    let alias = normalize_alias_from_url(url)?;
    let checkout = ensure_git_dependency(project_root, &alias, url, tag, true)?;
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
            url: Some(url.to_string()),
            path: None,
            tag: Some(tag.to_string()),
        },
    );

    write_manifest(&project_root.join(MANIFEST_FILE), &root.manifest)
}

pub fn ensure_git_dependency(
    project_root: &Path,
    alias: &str,
    url: &str,
    tag: &str,
    allow_network: bool,
) -> Result<GitCheckout> {
    let deps_root = project_root.join(DEPS_ROOT);
    fs::create_dir_all(&deps_root)
        .with_context(|| format!("failed to create {}", deps_root.display()))?;
    let checkout_path = deps_root.join(alias);

    if checkout_path.exists() {
        let remote_url = git_output(&checkout_path, ["remote", "get-url", "origin"])?;
        if remote_url.trim() != url {
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
            ["clone", url, checkout_path.to_string_lossy().as_ref()],
        )?;
    }

    let rev = resolve_tag_to_rev(&checkout_path, tag)?;
    if allow_network {
        git_run(&checkout_path, ["checkout", "--detach", &rev])?;
    }

    Ok(GitCheckout {
        path: checkout_path,
        url: url.to_string(),
        tag: tag.to_string(),
        rev,
    })
}

pub fn current_rev(path: &Path) -> Result<String> {
    git_output(path, ["rev-parse", "HEAD"])
}

pub fn resolve_tag_to_rev(path: &Path, tag: &str) -> Result<String> {
    git_output(path, ["rev-parse", &format!("{tag}^{{commit}}")])
}

pub fn normalize_alias_from_url(url: &str) -> Result<String> {
    let trimmed = url.trim_end_matches('/').trim_end_matches(".git");
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
    }
}
