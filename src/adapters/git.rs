use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::process::{Command, Output};

use crate::adapters::gitverse::GitverseError;
pub use crate::models::BareMirror;
use crate::models::Repository;
use crate::ports::RepositoryMirror;

const ASKPASS_SCRIPT: &str = r#"#!/bin/sh
case "$1" in
  *sername*) printf '%s\n' "$GITVERSE_TOKEN" ;;
  *assword*) printf '\n' ;;
  *) exit 1 ;;
esac
"#;

#[derive(Default)]
pub struct GitRepositoryMirror;

pub fn mirror_repository(
    repository: &Repository,
    gitverse_token: &str,
) -> Result<BareMirror, GitError> {
    let (owner, name) = repository
        .owner_and_name()
        .map_err(GitError::InvalidRepository)?;
    mirror_url(owner, name, &repository.clone_url, gitverse_token)
}

impl RepositoryMirror for GitRepositoryMirror {
    fn mirror(
        &mut self,
        repository: &Repository,
        gitverse_token: &str,
    ) -> anyhow::Result<BareMirror> {
        Ok(mirror_repository(repository, gitverse_token)?)
    }
}

pub(crate) fn mirror_url(
    owner: &str,
    name: &str,
    clone_url: &str,
    gitverse_token: &str,
) -> Result<BareMirror, GitError> {
    if !valid_component(owner) || !valid_component(name) {
        return Err(GitError::InvalidRepositoryName);
    }
    let workspace = tempfile::Builder::new()
        .prefix("gitverse-backup-")
        .tempdir()
        .map_err(GitError::Workspace)?;
    let askpass_path = workspace.path().join("git-askpass.sh");
    write_askpass(&askpass_path)?;
    let global_config_path = workspace.path().join("gitconfig");
    File::create(&global_config_path).map_err(GitError::Workspace)?;

    // Keep credentials out of the remote URL and ignore host-level Git helpers.
    let mirror_path = workspace.path().join("repository.git");
    let output = Command::new("git")
        .arg("-c")
        .arg("credential.helper=")
        .arg("clone")
        .arg("--quiet")
        .arg("--mirror")
        .arg("--")
        .arg(clone_url)
        .arg(&mirror_path)
        .env("GIT_ASKPASS", &askpass_path)
        .env("GITVERSE_TOKEN", gitverse_token)
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", &global_config_path)
        .output()
        .map_err(GitError::Spawn)?;

    ensure_success(output)?;

    Ok(BareMirror::new(
        owner.to_owned(),
        name.to_owned(),
        mirror_path,
        workspace,
    ))
}

fn valid_component(value: &str) -> bool {
    !value.is_empty()
        && value != "."
        && value != ".."
        && !value.contains(['/', '\\'])
        && !value.chars().any(char::is_control)
}

fn write_askpass(path: &std::path::Path) -> Result<(), GitError> {
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .map_err(GitError::Workspace)?;
    file.write_all(ASKPASS_SCRIPT.as_bytes())
        .map_err(GitError::Workspace)?;
    file.flush().map_err(GitError::Workspace)?;

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;

        let mut permissions = fs::metadata(path)
            .map_err(GitError::Workspace)?
            .permissions();
        permissions.set_mode(0o700);
        fs::set_permissions(path, permissions).map_err(GitError::Workspace)?;
    }

    Ok(())
}

fn ensure_success(output: Output) -> Result<(), GitError> {
    if output.status.success() {
        Ok(())
    } else {
        Err(GitError::CloneFailed(output.status.code()))
    }
}

#[derive(Debug, thiserror::Error)]
pub enum GitError {
    #[error("invalid GitVerse repository")]
    InvalidRepository(#[source] GitverseError),
    #[error("invalid repository path component")]
    InvalidRepositoryName,
    #[error("could not create temporary Git workspace")]
    Workspace(#[source] std::io::Error),
    #[error("could not start Git")]
    Spawn(#[source] std::io::Error),
    #[error("Git mirror clone failed with status {0:?}")]
    CloneFailed(Option<i32>),
}

#[cfg(test)]
mod tests {

    use super::*;
    use std::path::Path;
    use std::process::{Command, Output};

    fn git(directory: &Path, arguments: &[&str]) -> Output {
        Command::new("git")
            .arg("-C")
            .arg(directory)
            .args(arguments)
            .output()
            .unwrap()
    }

    fn successful_git(directory: &Path, arguments: &[&str]) -> String {
        let output = git(directory, arguments);
        assert!(
            output.status.success(),
            "git {arguments:?} failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8_lossy(&output.stdout).trim().to_owned()
    }

    fn create_source_repository(root: &Path, owner: &str, name: &str) -> std::path::PathBuf {
        let path = root.join(format!("{owner}-{name}"));
        let output = Command::new("git")
            .args(["init", "--quiet", "--initial-branch=main"])
            .arg(&path)
            .output()
            .unwrap();
        assert!(output.status.success());
        successful_git(&path, &["config", "user.name", "Backup Test"]);
        successful_git(
            &path,
            &["config", "user.email", "backup-test@example.invalid"],
        );
        fs::write(path.join("README.md"), format!("{owner}/{name}\n")).unwrap();
        successful_git(&path, &["add", "README.md"]);
        successful_git(&path, &["commit", "--quiet", "-m", "initial"]);
        successful_git(&path, &["tag", "v1"]);
        successful_git(&path, &["switch", "--quiet", "-c", "feature"]);
        fs::write(path.join("feature.txt"), "feature branch\n").unwrap();
        successful_git(&path, &["add", "feature.txt"]);
        successful_git(&path, &["commit", "--quiet", "-m", "feature"]);
        path
    }

    #[test]
    fn creates_a_bare_mirror_with_branches_tags_and_history_and_cleans_it_on_drop() {
        let source_root = tempfile::tempdir().unwrap();
        let source = create_source_repository(source_root.path(), "alice", "tool");
        let source_url = source.to_string_lossy().to_string();
        let mirror = mirror_url("alice", "tool", &source_url, "test-token").unwrap();
        let mirror_path = mirror.path().to_path_buf();

        assert_eq!(
            successful_git(mirror.path(), &["rev-parse", "--is-bare-repository"]),
            "true"
        );
        let refs = successful_git(mirror.path(), &["for-each-ref", "--format=%(refname)"]);
        assert!(refs.contains("refs/heads/main"));
        assert!(refs.contains("refs/heads/feature"));
        assert!(refs.contains("refs/tags/v1"));
        assert_eq!(
            successful_git(mirror.path(), &["rev-list", "--all", "--count"]),
            "2"
        );

        let remote_url = successful_git(mirror.path(), &["remote", "get-url", "origin"]);
        assert_eq!(remote_url, source_url);
        assert!(!remote_url.contains("test-token"));
        assert!(!mirror.path().join("lfs/objects").exists());

        drop(mirror);
        assert!(!mirror_path.exists());
    }

    #[test]
    fn rejects_unsafe_archive_path_components_before_creating_a_workspace() {
        assert!(mirror_url("alice", "../outside", "unused", "token").is_err());
    }
}
