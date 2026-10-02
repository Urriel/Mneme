use std::fs;
use std::path::{Path, PathBuf};

use crate::Error;

const MNEME_SKILL: &str = include_str!("../.agents/skills/mneme/SKILL.md");
const DREAM_SKILL: &str = include_str!("../.agents/skills/mneme-dream/SKILL.md");

const PROJECT_POINTER: &str = "If the Mneme tools ingest, search, list_recent, add_note, link, and set_cursor are available, follow `.agents/skills/mneme/SKILL.md`. The dream pass is `.agents/skills/mneme-dream/SKILL.md`.";
const GLOBAL_POINTER: &str = "If the Mneme tools ingest, search, list_recent, add_note, link, and set_cursor are available, follow `~/.agents/skills/mneme/SKILL.md`. The dream pass is `~/.agents/skills/mneme-dream/SKILL.md`.";

#[derive(Clone, Debug)]
pub enum SetupScope {
    Project { root: PathBuf },
    Global { home: PathBuf },
}

pub fn setup(scope: SetupScope) -> Result<Vec<String>, Error> {
    let mut lines = Vec::new();
    match scope {
        SetupScope::Project { root } => {
            install_pair(&root.join(".agents/skills"), &mut lines)?;
            link_claude_skills(&root.join(".claude/skills"), &mut lines)?;
            ensure_pointer(&root.join("AGENTS.md"), PROJECT_POINTER, &mut lines)?;
            ensure_claude_md(&root, &mut lines)?;
        }
        SetupScope::Global { home } => {
            install_pair(&home.join(".agents/skills"), &mut lines)?;
            link_claude_skills(&home.join(".claude/skills"), &mut lines)?;
            ensure_pointer(&home.join(".claude/CLAUDE.md"), GLOBAL_POINTER, &mut lines)?;
        }
    }
    Ok(lines)
}

pub fn mcp_snippet(command: &Path, data_dir: &Path) -> String {
    format!(
        "{{\n  \"mcpServers\": {{\n    \"mneme\": {{\n      \"command\": \"{}\",\n      \"args\": [\"run\", \"--data\", \"{}\"]\n    }}\n  }}\n}}",
        command.display(),
        data_dir.display()
    )
}

fn install_pair(skills_root: &Path, lines: &mut Vec<String>) -> Result<(), Error> {
    install_skill(&skills_root.join("mneme/SKILL.md"), MNEME_SKILL, lines)?;
    install_skill(
        &skills_root.join("mneme-dream/SKILL.md"),
        DREAM_SKILL,
        lines,
    )?;
    Ok(())
}

fn install_skill(path: &Path, body: &str, lines: &mut Vec<String>) -> Result<(), Error> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    if path.is_file() {
        let current = fs::read_to_string(path)?;
        if current == body {
            lines.push(format!("kept {}", path.display()));
        } else {
            lines.push(format!("left {} because it differs", path.display()));
        }
        return Ok(());
    }
    fs::write(path, body)?;
    lines.push(format!("wrote {}", path.display()));
    Ok(())
}

fn link_claude_skills(claude_skills: &Path, lines: &mut Vec<String>) -> Result<(), Error> {
    link_skill(claude_skills, "mneme", lines)?;
    link_skill(claude_skills, "mneme-dream", lines)?;
    Ok(())
}

fn link_skill(claude_skills: &Path, name: &str, lines: &mut Vec<String>) -> Result<(), Error> {
    let link = claude_skills.join(name).join("SKILL.md");
    let target = PathBuf::from(format!("../../../.agents/skills/{name}/SKILL.md"));
    if let Some(parent) = link.parent() {
        fs::create_dir_all(parent)?;
    }
    if link.symlink_metadata().is_ok() {
        if fs::read_link(&link).ok().as_deref() == Some(target.as_path()) {
            lines.push(format!("kept link {}", link.display()));
            return Ok(());
        }
        lines.push(format!("left {} because it differs", link.display()));
        return Ok(());
    }
    std::os::unix::fs::symlink(&target, &link)?;
    lines.push(format!("linked {}", link.display()));
    Ok(())
}

fn ensure_pointer(path: &Path, pointer: &str, lines: &mut Vec<String>) -> Result<(), Error> {
    if let Some(parent) = path.parent() {
        if !parent.as_os_str().is_empty() {
            fs::create_dir_all(parent)?;
        }
    }
    if !path.exists() {
        fs::write(path, format!("{pointer}\n"))?;
        lines.push(format!("added the Mneme pointer to {}", path.display()));
        return Ok(());
    }
    let current = fs::read_to_string(path)?;
    if current.contains(pointer) {
        lines.push(format!("Mneme pointer already in {}", path.display()));
        return Ok(());
    }
    let mut next = current;
    if !next.is_empty() && !next.ends_with('\n') {
        next.push('\n');
    }
    if !next.is_empty() {
        next.push('\n');
    }
    next.push_str(pointer);
    next.push('\n');
    fs::write(path, next)?;
    lines.push(format!("added the Mneme pointer to {}", path.display()));
    Ok(())
}

fn ensure_claude_md(root: &Path, lines: &mut Vec<String>) -> Result<(), Error> {
    let claude = root.join("CLAUDE.md");
    if !claude.symlink_metadata().is_ok() {
        std::os::unix::fs::symlink("AGENTS.md", &claude)?;
        lines.push(format!("linked {}", claude.display()));
        return Ok(());
    }
    if fs::read_link(&claude).ok().as_deref() == Some(Path::new("AGENTS.md")) {
        lines.push(format!("kept link {}", claude.display()));
        return Ok(());
    }
    ensure_pointer(&claude, PROJECT_POINTER, lines)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::time::{SystemTime, UNIX_EPOCH};

    static TEMP_SEQ: AtomicU64 = AtomicU64::new(0);

    struct TempDir(PathBuf);

    impl TempDir {
        fn new() -> Self {
            let stamp = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos();
            let seq = TEMP_SEQ.fetch_add(1, Ordering::Relaxed);
            let path = std::env::temp_dir()
                .join(format!("mneme-setup-{stamp}-{seq}-{}", std::process::id()));
            fs::create_dir_all(&path).unwrap();
            Self(path)
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn project_setup_is_idempotent_and_keeps_edits() {
        let dir = TempDir::new();
        let first = setup(SetupScope::Project {
            root: dir.0.clone(),
        })
        .unwrap();
        assert!(
            first.iter().any(|line| line.starts_with("wrote ")),
            "first setup writes the skills: {first:?}"
        );
        let skill = fs::read_to_string(dir.0.join(".agents/skills/mneme/SKILL.md")).unwrap();
        assert!(
            skill.contains("name: mneme"),
            "installed skill has the name"
        );
        let linked = fs::read_to_string(dir.0.join(".claude/skills/mneme/SKILL.md")).unwrap();
        assert_eq!(skill, linked, "the Claude skill link reads the same file");
        let agents = fs::read_to_string(dir.0.join("AGENTS.md")).unwrap();
        assert_eq!(agents.matches(PROJECT_POINTER).count(), 1);
        assert_eq!(
            fs::read_link(dir.0.join("CLAUDE.md")).unwrap(),
            PathBuf::from("AGENTS.md")
        );

        let second = setup(SetupScope::Project {
            root: dir.0.clone(),
        })
        .unwrap();
        assert!(
            second
                .iter()
                .all(|line| line.starts_with("kept ") || line.contains("already")),
            "second setup changes nothing: {second:?}"
        );
        let agents_again = fs::read_to_string(dir.0.join("AGENTS.md")).unwrap();
        assert_eq!(agents_again.matches(PROJECT_POINTER).count(), 1);

        fs::write(dir.0.join(".agents/skills/mneme/SKILL.md"), "local edit\n").unwrap();
        let third = setup(SetupScope::Project {
            root: dir.0.clone(),
        })
        .unwrap();
        assert!(
            third.iter().any(|line| line.contains("differs")),
            "an edited skill stays: {third:?}"
        );
        assert_eq!(
            fs::read_to_string(dir.0.join(".agents/skills/mneme/SKILL.md")).unwrap(),
            "local edit\n"
        );
    }

    #[test]
    fn an_existing_claude_file_gains_one_pointer() {
        let dir = TempDir::new();
        fs::write(dir.0.join("CLAUDE.md"), "# Notes\n").unwrap();
        setup(SetupScope::Project {
            root: dir.0.clone(),
        })
        .unwrap();
        setup(SetupScope::Project {
            root: dir.0.clone(),
        })
        .unwrap();
        let claude = fs::read_to_string(dir.0.join("CLAUDE.md")).unwrap();
        assert!(claude.starts_with("# Notes\n"), "existing text stays");
        assert_eq!(claude.matches(PROJECT_POINTER).count(), 1);
    }

    #[test]
    fn global_setup_writes_under_the_home_directory() {
        let home = TempDir::new();
        let lines = setup(SetupScope::Global {
            home: home.0.clone(),
        })
        .unwrap();
        assert!(
            home.0.join(".agents/skills/mneme-dream/SKILL.md").is_file(),
            "global dream skill is installed: {lines:?}"
        );
        let memory = fs::read_to_string(home.0.join(".claude/CLAUDE.md")).unwrap();
        assert!(memory.contains("~/.agents/skills/mneme/SKILL.md"));
    }
}
