use std::fs;
use std::path::{Path, PathBuf};

use crate::Error;

const MNEME_SKILL: &str = include_str!("../.agents/skills/mneme/SKILL.md");
const DREAM_SKILL: &str = include_str!("../.agents/skills/mneme-dream/SKILL.md");

const PROJECT_POINTER: &str = "If the Mneme tools ingest, search_doc, search_note, get, list_recent, add_note, link, and set_cursor are available, follow `.agents/skills/mneme/SKILL.md`. The dream pass is `.agents/skills/mneme-dream/SKILL.md`.";
const GLOBAL_POINTER: &str = "If the Mneme tools ingest, search_doc, search_note, get, list_recent, add_note, link, and set_cursor are available, follow `~/.agents/skills/mneme/SKILL.md`. The dream pass is `~/.agents/skills/mneme-dream/SKILL.md`.";

#[derive(Clone, Debug)]
pub enum SetupScope {
    Project { root: PathBuf },
    Global { home: PathBuf },
}

#[derive(Clone, Debug)]
pub struct McpInstall {
    pub command: PathBuf,
    pub data_dir: PathBuf,
    pub home: PathBuf,
}

pub fn setup(scope: SetupScope, mcp: &McpInstall) -> Result<Vec<String>, Error> {
    let mut lines = Vec::new();
    match &scope {
        SetupScope::Project { root } => {
            install_pair(&root.join(".agents/skills"), &mut lines)?;
            link_claude_skills(&root.join(".claude/skills"), &mut lines)?;
            ensure_pointer(&root.join("AGENTS.md"), PROJECT_POINTER, &mut lines)?;
            ensure_claude_md(root, &mut lines)?;
        }
        SetupScope::Global { home } => {
            install_pair(&home.join(".agents/skills"), &mut lines)?;
            link_claude_skills(&home.join(".claude/skills"), &mut lines)?;
            ensure_pointer(&home.join(".claude/CLAUDE.md"), GLOBAL_POINTER, &mut lines)?;
        }
    }
    register_mcp(&scope, mcp, &mut lines)?;
    Ok(lines)
}

fn register_mcp(
    scope: &SetupScope,
    mcp: &McpInstall,
    lines: &mut Vec<String>,
) -> Result<(), Error> {
    let entry = serde_json::json!({
        "command": mcp.command.display().to_string(),
        "args": ["run", "--data", mcp.data_dir.display().to_string()]
    });
    let project_root = match scope {
        SetupScope::Project { root } => Some(root.as_path()),
        SetupScope::Global { .. } => None,
    };
    let mut found = false;
    if cursor_present(&mcp.home, project_root) {
        found = true;
        let path = match scope {
            SetupScope::Project { root } => root.join(".cursor/mcp.json"),
            SetupScope::Global { home } => home.join(".cursor/mcp.json"),
        };
        upsert_server(&path, &entry, lines)?;
    }
    if claude_code_present(&mcp.home, project_root) {
        found = true;
        let path = match scope {
            SetupScope::Project { root } => root.join(".mcp.json"),
            SetupScope::Global { home } => home.join(".claude.json"),
        };
        upsert_server(&path, &entry, lines)?;
    }
    if let Some(path) = desktop_config(&mcp.home) {
        found = true;
        upsert_server(&path, &entry, lines)?;
    }
    if let Some(path) = grok_config(scope, &mcp.home) {
        found = true;
        upsert_grok(&path, mcp, lines)?;
    }
    if !found {
        lines.push("no harness config found".into());
    }
    Ok(())
}

fn cursor_present(home: &Path, project: Option<&Path>) -> bool {
    home.join(".cursor").is_dir() || project.is_some_and(|root| root.join(".cursor").is_dir())
}

fn claude_code_present(home: &Path, project: Option<&Path>) -> bool {
    home.join(".claude").is_dir()
        || home.join(".claude.json").is_file()
        || project
            .is_some_and(|root| root.join(".claude").is_dir() || root.join(".mcp.json").is_file())
}

fn grok_config(scope: &SetupScope, home: &Path) -> Option<PathBuf> {
    let path = match scope {
        SetupScope::Project { root } => root.join(".grok/config.toml"),
        SetupScope::Global { .. } => home.join(".grok/config.toml"),
    };
    let dir = path.parent()?;
    if dir.is_dir() { Some(path) } else { None }
}

fn upsert_grok(path: &Path, mcp: &McpInstall, lines: &mut Vec<String>) -> Result<(), Error> {
    let block = format!(
        "[mcp_servers.mneme]\ncommand = \"{}\"\nargs = [\"run\", \"--data\", \"{}\"]\nenabled = true\nstartup_timeout_sec = 180\n",
        mcp.command.display(),
        mcp.data_dir.display()
    );
    if path.is_file() {
        let current = fs::read_to_string(path)?;
        if let Some(section) = current.split("[mcp_servers.mneme]").nth(1) {
            let body = section.split("\n[").next().unwrap_or(section);
            let same = body.contains(&mcp.command.display().to_string())
                && body.contains(&mcp.data_dir.display().to_string());
            if same {
                lines.push(format!("kept mneme in {}", path.display()));
            } else {
                lines.push(format!(
                    "left mneme in {} because it differs",
                    path.display()
                ));
            }
            return Ok(());
        }
        let mut next = current;
        if !next.ends_with('\n') {
            next.push('\n');
        }
        next.push('\n');
        next.push_str(&block);
        fs::write(path, next)?;
    } else {
        fs::write(path, block)?;
    }
    lines.push(format!("added mneme to {}", path.display()));
    Ok(())
}

fn desktop_config(home: &Path) -> Option<PathBuf> {
    let candidates = [
        home.join("Library/Application Support/Claude/claude_desktop_config.json"),
        home.join(".config/Claude/claude_desktop_config.json"),
    ];
    candidates
        .into_iter()
        .find(|path| path.parent().is_some_and(|parent| parent.is_dir()))
}

fn upsert_server(
    path: &Path,
    entry: &serde_json::Value,
    lines: &mut Vec<String>,
) -> Result<(), Error> {
    let mut root = if path.is_file() {
        let text = fs::read_to_string(path)?;
        match serde_json::from_str::<serde_json::Value>(&text) {
            Ok(value) if value.is_object() => value,
            Ok(_) => {
                lines.push(format!(
                    "left {} because it is not a json object",
                    path.display()
                ));
                return Ok(());
            }
            Err(_) => {
                lines.push(format!("left {} because it is not json", path.display()));
                return Ok(());
            }
        }
    } else {
        serde_json::json!({})
    };
    let Some(object) = root.as_object_mut() else {
        return Ok(());
    };
    let servers = object
        .entry("mcpServers")
        .or_insert_with(|| serde_json::json!({}));
    if !servers.is_object() {
        lines.push(format!(
            "left {} because mcpServers is not an object",
            path.display()
        ));
        return Ok(());
    }
    let Some(map) = servers.as_object_mut() else {
        return Ok(());
    };
    if let Some(existing) = map.get("mneme") {
        if existing == entry {
            lines.push(format!("kept mneme in {}", path.display()));
        } else {
            lines.push(format!(
                "left mneme in {} because it differs",
                path.display()
            ));
        }
        return Ok(());
    }
    map.insert("mneme".into(), entry.clone());
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let body = serde_json::to_string_pretty(&root).map_err(|err| Error::Store(err.to_string()))?;
    fs::write(path, format!("{body}\n"))?;
    lines.push(format!("added mneme to {}", path.display()));
    Ok(())
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

    fn bare_mcp(home: &Path) -> McpInstall {
        McpInstall {
            command: PathBuf::from("/usr/local/bin/mneme"),
            data_dir: PathBuf::from("brains/personal"),
            home: home.to_path_buf(),
        }
    }

    #[test]
    fn project_setup_is_idempotent_and_keeps_edits() {
        let dir = TempDir::new();
        let home = TempDir::new();
        let first = setup(
            SetupScope::Project {
                root: dir.0.clone(),
            },
            &bare_mcp(&home.0),
        )
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

        let second = setup(
            SetupScope::Project {
                root: dir.0.clone(),
            },
            &bare_mcp(&home.0),
        )
        .unwrap();
        assert!(
            second.iter().all(|line| {
                line.starts_with("kept ")
                    || line.contains("already")
                    || line == "no harness config found"
            }),
            "second setup changes nothing: {second:?}"
        );
        let agents_again = fs::read_to_string(dir.0.join("AGENTS.md")).unwrap();
        assert_eq!(agents_again.matches(PROJECT_POINTER).count(), 1);

        fs::write(dir.0.join(".agents/skills/mneme/SKILL.md"), "local edit\n").unwrap();
        let third = setup(
            SetupScope::Project {
                root: dir.0.clone(),
            },
            &bare_mcp(&home.0),
        )
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
        let home = TempDir::new();
        let mcp = bare_mcp(&home.0);
        setup(
            SetupScope::Project {
                root: dir.0.clone(),
            },
            &mcp,
        )
        .unwrap();
        setup(
            SetupScope::Project {
                root: dir.0.clone(),
            },
            &mcp,
        )
        .unwrap();
        let claude = fs::read_to_string(dir.0.join("CLAUDE.md")).unwrap();
        assert!(claude.starts_with("# Notes\n"), "existing text stays");
        assert_eq!(claude.matches(PROJECT_POINTER).count(), 1);
    }

    #[test]
    fn global_setup_writes_under_the_home_directory() {
        let home = TempDir::new();
        let lines = setup(
            SetupScope::Global {
                home: home.0.clone(),
            },
            &bare_mcp(&home.0),
        )
        .unwrap();
        assert!(
            home.0.join(".agents/skills/mneme-dream/SKILL.md").is_file(),
            "global dream skill is installed: {lines:?}"
        );
        let memory = fs::read_to_string(home.0.join(".claude/CLAUDE.md")).unwrap();
        assert!(memory.contains("~/.agents/skills/mneme/SKILL.md"));
    }

    #[test]
    fn setup_adds_mneme_to_each_installed_harness() {
        let home = TempDir::new();
        let root = TempDir::new();
        fs::create_dir_all(home.0.join(".cursor")).unwrap();
        fs::create_dir_all(home.0.join(".claude")).unwrap();
        fs::create_dir_all(home.0.join("Library/Application Support/Claude")).unwrap();
        fs::write(
            home.0.join(".cursor/mcp.json"),
            "{\n  \"mcpServers\": {\n    \"other\": { \"command\": \"other\" }\n  }\n}\n",
        )
        .unwrap();
        let mcp = bare_mcp(&home.0);
        let lines = setup(
            SetupScope::Global {
                home: home.0.clone(),
            },
            &mcp,
        )
        .unwrap();
        let cursor: serde_json::Value =
            serde_json::from_str(&fs::read_to_string(home.0.join(".cursor/mcp.json")).unwrap())
                .unwrap();
        assert_eq!(cursor["mcpServers"]["other"]["command"], "other");
        assert_eq!(
            cursor["mcpServers"]["mneme"]["args"][2], "brains/personal",
            "cursor keeps the other server and gains mneme: {lines:?}"
        );
        let claude: serde_json::Value =
            serde_json::from_str(&fs::read_to_string(home.0.join(".claude.json")).unwrap())
                .unwrap();
        assert_eq!(claude["mcpServers"]["mneme"]["args"][0], "run");
        let desktop = fs::read_to_string(
            home.0
                .join("Library/Application Support/Claude/claude_desktop_config.json"),
        )
        .unwrap();
        assert!(desktop.contains("\"mneme\""), "desktop config gains mneme");

        let again = setup(
            SetupScope::Global {
                home: home.0.clone(),
            },
            &mcp,
        )
        .unwrap();
        assert!(
            again.iter().any(|line| line.starts_with("kept mneme in ")),
            "the same server is kept: {again:?}"
        );

        let project = setup(
            SetupScope::Project {
                root: root.0.clone(),
            },
            &mcp,
        )
        .unwrap();
        assert!(root.0.join(".cursor/mcp.json").is_file(), "{project:?}");
        assert!(root.0.join(".mcp.json").is_file(), "{project:?}");
    }

    #[test]
    fn global_setup_appends_mneme_to_grok_config() {
        let home = TempDir::new();
        fs::create_dir_all(home.0.join(".grok")).unwrap();
        fs::write(
            home.0.join(".grok/config.toml"),
            "[mcp_servers.blender]\ncommand = \"uvx\"\n",
        )
        .unwrap();
        let mcp = bare_mcp(&home.0);
        setup(
            SetupScope::Global {
                home: home.0.clone(),
            },
            &mcp,
        )
        .unwrap();
        let text = fs::read_to_string(home.0.join(".grok/config.toml")).unwrap();
        assert!(text.contains("[mcp_servers.blender]"), "blender stays");
        assert!(
            text.contains("[mcp_servers.mneme]"),
            "grok config gains mneme: {text}"
        );
        assert!(text.contains("brains/personal"));
        setup(
            SetupScope::Global {
                home: home.0.clone(),
            },
            &mcp,
        )
        .unwrap();
        let again = fs::read_to_string(home.0.join(".grok/config.toml")).unwrap();
        assert_eq!(again.matches("[mcp_servers.mneme]").count(), 1);
    }
}
