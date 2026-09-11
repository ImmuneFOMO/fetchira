//! Embedded agent instructions, installed independently of MCP registration.
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{bail, Context};

const SHARED: &str = include_str!("../skills/shared.md");
const VARIANTS: [SkillVariant; 3] = [SkillVariant::Both, SkillVariant::Mcp, SkillVariant::Cli];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum SkillVariant {
    Both,
    Mcp,
    Cli,
    Skip,
}

impl SkillVariant {
    pub(crate) fn parse(value: &str) -> Option<Self> {
        match value {
            "both" => Some(Self::Both),
            "mcp" => Some(Self::Mcp),
            "cli" => Some(Self::Cli),
            "skip" => Some(Self::Skip),
            _ => None,
        }
    }
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Both => "both",
            Self::Mcp => "mcp",
            Self::Cli => "cli",
            Self::Skip => "skip",
        }
    }
    fn folder(self) -> &'static str {
        match self {
            Self::Both => "fetchira",
            Self::Mcp => "fetchira-mcp",
            Self::Cli => "fetchira-cli",
            Self::Skip => unreachable!(),
        }
    }
    fn text(self) -> &'static str {
        match self {
            Self::Both => include_str!("../skills/fetchira/SKILL.md"),
            Self::Mcp => include_str!("../skills/fetchira-mcp/SKILL.md"),
            Self::Cli => include_str!("../skills/fetchira-cli/SKILL.md"),
            Self::Skip => unreachable!(),
        }
    }
}

#[cfg(test)]
fn skill_text(variant: SkillVariant) -> String {
    let home = crate::cli::home();
    let home = if home.is_absolute() {
        home
    } else {
        std::env::current_dir()
            .map(|current| current.join(home))
            .unwrap_or_else(|_| crate::cli::home())
    };
    let Ok(bin) = std::env::current_exe() else {
        return variant.text().to_string();
    };
    skill_text_for(variant, &home, &bin)
}

fn skill_text_for(variant: SkillVariant, home: &Path, bin: &Path) -> String {
    let base = variant.text();
    if !matches!(variant, SkillVariant::Both | SkillVariant::Cli) {
        return base.to_string();
    }
    let home = shell_quote(&home.to_string_lossy());
    let bin = shell_quote(&bin.to_string_lossy());
    let command = if cfg!(windows) {
        format!("$env:FETCHIRA_HOME = {home}; & {bin}")
    } else {
        format!("FETCHIRA_HOME={home} {bin}")
    };
    format!(
        "{base}\n\n## Installation-specific command\n\nReplace the bare `fetchira` executable in the command forms above with `{command}` so the agent uses this installation's configured accounts and hosted connection. Keep the operation and flags after the executable."
    )
}

fn shell_quote(value: &str) -> String {
    if cfg!(windows) {
        format!("'{}'", value.replace('\'', "''"))
    } else {
        format!("'{}'", value.replace('\'', "'\\''"))
    }
}

pub(crate) struct SkillDestination {
    pub(crate) name: &'static str,
    pub(crate) parent: PathBuf,
    pub(crate) present: bool,
    pub(crate) variants: Vec<SkillVariant>,
    pub(crate) error: Option<String>,
    cleanup: Vec<PathBuf>,
    create_parent: bool,
}
#[derive(Debug)]
pub(crate) struct SkillInstallResult {
    pub(crate) name: &'static str,
    pub(crate) ok: bool,
    pub(crate) msg: String,
}

/// Inspect only direct children of the trusted home, without following symlinks.
fn metadata(path: &Path) -> anyhow::Result<Option<fs::Metadata>> {
    match fs::symlink_metadata(path) {
        Ok(m) if m.file_type().is_symlink() => bail!("refusing symlink {}", path.display()),
        Ok(m) => Ok(Some(m)),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e).with_context(|| format!("inspect {}", path.display())),
    }
}
fn directory(path: &Path) -> anyhow::Result<bool> {
    match metadata(path)? {
        Some(m) if !m.is_dir() => bail!("{} is not a directory", path.display()),
        m => Ok(m.is_some()),
    }
}
fn regular_file(path: &Path) -> anyhow::Result<bool> {
    Ok(matches!(metadata(path)?, Some(m) if m.is_file()))
}

pub(crate) fn skill_destinations(home: &Path) -> Vec<SkillDestination> {
    skill_destinations_with(home, configured_codex_home())
}

fn configured_codex_home() -> Option<PathBuf> {
    std::env::var_os("CODEX_HOME")
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
}

fn path_exists(path: &Path) -> bool {
    fs::symlink_metadata(path).is_ok()
}

/// Return the agent skill roots without installing the shared `.agents` skill twice.
///
/// Codex's documented user root is `$HOME/.agents/skills`; `$CODEX_HOME/skills` is its explicit
/// profile root, and `.codex/skills` is a legacy source migrated on install. Gemini also scans
/// `.agents/skills`.
fn skill_destinations_with(home: &Path, codex_home: Option<PathBuf>) -> Vec<SkillDestination> {
    // macOS's system /var -> /private/var alias is outside the untrusted agent paths.
    let root = home.canonicalize().unwrap_or_else(|_| home.to_path_buf());
    let agents = root.join(".agents");
    let legacy_codex = root.join(".codex");
    let codex_home =
        codex_home.filter(|path| path != &legacy_codex && path != &home.join(".codex"));
    let custom_codex = codex_home.is_some();
    let codex_detected = custom_codex || path_exists(&agents) || path_exists(&legacy_codex);
    // Always prefer the documented shared root. If the legacy Codex state directory exists,
    // create `.agents` on first install and move any old Fetchira variants into its backup.
    let codex = codex_home.unwrap_or_else(|| agents.clone());
    let codex_uses_shared_agents = codex == agents;
    let mut specs: Vec<(&'static str, PathBuf, Vec<PathBuf>, bool)> = vec![
        ("Claude", root.join(".claude"), vec![], false),
        ("Cursor", root.join(".cursor"), vec![], false),
        (
            "Codex",
            codex.clone(),
            codex_cleanup(&root, &codex, &agents, &legacy_codex),
            codex_uses_shared_agents && !path_exists(&agents) && path_exists(&legacy_codex),
        ),
    ];
    // With the default Codex root, both Codex and Gemini load `.agents/skills`; one installed
    // folder is enough. A custom CODEX_HOME leaves `.agents/skills` available to Gemini.
    if !codex_detected || !codex_uses_shared_agents {
        let gemini = if path_exists(&agents) {
            agents.clone()
        } else {
            root.join(".gemini")
        };
        let cleanup = (gemini == agents)
            .then(|| root.join(".gemini"))
            .into_iter()
            .collect();
        specs.push(("Gemini", gemini, cleanup, false));
    }
    specs
        .into_iter()
        .map(|(name, parent, cleanup, create_parent)| {
            let mut dest = SkillDestination {
                name,
                parent,
                present: false,
                variants: vec![],
                error: None,
                cleanup,
                create_parent,
            };
            let probe = || -> anyhow::Result<(bool, Vec<SkillVariant>)> {
                if !directory(&dest.parent)? {
                    return Ok((dest.create_parent, vec![]));
                }
                let skills = dest.parent.join("skills");
                if !directory(&skills)? {
                    return Ok((true, vec![]));
                }
                let mut variants = Vec::new();
                for v in VARIANTS {
                    let folder = skills.join(v.folder());
                    if !directory(&folder)? {
                        continue;
                    }
                    if regular_file(&folder.join("SKILL.md"))?
                        && regular_file(&folder.join("references.md"))?
                    {
                        variants.push(v);
                    }
                }
                Ok((true, variants))
            };
            match probe() {
                Ok((present, variants)) => {
                    dest.present = present;
                    dest.variants = variants;
                }
                Err(e) => dest.error = Some(format!("{e:#}")),
            }
            dest
        })
        .collect()
}

fn codex_cleanup(root: &Path, active: &Path, agents: &Path, legacy: &Path) -> Vec<PathBuf> {
    let mut paths = Vec::new();
    if active != legacy {
        paths.push(legacy.to_path_buf());
    }
    // `.gemini/skills` is a second Gemini discovery root only when `.agents` is Codex's active
    // default root. Move stale Fetchira variants there as well to avoid duplicate instructions.
    if active == agents {
        paths.push(root.join(".gemini"));
    }
    paths.retain(|path| path != active);
    paths.sort();
    paths.dedup();
    paths
}

pub(crate) fn install_skills_for_agents(
    home: &Path,
    config_home: &Path,
    bin: &Path,
    variant: SkillVariant,
    agents: &[String],
) -> Vec<SkillInstallResult> {
    install_skills_with_agents(
        home,
        config_home,
        bin,
        variant,
        configured_codex_home(),
        Some(agents),
    )
}

/// Refresh only existing skills, preserving their transport choice and archiving custom files.
pub(crate) fn refresh_existing(
    home: &Path,
    config_home: &Path,
    bin: &Path,
    apply: bool,
) -> Vec<SkillInstallResult> {
    refresh_destinations(skill_destinations(home), config_home, bin, apply)
}

fn refresh_destinations(
    destinations: Vec<SkillDestination>,
    config_home: &Path,
    bin: &Path,
    apply: bool,
) -> Vec<SkillInstallResult> {
    destinations.into_iter().filter_map(|dest| {
        let inspect = || -> anyhow::Result<Option<(SkillVariant, String)>> {
            if let Some(error) = &dest.error { bail!("{error}"); }
            let mut old = fetchira_variants(&dest.parent)?;
            for alias in &dest.cleanup { old.extend(fetchira_variants(alias)?); }
            if old.is_empty() { return Ok(None); }
            let variants: Vec<_> = VARIANTS.into_iter().filter(|variant| {
                old.iter().any(|path| path.file_name().and_then(|name| name.to_str()) == Some(variant.folder()))
            }).collect();
            if variants.len() != 1 {
                bail!("conflicting Fetchira skills at {}; run `fetchira install` to choose one integration", dest.parent.display());
            }
            let variant = variants[0];
            let text = skill_text_for(variant, &crate::cli::absolute_path(config_home), bin);
            let selected = dest.parent.join("skills").join(variant.folder());
            if old.len() == 1 && old[0] == selected
                && regular_file(&selected.join("SKILL.md"))?
                && regular_file(&selected.join("references.md"))?
                && fs::read_to_string(selected.join("SKILL.md"))? == text
                && fs::read_to_string(selected.join("references.md"))? == SHARED {
                return Ok(None);
            }
            Ok(Some((variant, text)))
        };
        let result = match inspect() {
            Ok(None) => return None,
            Ok(Some((variant, text))) if apply => install_destination(&dest.parent, variant, &dest.cleanup, &text),
            Ok(Some(_)) => Ok(format!("update available at {}", dest.parent.display())),
            Err(error) => Err(error),
        };
        Some(SkillInstallResult { name: dest.name, ok: result.is_ok(), msg: match result {
            Ok(message) => message,
            Err(error) => format!("{error:#}"),
        }})
    }).collect()
}

/// Check skill roots before the caller changes any MCP configuration. Cursor can discover the
/// common agent roots, so a different Fetchira variant outside the selected roots would make the
/// resulting installation ambiguous. Selected roots are managed by `install_destination`, which
/// archives an old variant and may therefore switch it safely.
pub(crate) fn preflight_skills_for_agents(
    home: &Path,
    variant: SkillVariant,
    agents: &[String],
) -> anyhow::Result<()> {
    if variant == SkillVariant::Skip || agents.is_empty() {
        return Ok(());
    }
    let root = home
        .canonicalize()
        .unwrap_or_else(|_| home.to_path_buf())
        .to_path_buf();
    let shared_agents = root.join(".agents");
    let destinations = skill_destinations_with(home, configured_codex_home());
    let managed: Vec<PathBuf> = destinations
        .iter()
        .filter(|destination| destination_selected(destination, &shared_agents, agents))
        .flat_map(|destination| {
            std::iter::once(destination.parent.clone()).chain(destination.cleanup.iter().cloned())
        })
        .collect();
    if managed.is_empty() {
        return Ok(());
    }
    // These are the roots that can be loaded together by Cursor. Keep this list explicit so a
    // user selecting one agent gets an actionable conflict before another config file is edited.
    let visible = [".claude", ".cursor", ".agents", ".codex"]
        .into_iter()
        .map(|name| root.join(name));
    let mut conflicts = Vec::new();
    for visible_root in visible {
        if managed.iter().any(|path| path == &visible_root) {
            continue;
        }
        for existing in fetchira_variants(&visible_root)? {
            let name = existing
                .file_name()
                .and_then(|name| name.to_str())
                .unwrap_or("unknown");
            if name != variant.folder() {
                conflicts.push(format!("{} ({name})", existing.display()));
            }
        }
    }
    // Selected destinations can still fail before installation due to a symlink or a non-directory
    // parent. Surface those errors during the same preflight, before any MCP target is changed.
    for destination in destinations
        .iter()
        .filter(|destination| destination_selected(destination, &shared_agents, agents))
    {
        if let Some(error) = &destination.error {
            bail!(
                "cannot install {} for the selected agents: inspect {} first ({error})",
                variant.folder(),
                destination.parent.display()
            );
        }
        for cleanup in &destination.cleanup {
            if let Err(error) = fetchira_variants(cleanup) {
                bail!(
                    "cannot install {} for the selected agents: inspect {} first ({error})",
                    variant.folder(),
                    cleanup.display()
                );
            }
        }
    }
    conflicts.sort();
    conflicts.dedup();
    if let Some(first) = conflicts.first() {
        let more = conflicts.len().saturating_sub(1);
        bail!(
            "cannot install {}: a different Fetchira skill is visible outside the selected destinations at {first}{}; select that agent too or switch it first",
            variant.folder(),
            if more == 0 {
                String::new()
            } else {
                format!(" and {more} more location(s)")
            }
        );
    }
    Ok(())
}

#[cfg(test)]
fn install_skills_with(
    home: &Path,
    variant: SkillVariant,
    codex_home: Option<PathBuf>,
) -> Vec<SkillInstallResult> {
    let config_home = crate::cli::home();
    let bin = std::env::current_exe().unwrap_or_else(|_| PathBuf::from("fetchira"));
    install_skills_with_agents(home, &config_home, &bin, variant, codex_home, None)
}

fn install_skills_with_agents(
    home: &Path,
    config_home: &Path,
    bin: &Path,
    variant: SkillVariant,
    codex_home: Option<PathBuf>,
    selected_agents: Option<&[String]>,
) -> Vec<SkillInstallResult> {
    let shared_agents = home
        .canonicalize()
        .unwrap_or_else(|_| home.to_path_buf())
        .join(".agents");
    skill_destinations_with(home, codex_home)
        .into_iter()
        .map(|dest| {
            let result = if variant == SkillVariant::Skip {
                Ok("skipped".into())
            } else if selected_agents
                .is_some_and(|agents| !destination_selected(&dest, &shared_agents, agents))
            {
                Ok(format!(
                    "skipped: {} was not selected",
                    dest.parent.display()
                ))
            } else if let Some(error) = dest.error {
                Err(anyhow::anyhow!(error))
            } else if !dest.present {
                Ok(format!("skipped: {} does not exist", dest.parent.display()))
            } else {
                let text = skill_text_for(variant, &crate::cli::absolute_path(config_home), bin);
                install_destination(&dest.parent, variant, &dest.cleanup, &text)
            };
            match result {
                Ok(msg) => SkillInstallResult {
                    name: dest.name,
                    ok: true,
                    msg,
                },
                Err(e) => SkillInstallResult {
                    name: dest.name,
                    ok: false,
                    msg: format!("{e:#}"),
                },
            }
        })
        .collect()
}

fn destination_selected(
    destination: &SkillDestination,
    shared_agents: &Path,
    agents: &[String],
) -> bool {
    let selected = |name: &str| agents.iter().any(|agent| agent == name);
    match destination.name {
        "Claude" => selected("Claude Code"),
        "Cursor" => selected("Cursor"),
        "Codex" => {
            selected("Codex CLI") || (destination.parent == shared_agents && selected("Gemini CLI"))
        }
        "Gemini" => {
            selected("Gemini CLI") || (destination.parent == shared_agents && selected("Codex CLI"))
        }
        _ => false,
    }
}

fn fetchira_variants(parent: &Path) -> anyhow::Result<Vec<PathBuf>> {
    if !directory(parent)? {
        return Ok(vec![]);
    }
    let skills = parent.join("skills");
    if !directory(&skills)? {
        return Ok(vec![]);
    }
    let mut variants = Vec::new();
    for v in VARIANTS {
        let path = skills.join(v.folder());
        if directory(&path)? {
            variants.push(path);
        }
    }
    Ok(variants)
}

fn install_destination(
    parent: &Path,
    variant: SkillVariant,
    cleanup: &[PathBuf],
    skill_text: &str,
) -> anyhow::Result<String> {
    let parent_present = directory(parent)?;
    // Validate every conflicting path before replacing a working installation. This includes
    // legacy Codex/Gemini roots when `.agents/skills` is the active shared root.
    let old = if parent_present {
        fetchira_variants(parent)?
    } else {
        Vec::new()
    };
    let mut aliases = Vec::new();
    for path in cleanup {
        for variant in fetchira_variants(path)? {
            if !old.contains(&variant) {
                aliases.push(variant);
            }
        }
    }
    if !parent_present {
        fs::create_dir(parent)?;
    }
    let skills = parent.join("skills");
    if !directory(&skills)? {
        fs::create_dir(&skills)?;
    }
    let selected = skills.join(variant.folder());
    if old.len() == 1
        && aliases.is_empty()
        && old[0] == selected
        && regular_file(&selected.join("SKILL.md"))?
        && regular_file(&selected.join("references.md"))?
        && fs::read_to_string(selected.join("SKILL.md"))? == skill_text
        && fs::read_to_string(selected.join("references.md"))? == SHARED
    {
        return Ok(format!("already installed {}", selected.display()));
    }
    let id = format!(
        "{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)?
            .as_nanos()
    );
    // Backups live outside skills/ so agents never discover conflicting instructions.
    let backups = parent.join("fetchira-skill-backups");
    if (!old.is_empty() || !aliases.is_empty()) && !directory(&backups)? {
        fs::create_dir(&backups)?;
    }
    let archive = backups.join(&id);
    if !old.is_empty() || !aliases.is_empty() {
        fs::create_dir(&archive)?;
    }
    let staging = parent.join(format!(".fetchira-stage-{id}"));
    fs::create_dir(&staging)?;
    let mut moved: Vec<(PathBuf, PathBuf)> = vec![];
    let mut committed = false;
    let result = (|| -> anyhow::Result<()> {
        fs::write(staging.join("SKILL.md"), skill_text)?;
        fs::write(staging.join("references.md"), SHARED)?;
        if old.contains(&selected) {
            let backup = archive.join(variant.folder());
            fs::rename(&selected, &backup)?;
            moved.push((selected.clone(), backup));
        }
        fs::rename(&staging, &selected)?;
        committed = true;
        for sibling in old.iter().filter(|p| **p != selected) {
            let backup = archive.join(sibling.file_name().unwrap());
            fs::rename(sibling, &backup)?;
            moved.push((sibling.clone(), backup));
        }
        for (index, sibling) in aliases.iter().enumerate() {
            let backup = archive
                .join("aliases")
                .join(index.to_string())
                .join(sibling.file_name().unwrap());
            if let Some(parent) = backup.parent() {
                fs::create_dir_all(parent)?;
            }
            fs::rename(sibling, &backup)?;
            moved.push((sibling.clone(), backup));
        }
        Ok(())
    })();
    if let Err(error) = result {
        // Only remove the new folder containing our two files; originals remain in backups.
        let rollback = (|| -> anyhow::Result<()> {
            if committed {
                fs::remove_dir_all(&selected)?;
            }
            for (original, backup) in moved.iter().rev() {
                fs::rename(backup, original)?;
            }
            Ok(())
        })();
        let _ = fs::remove_dir_all(&staging);
        if let Err(restore) = rollback {
            bail!(
                "install {} failed: {error}; restore failed: {restore}; originals are in {}",
                selected.display(),
                archive.display()
            );
        }
        return Err(error)
            .with_context(|| format!("install {} (previous skills restored)", selected.display()));
    }
    Ok(if old.is_empty() && aliases.is_empty() {
        format!("installed {}", selected.display())
    } else {
        format!(
            "installed {}; previous files preserved in {}",
            selected.display(),
            archive.display()
        )
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn refresh_preserves_transport_migrates_old_skill_and_does_not_install_new_agents() {
        let home = std::env::temp_dir().join(format!(
            "fetchira-refresh-{}-{}",
            std::process::id(),
            rand::random::<u64>()
        ));
        let legacy = home.join(".codex/skills/fetchira-cli");
        fs::create_dir_all(&legacy).unwrap();
        fs::create_dir_all(home.join(".cursor")).unwrap();
        fs::write(legacy.join("SKILL.md"), "old single-file skill").unwrap();
        fs::write(legacy.join("custom.md"), "user notes").unwrap();
        let run = |apply| {
            refresh_destinations(
                skill_destinations_with(&home, None),
                &home.join("config"),
                Path::new("/usr/local/bin/fetchira"),
                apply,
            )
        };
        assert_eq!(run(false).len(), 1);
        assert!(legacy.exists());
        let refreshed = run(true);
        assert_eq!(refreshed.len(), 1);
        assert!(refreshed[0].ok, "{}", refreshed[0].msg);
        assert!(!legacy.exists());
        assert!(!home.join(".cursor/skills").exists());
        let current = home.join(".agents/skills/fetchira-cli");
        assert_eq!(
            fs::read_to_string(current.join("references.md")).unwrap(),
            SHARED
        );
        assert!(fs::read_to_string(current.join("SKILL.md"))
            .unwrap()
            .contains("/usr/local/bin/fetchira"));
        assert!(run(true).is_empty());
        let archive = fs::read_dir(home.join(".agents/fetchira-skill-backups"))
            .unwrap()
            .next()
            .unwrap()
            .unwrap()
            .path();
        let saved = archive.join("aliases/0/fetchira-cli");
        assert_eq!(
            fs::read_to_string(saved.join("custom.md")).unwrap(),
            "user notes"
        );
        fs::create_dir_all(home.join(".codex/skills/fetchira-mcp")).unwrap();
        let conflicted = run(true);
        assert!(!conflicted[0].ok);
        assert!(conflicted[0].msg.contains("conflicting"));
        assert!(current.exists());
        fs::remove_dir_all(home).unwrap();
    }

    #[test]
    fn install_switch_skip_and_conflict_preserve_files() {
        let home = std::env::temp_dir().join(format!(
            "fetchira-skills-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(home.join(".codex")).unwrap();
        assert_eq!(
            skill_destinations_with(&home, Some(home.join(".codex")))[2].parent,
            home.canonicalize().unwrap().join(".agents")
        );
        let check = |v| assert!(install_skills_with(&home, v, None).iter().all(|r| r.ok));
        check(SkillVariant::Both);
        let original = home.join(".agents/skills/fetchira");
        assert_eq!(
            fs::read_to_string(original.join("SKILL.md")).unwrap(),
            skill_text(SkillVariant::Both)
        );
        assert_eq!(
            fs::read_to_string(original.join("references.md")).unwrap(),
            SHARED
        );
        fs::write(original.join("custom.md"), "keep").unwrap();
        check(SkillVariant::Both);
        assert!(!home.join(".agents/fetchira-skill-backups").exists());
        check(SkillVariant::Skip);
        assert!(original.is_dir());
        check(SkillVariant::Cli);
        assert!(!original.exists());
        let backup = fs::read_dir(home.join(".agents/fetchira-skill-backups"))
            .unwrap()
            .next()
            .unwrap()
            .unwrap()
            .path();
        assert_eq!(
            fs::read_to_string(backup.join("fetchira/custom.md")).unwrap(),
            "keep"
        );
        assert_eq!(
            skill_destinations_with(&home, None)[2].variants,
            [SkillVariant::Cli]
        );
        fs::create_dir_all(home.join(".codex/skills")).unwrap();
        fs::write(home.join(".codex/skills/fetchira-mcp"), "conflict").unwrap();
        assert!(!install_skills_with(&home, SkillVariant::Mcp, None)[2].ok);
        assert!(home.join(".agents/skills/fetchira-cli/SKILL.md").is_file());
        assert!(!home.join(".claude").exists());
        #[cfg(unix)]
        {
            std::os::unix::fs::symlink(home.join(".codex"), home.join(".claude")).unwrap();
            assert!(!install_skills_with(&home, SkillVariant::Both, None)[0].ok);
        }
        fs::remove_dir_all(home).unwrap();
    }

    fn write_variant(parent: &Path, variant: SkillVariant, custom: &str) {
        let folder = parent.join("skills").join(variant.folder());
        fs::create_dir_all(&folder).unwrap();
        fs::write(folder.join("SKILL.md"), variant.text()).unwrap();
        fs::write(folder.join("references.md"), SHARED).unwrap();
        fs::write(folder.join("custom.md"), custom).unwrap();
    }

    #[test]
    fn embedded_skills_have_matching_metadata_and_reference() {
        for variant in VARIANTS {
            let folder = variant.folder();
            let text = variant.text();
            assert!(text.starts_with("---\nname: "));
            assert!(text.contains(&format!("name: {folder}\n")));
            assert!(text.contains("description: ") && !text.contains("description: \n"));
            assert!(!text.contains("TODO"));
        }
    }

    #[test]
    fn installed_cli_skills_pin_the_resolved_home_and_binary() {
        let text = skill_text_for(
            SkillVariant::Cli,
            Path::new("/tmp/fetchira-config"),
            Path::new("/opt/fetchira/bin/fetchira"),
        );
        assert!(text.contains("/tmp/fetchira-config"));
        assert!(text.contains("/opt/fetchira/bin/fetchira"));
        assert!(text.contains("Installation-specific command"));
    }

    #[test]
    fn documented_agents_root_is_shared_and_migrates_legacy_variants() {
        let home = std::env::temp_dir().join(format!(
            "fetchira-agents-skills-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(home.join(".agents")).unwrap();
        fs::create_dir_all(home.join(".codex")).unwrap();
        fs::create_dir_all(home.join(".gemini")).unwrap();
        write_variant(&home.join(".codex"), SkillVariant::Cli, "legacy codex");
        write_variant(&home.join(".gemini"), SkillVariant::Mcp, "legacy gemini");

        let results = install_skills_with(&home, SkillVariant::Both, None);
        assert!(results.iter().all(|result| result.ok), "{results:?}");
        let destinations = skill_destinations_with(&home, None);
        assert_eq!(destinations.len(), 3, "Gemini shares .agents with Codex");
        let codex = destinations.iter().find(|d| d.name == "Codex").unwrap();
        assert_eq!(codex.parent, home.canonicalize().unwrap().join(".agents"));
        assert_eq!(codex.variants, [SkillVariant::Both]);
        let installed = codex.parent.join("skills/fetchira/SKILL.md");
        assert!(installed.is_file());
        assert!(fs::read_to_string(&installed)
            .unwrap()
            .starts_with("---\nname: fetchira\n"));
        assert_eq!(
            fs::read_to_string(codex.parent.join("skills/fetchira/references.md")).unwrap(),
            SHARED
        );
        assert!(!home.join(".codex/skills/fetchira-cli").exists());
        assert!(!home.join(".gemini/skills/fetchira-mcp").exists());
        let backup = codex.parent.join("fetchira-skill-backups");
        let archive = fs::read_dir(&backup)
            .unwrap()
            .next()
            .unwrap()
            .unwrap()
            .path();
        assert_eq!(
            fs::read_to_string(archive.join("aliases/0/fetchira-cli/custom.md")).unwrap(),
            "legacy codex"
        );
        assert_eq!(
            fs::read_to_string(archive.join("aliases/1/fetchira-mcp/custom.md")).unwrap(),
            "legacy gemini"
        );
        fs::remove_dir_all(home).unwrap();
    }

    #[test]
    fn cursor_preflight_reports_a_conflicting_shared_variant() {
        let home = std::env::temp_dir().join(format!(
            "fetchira-skills-preflight-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(home.join(".agents")).unwrap();
        fs::create_dir_all(home.join(".cursor")).unwrap();
        write_variant(&home.join(".agents"), SkillVariant::Mcp, "keep");
        let agents = vec!["Cursor".to_string()];
        let error = preflight_skills_for_agents(&home, SkillVariant::Cli, &agents)
            .unwrap_err()
            .to_string();
        assert!(error.contains("different Fetchira skill"), "{error}");
        assert!(error.contains("fetchira-mcp"), "{error}");
        assert!(error.contains("select that agent"), "{error}");
        assert!(home.join(".agents/skills/fetchira-mcp/SKILL.md").is_file());
        fs::remove_dir_all(home).unwrap();
    }

    #[test]
    fn custom_codex_home_is_separate_from_shared_gemini_root() {
        let home = std::env::temp_dir().join(format!(
            "fetchira-custom-codex-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let custom = home.join("codex-profile");
        fs::create_dir_all(&custom).unwrap();
        fs::create_dir_all(home.join(".agents")).unwrap();
        let destinations = skill_destinations_with(&home, Some(custom.clone()));
        assert_eq!(
            destinations.iter().map(|d| d.name).collect::<Vec<_>>(),
            ["Claude", "Cursor", "Codex", "Gemini"]
        );
        assert_eq!(
            destinations
                .iter()
                .find(|d| d.name == "Codex")
                .unwrap()
                .parent,
            custom
        );
        assert_eq!(
            destinations
                .iter()
                .find(|d| d.name == "Gemini")
                .unwrap()
                .parent,
            home.canonicalize().unwrap().join(".agents")
        );
        let results = install_skills_with(&home, SkillVariant::Cli, Some(custom.clone()));
        assert!(results.iter().all(|result| result.ok), "{results:?}");
        assert!(custom.join("skills/fetchira-cli/SKILL.md").is_file());
        assert!(home.join(".agents/skills/fetchira-cli/SKILL.md").is_file());
        fs::remove_dir_all(home).unwrap();
    }
}
