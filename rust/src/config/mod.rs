pub mod global;
pub mod project;
pub mod remote;

use std::path::{Path, PathBuf};

/// Detected operating mode.
#[derive(Debug, Clone, PartialEq)]
pub enum Mode {
    /// CWD is inside a git repo with `.himitsu.yaml` at the repo root.
    Project {
        repo_root: PathBuf,
        config: project::ProjectConfig,
    },
    /// No project binding found; using global config.
    User,
}

/// Return the himitsu home directory, respecting `HIMITSU_HOME` env override.
pub fn himitsu_home() -> PathBuf {
    if let Ok(val) = std::env::var("HIMITSU_HOME") {
        return PathBuf::from(val);
    }
    dirs::home_dir()
        .expect("cannot determine home directory")
        .join(".himitsu")
}

/// Detect whether CWD (or an ancestor) is inside a project with `.himitsu.yaml`.
///
/// Walks from `start` upward looking for `.git`. If found and `.himitsu.yaml`
/// exists in the same directory, returns `Mode::Project`. Otherwise `Mode::User`.
pub fn detect_mode(start: &Path) -> Mode {
    let mut dir = start.to_path_buf();
    loop {
        if dir.join(".git").exists() {
            let config_path = dir.join(".himitsu.yaml");
            if config_path.exists() {
                if let Ok(cfg) = project::ProjectConfig::load(&config_path) {
                    return Mode::Project {
                        repo_root: dir,
                        config: cfg,
                    };
                }
            }
            // .git found but no valid .himitsu.yaml -> user mode
            return Mode::User;
        }
        if !dir.pop() {
            break;
        }
    }
    Mode::User
}

/// Resolve which remote to use given CLI flags, project config, and global config.
/// Returns the org/repo string (e.g. "myorg/secrets").
pub fn resolve_remote(
    cli_remote: &Option<String>,
    mode: &Mode,
    himitsu_home: &Path,
) -> crate::error::Result<String> {
    // 1. CLI -r flag takes precedence
    if let Some(r) = cli_remote {
        return Ok(r.clone());
    }

    // 2. Project mode uses the project config remote
    if let Mode::Project { config, .. } = mode {
        return Ok(config.remote.clone());
    }

    // 3. User mode reads default_remote from global config
    let global_config_path = himitsu_home.join("config.yaml");
    if global_config_path.exists() {
        let global_cfg = global::GlobalConfig::load(&global_config_path)?;
        if let Some(default) = global_cfg.default_remote {
            return Ok(default);
        }
    }

    Err(crate::error::HimitsuError::Remote(
        "no remote specified: use -r <org/repo>, set up a .himitsu.yaml, or configure default_remote"
            .into(),
    ))
}

/// Return the filesystem path for a remote given its org/repo reference.
pub fn remote_path(himitsu_home: &Path, remote_ref: &str) -> PathBuf {
    himitsu_home.join("data").join(remote_ref)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detect_mode_project_when_git_and_config_exist() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::create_dir(tmp.path().join(".git")).unwrap();
        std::fs::write(tmp.path().join(".himitsu.yaml"), "remote: test/repo\n").unwrap();

        match detect_mode(tmp.path()) {
            Mode::Project { config, .. } => {
                assert_eq!(config.remote, "test/repo");
            }
            Mode::User => panic!("expected project mode"),
        }
    }

    #[test]
    fn detect_mode_user_when_git_without_config() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::create_dir(tmp.path().join(".git")).unwrap();
        assert_eq!(detect_mode(tmp.path()), Mode::User);
    }

    #[test]
    fn detect_mode_user_when_no_git() {
        let tmp = tempfile::tempdir().unwrap();
        assert_eq!(detect_mode(tmp.path()), Mode::User);
    }
}
