use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, anyhow, bail};

use crate::adapters::{Adapter, Adapters, ArtifactKind, managed_artifact_path, managed_skill_root};
use crate::git::{
    git_urls_match, is_git_repository, normalize_git_url, repository_origin_url,
    resolve_dependency_alias,
};
use crate::local_config::{LocalConfig, RelayLink};
use crate::manifest::{DependencySourceKind, SkillEntry, load_root_from_dir};
use crate::report::Reporter;
use crate::resolver::{PackageSource, ResolvedPackage, resolve_project_from_current_lockfile_in_dir};
use crate::selection::resolve_adapter_selection;
use crate::store::snapshot_resolution;

#[derive(Debug, Clone)]
pub struct RelaySummary {
    pub alias: String,
    pub linked_repo: PathBuf,
    pub updated_file_count: usize,
}

#[derive(Debug, Clone)]
struct RelayWorkspace {
    root: crate::manifest::LoadedManifest,
    project_root: PathBuf,
    selected_adapters: Adapters,
    resolution: crate::resolver::Resolution,
    snapshot_roots: HashMap<String, PathBuf>,
    local_config: LocalConfig,
}

#[derive(Debug, Clone)]
struct DependencyContext {
    alias: String,
    url: String,
    package: ResolvedPackage,
    snapshot_root: PathBuf,
}

#[derive(Debug, Clone)]
struct RelayFileMapping {
    managed_path: PathBuf,
    snapshot_path: PathBuf,
    linked_source_path: PathBuf,
    transform: RelayTransform,
}

#[derive(Debug, Clone)]
enum RelayTransform {
    None,
    OpenCodeSkillName { managed_skill_id: String },
}

#[derive(Debug, Clone, Default)]
struct RelayPlan {
    updates: BTreeMap<PathBuf, Vec<u8>>,
    noops: BTreeSet<PathBuf>,
    conflicts: Vec<String>,
}

pub fn relay_dependency_in_dir(
    project_root: &Path,
    cache_root: &Path,
    package: &str,
    repo_path_override: Option<&Path>,
    reporter: &Reporter,
) -> Result<RelaySummary> {
    let mut workspace = load_workspace(project_root, cache_root, reporter)?;
    let dependency = dependency_context(&workspace, package)?;
    let linked_repo = resolve_linked_repo(
        project_root,
        &mut workspace.local_config,
        &dependency,
        repo_path_override,
    )?;

    let plan = build_relay_plan(&dependency, &workspace.project_root, workspace.selected_adapters, &linked_repo)?;
    if !plan.conflicts.is_empty() {
        bail!(
            "relay conflicts for `{}`:\n{}",
            dependency.alias,
            plan.conflicts.join("\n")
        );
    }

    reporter.status("Relaying", format!("managed edits for {}", dependency.alias))?;
    for (path, contents) in &plan.updates {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)
                .with_context(|| format!("failed to create {}", parent.display()))?;
        }
        crate::store::write_atomic(path, contents)
            .with_context(|| format!("failed to write relayed source {}", path.display()))?;
        reporter.note(format!("updated {}", display_relative(&linked_repo, path)))?;
    }
    for path in &plan.noops {
        reporter.note(format!(
            "{} already matches managed edits",
            display_relative(&linked_repo, path)
        ))?;
    }

    workspace.local_config.save_in_dir(project_root)?;

    Ok(RelaySummary {
        alias: dependency.alias,
        linked_repo,
        updated_file_count: plan.updates.len(),
    })
}

pub fn ensure_no_pending_relay_edits_in_dir(project_root: &Path, cache_root: &Path) -> Result<()> {
    let reporter = Reporter::silent();
    let workspace = load_workspace_if_linked(project_root, cache_root, &reporter)?;
    if workspace.local_config.relay.is_empty() {
        return Ok(());
    }

    let linked_aliases = workspace
        .root
        .manifest
        .dependencies
        .keys()
        .filter(|alias| workspace.local_config.relay.contains_key(*alias))
        .cloned()
        .collect::<Vec<_>>();
    if linked_aliases.is_empty() {
        return Ok(());
    }

    let mut blocked = Vec::new();
    for alias in linked_aliases {
        let dependency = dependency_context(&workspace, &alias)?;
        let linked = resolve_existing_link(&workspace.local_config, &dependency)?;
        let plan = build_relay_plan(
            &dependency,
            &workspace.project_root,
            workspace.selected_adapters,
            &linked,
        )?;
        if !plan.conflicts.is_empty() {
            blocked.push(format!(
                "{alias}: {}",
                plan.conflicts.join("; ")
            ));
            continue;
        }
        if !plan.updates.is_empty() {
            blocked.push(format!(
                "{alias}: {} pending relayed source files",
                plan.updates.len()
            ));
        }
    }

    if blocked.is_empty() {
        Ok(())
    } else {
        bail!(
            "pending relay edits would be overwritten:\n{}\nRun `nodus relay <dependency>` or discard the managed edits first.",
            blocked.join("\n")
        )
    }
}

fn load_workspace(project_root: &Path, cache_root: &Path, reporter: &Reporter) -> Result<RelayWorkspace> {
    let root = load_root_from_dir(project_root)?;
    let selection = resolve_adapter_selection(project_root, &root.manifest, &[], false)?;
    let selected_adapters = Adapters::from_slice(&selection.adapters);
    let local_config = LocalConfig::load_in_dir(project_root)?;
    let (resolution, _lockfile) = resolve_project_from_current_lockfile_in_dir(
        project_root,
        cache_root,
        selected_adapters,
        reporter,
    )?;
    let snapshot_roots = snapshot_resolution(cache_root, &resolution)?
        .into_iter()
        .map(|stored| (stored.digest, stored.snapshot_root))
        .collect::<HashMap<_, _>>();

    Ok(RelayWorkspace {
        root,
        project_root: project_root.to_path_buf(),
        selected_adapters,
        resolution,
        snapshot_roots,
        local_config,
    })
}

fn load_workspace_if_linked(
    project_root: &Path,
    cache_root: &Path,
    reporter: &Reporter,
) -> Result<RelayWorkspace> {
    let root = load_root_from_dir(project_root)?;
    let local_config = LocalConfig::load_in_dir(project_root)?;
    if local_config.relay.is_empty() {
        return Ok(RelayWorkspace {
            root,
            project_root: project_root.to_path_buf(),
            selected_adapters: Adapters::NONE,
            resolution: crate::resolver::Resolution {
                project_root: project_root.to_path_buf(),
                packages: Vec::new(),
                warnings: Vec::new(),
            },
            snapshot_roots: HashMap::new(),
            local_config,
        });
    }
    let selection = resolve_adapter_selection(project_root, &root.manifest, &[], false)?;
    let selected_adapters = Adapters::from_slice(&selection.adapters);

    let (resolution, _lockfile) = resolve_project_from_current_lockfile_in_dir(
        project_root,
        cache_root,
        selected_adapters,
        reporter,
    )?;
    let snapshot_roots = snapshot_resolution(cache_root, &resolution)?
        .into_iter()
        .map(|stored| (stored.digest, stored.snapshot_root))
        .collect::<HashMap<_, _>>();

    Ok(RelayWorkspace {
        root,
        project_root: project_root.to_path_buf(),
        selected_adapters,
        resolution,
        snapshot_roots,
        local_config,
    })
}

fn dependency_context(workspace: &RelayWorkspace, package: &str) -> Result<DependencyContext> {
    let alias = resolve_dependency_alias(&workspace.root.manifest.dependencies, package)?;
    let spec = workspace
        .root
        .manifest
        .dependencies
        .get(&alias)
        .ok_or_else(|| anyhow!("dependency `{alias}` does not exist"))?;
    if spec.source_kind()? != DependencySourceKind::Git {
        bail!("relay supports direct git dependencies only; `{alias}` is a path dependency");
    }
    let url = normalize_git_url(&spec.resolved_git_url()?);

    let package = workspace
        .resolution
        .packages
        .iter()
        .find(|resolved| {
            resolved.alias == alias
                && matches!(
                    &resolved.source,
                    PackageSource::Git { url: resolved_url, .. } if normalize_git_url(resolved_url) == url
                )
        })
        .cloned()
        .ok_or_else(|| anyhow!("dependency `{alias}` is missing from the current lockfile state"))?;
    let snapshot_root = workspace
        .snapshot_roots
        .get(&package.digest)
        .cloned()
        .ok_or_else(|| anyhow!("missing snapshot for dependency `{alias}`"))?;

    Ok(DependencyContext {
        alias,
        url,
        package,
        snapshot_root,
    })
}

fn resolve_linked_repo(
    project_root: &Path,
    local_config: &mut LocalConfig,
    dependency: &DependencyContext,
    repo_path_override: Option<&Path>,
) -> Result<PathBuf> {
    match repo_path_override {
        Some(path) => {
            let linked_repo = canonicalize_existing_dir(path)?;
            validate_linked_repo(&linked_repo, &dependency.url)?;
            local_config.set_relay_link(
                dependency.alias.clone(),
                RelayLink {
                    repo_path: linked_repo.clone(),
                    url: dependency.url.clone(),
                },
            );
            local_config.save_in_dir(project_root)?;
            Ok(linked_repo)
        }
        None => resolve_existing_link(local_config, dependency),
    }
}

fn resolve_existing_link(local_config: &LocalConfig, dependency: &DependencyContext) -> Result<PathBuf> {
    let link = local_config.relay_link(&dependency.alias).ok_or_else(|| {
        anyhow!(
            "no relay link configured for `{}`; rerun with `--repo-path <path>`",
            dependency.alias
        )
    })?;
    let linked_repo = canonicalize_existing_dir(&link.repo_path)?;
    validate_linked_repo(&linked_repo, &dependency.url)?;
    Ok(linked_repo)
}

fn validate_linked_repo(path: &Path, url: &str) -> Result<()> {
    if !is_git_repository(path) {
        bail!("linked repo {} is not a git repository", path.display());
    }
    let origin = repository_origin_url(path)
        .with_context(|| format!("linked repo {} is missing an `origin` remote", path.display()))?;
    if !git_urls_match(&origin, url) {
        bail!(
            "linked repo {} has origin `{}` instead of `{}`",
            path.display(),
            origin,
            url
        );
    }
    Ok(())
}

fn build_relay_plan(
    dependency: &DependencyContext,
    project_root: &Path,
    selected_adapters: Adapters,
    linked_repo: &Path,
) -> Result<RelayPlan> {
    let mappings = build_mappings(dependency, project_root, selected_adapters, linked_repo)?;
    let mut grouped = BTreeMap::<PathBuf, Vec<RelayFileMapping>>::new();
    for mapping in mappings {
        grouped
            .entry(mapping.linked_source_path.clone())
            .or_default()
            .push(mapping);
    }

    let mut plan = RelayPlan::default();
    for (linked_source_path, group) in grouped {
        let mut candidate_source: Option<Vec<u8>> = None;
        let linked_current = fs::read(&linked_source_path).ok();
        let mut linked_changed = false;

        for mapping in group {
            let baseline_source = fs::read(&mapping.snapshot_path).with_context(|| {
                format!("failed to read relay baseline {}", mapping.snapshot_path.display())
            })?;
            let baseline_managed = mapping.transform.to_managed_bytes(&baseline_source)?;
            let current_managed = match fs::read(&mapping.managed_path) {
                Ok(contents) => contents,
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                    plan.conflicts.push(format!(
                        "{} is missing from managed outputs",
                        mapping.managed_path.display()
                    ));
                    continue;
                }
                Err(error) => {
                    return Err(error).with_context(|| {
                        format!("failed to read managed file {}", mapping.managed_path.display())
                    });
                }
            };
            if current_managed == baseline_managed {
                continue;
            }

            let candidate = mapping
                .transform
                .to_source_bytes(&current_managed, &baseline_source)?;
            if linked_current.as_deref() != Some(baseline_source.as_slice()) {
                linked_changed = true;
            }
            if let Some(existing) = &candidate_source {
                if existing != &candidate {
                    plan.conflicts.push(format!(
                        "managed variants for {} disagree on relayed contents",
                        linked_source_path.display()
                    ));
                    continue;
                }
            } else {
                candidate_source = Some(candidate);
            }
        }

        let Some(candidate_source) = candidate_source else {
            continue;
        };
        if linked_current.as_deref() == Some(candidate_source.as_slice()) {
            plan.noops.insert(linked_source_path);
            continue;
        }
        if linked_changed {
            plan.conflicts.push(format!(
                "{} changed in both managed outputs and linked source",
                linked_source_path.display()
            ));
            continue;
        }
        plan.updates.insert(linked_source_path, candidate_source);
    }

    Ok(plan)
}

fn build_mappings(
    dependency: &DependencyContext,
    project_root: &Path,
    selected_adapters: Adapters,
    linked_repo: &Path,
) -> Result<Vec<RelayFileMapping>> {
    let mut mappings = Vec::new();
    let package = &dependency.package;
    let snapshot_root = &dependency.snapshot_root;

    for skill in &package.manifest.discovered.skills {
        if !package.selects_component(crate::manifest::DependencyComponent::Skills) {
            continue;
        }

        for adapter in [
            Adapter::Agents,
            Adapter::Claude,
            Adapter::Codex,
            Adapter::Cursor,
            Adapter::OpenCode,
        ] {
            if !selected_adapters.contains(adapter) {
                continue;
            }
            let source_root = snapshot_root.join(&skill.path);
            let managed_root = managed_skill_root(project_root, adapter, package, &skill.id);
            let target_root = linked_repo.join(&skill.path);
            mappings.extend(skill_mappings(
                adapter,
                package,
                skill,
                snapshot_root,
                &source_root,
                &target_root,
                &managed_root,
            )?);
        }
    }

    for agent in &package.manifest.discovered.agents {
        if !package.selects_component(crate::manifest::DependencyComponent::Agents) {
            continue;
        }
        for adapter in [Adapter::Claude, Adapter::OpenCode] {
            if !selected_adapters.contains(adapter) {
                continue;
            }
            if let Some(managed_path) =
                managed_artifact_path(project_root, adapter, ArtifactKind::Agent, package, &agent.id)
            {
                mappings.push(file_mapping(
                    managed_path,
                    snapshot_root.join(&agent.path),
                    linked_repo.join(&agent.path),
                    RelayTransform::None,
                ));
            }
        }
    }

    for rule in &package.manifest.discovered.rules {
        if !package.selects_component(crate::manifest::DependencyComponent::Rules) {
            continue;
        }
        for (adapter, kind) in [
            (Adapter::Claude, ArtifactKind::Rule),
            (Adapter::Codex, ArtifactKind::Rule),
            (Adapter::Cursor, ArtifactKind::Rule),
            (Adapter::OpenCode, ArtifactKind::Rule),
        ] {
            if !selected_adapters.contains(adapter) {
                continue;
            }
            if let Some(managed_path) =
                managed_artifact_path(project_root, adapter, kind, package, &rule.id)
            {
                mappings.push(file_mapping(
                    managed_path,
                    snapshot_root.join(&rule.path),
                    linked_repo.join(&rule.path),
                    RelayTransform::None,
                ));
            }
        }
    }

    for command in &package.manifest.discovered.commands {
        if !package.selects_component(crate::manifest::DependencyComponent::Commands) {
            continue;
        }
        for (adapter, kind) in [
            (Adapter::Agents, ArtifactKind::Command),
            (Adapter::Claude, ArtifactKind::Command),
            (Adapter::Cursor, ArtifactKind::Command),
            (Adapter::OpenCode, ArtifactKind::Command),
        ] {
            if !selected_adapters.contains(adapter) {
                continue;
            }
            if let Some(managed_path) =
                managed_artifact_path(project_root, adapter, kind, package, &command.id)
            {
                mappings.push(file_mapping(
                    managed_path,
                    snapshot_root.join(&command.path),
                    linked_repo.join(&command.path),
                    RelayTransform::None,
                ));
            }
        }
    }

    Ok(mappings)
}

fn skill_mappings(
    adapter: Adapter,
    package: &ResolvedPackage,
    skill: &SkillEntry,
    snapshot_root: &Path,
    source_root: &Path,
    linked_root: &Path,
    managed_root: &Path,
) -> Result<Vec<RelayFileMapping>> {
    let mut mappings = Vec::new();
    for entry in walkdir::WalkDir::new(source_root) {
        let entry = entry?;
        if !entry.file_type().is_file() {
            continue;
        }
        let relative = entry
            .path()
            .strip_prefix(source_root)
            .with_context(|| format!("failed to make {} relative", entry.path().display()))?;
        let transform = if adapter == Adapter::OpenCode && relative == Path::new("SKILL.md") {
            RelayTransform::OpenCodeSkillName {
                managed_skill_id: crate::adapters::namespaced_skill_id(package, &skill.id),
            }
        } else {
            RelayTransform::None
        };
        mappings.push(file_mapping(
            managed_root.join(relative),
            snapshot_root.join(&skill.path).join(relative),
            linked_root.join(relative),
            transform,
        ));
    }
    Ok(mappings)
}

fn file_mapping(
    managed_path: PathBuf,
    snapshot_path: PathBuf,
    linked_source_path: PathBuf,
    transform: RelayTransform,
) -> RelayFileMapping {
    RelayFileMapping {
        managed_path,
        snapshot_path,
        linked_source_path,
        transform,
    }
}

impl RelayTransform {
    fn to_managed_bytes(&self, source: &[u8]) -> Result<Vec<u8>> {
        match self {
            Self::None => Ok(source.to_vec()),
            Self::OpenCodeSkillName { managed_skill_id } => {
                crate::adapters::opencode::rewrite_skill_name(source, managed_skill_id)
            }
        }
    }

    fn to_source_bytes(&self, managed: &[u8], baseline_source: &[u8]) -> Result<Vec<u8>> {
        match self {
            Self::None => Ok(managed.to_vec()),
            Self::OpenCodeSkillName { managed_skill_id } => {
                restore_opencode_skill_name(managed, baseline_source, managed_skill_id)
            }
        }
    }
}

fn restore_opencode_skill_name(
    managed: &[u8],
    baseline_source: &[u8],
    managed_skill_id: &str,
) -> Result<Vec<u8>> {
    let managed = String::from_utf8(managed.to_vec()).context("OpenCode managed skills must be UTF-8")?;
    let baseline_source = String::from_utf8(baseline_source.to_vec())
        .context("OpenCode source skills must be UTF-8")?;
    let restored_name = extract_frontmatter_name(&baseline_source)?;
    let mut lines = managed.lines().map(str::to_string).collect::<Vec<_>>();
    let Some(index) = lines
        .iter()
        .position(|line| line.trim_start() == format!("name: {managed_skill_id}"))
        .or_else(|| lines.iter().position(|line| line.trim_start().starts_with("name:")))
    else {
        bail!("OpenCode managed skill is missing a frontmatter `name`");
    };
    lines[index] = format!("name: {restored_name}");
    let mut restored = lines.join("\n");
    if managed.ends_with('\n') {
        restored.push('\n');
    }
    Ok(restored.into_bytes())
}

fn extract_frontmatter_name(contents: &str) -> Result<String> {
    let lines = contents.lines().collect::<Vec<_>>();
    if lines.first().copied() != Some("---") {
        bail!("OpenCode skill is missing YAML frontmatter");
    }
    let Some(frontmatter_end) = lines.iter().skip(1).position(|line| *line == "---") else {
        bail!("OpenCode skill is missing a closing frontmatter fence");
    };
    let frontmatter_end = frontmatter_end + 1;
    for line in lines.iter().take(frontmatter_end) {
        if let Some(value) = line.trim_start().strip_prefix("name:") {
            return Ok(value.trim().to_string());
        }
    }
    bail!("OpenCode skill is missing a frontmatter `name`")
}

fn canonicalize_existing_dir(path: &Path) -> Result<PathBuf> {
    let canonical = path
        .canonicalize()
        .with_context(|| format!("failed to access {}", path.display()))?;
    if !canonical.is_dir() {
        bail!("{} is not a directory", canonical.display());
    }
    Ok(canonical)
}

fn display_relative(root: &Path, path: &Path) -> String {
    path.strip_prefix(root)
        .unwrap_or(path)
        .to_string_lossy()
        .replace('\\', "/")
}
