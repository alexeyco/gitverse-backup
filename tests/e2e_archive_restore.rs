#![expect(
    dead_code,
    reason = "The harness imports modules needed for the Git-to-ZIP restore flow."
)]
#![expect(
    unused_imports,
    reason = "Path imports also compile their unit test modules into this standalone harness."
)]

mod adapters {
    pub(crate) use crate::gitverse;
}
#[path = "../src/adapters/archive.rs"]
pub(crate) mod archive;
#[path = "../src/adapters/git.rs"]
pub(crate) mod git;
#[path = "../src/adapters/gitverse.rs"]
pub(crate) mod gitverse;
#[path = "../src/models.rs"]
pub(crate) mod models;
#[path = "../src/ports.rs"]
pub(crate) mod ports;

use std::fs::{self, File};
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

fn source_repository(root: &Path, name: &str, content: &str) -> std::path::PathBuf {
    let path = root.join(name);
    let output = Command::new("git")
        .args(["init", "--quiet", "--initial-branch=main"])
        .arg(&path)
        .output()
        .unwrap();
    assert!(output.status.success());
    successful_git(&path, &["config", "user.name", "Archive Test"]);
    successful_git(
        &path,
        &["config", "user.email", "archive-test@example.invalid"],
    );
    fs::write(path.join("data.txt"), content).unwrap();
    successful_git(&path, &["add", "data.txt"]);
    successful_git(&path, &["commit", "--quiet", "-m", "initial"]);
    successful_git(&path, &["tag", "v1"]);
    path
}

fn mirror(root: &Path, owner: &str, name: &str, content: &str) -> git::BareMirror {
    let source = source_repository(root, name, content);
    git::mirror_url(owner, name, &source.to_string_lossy(), "test-token").unwrap()
}

fn main() {
    let sources = tempfile::tempdir().unwrap();
    let first = mirror(sources.path(), "alice", "one", "one\n");
    let second = mirror(sources.path(), "team", "two", "two\n");
    let output = tempfile::tempdir().unwrap();

    let artifacts =
        archive::create_cycle_archive(&[first, second], output.path(), "20261025T120000Z").unwrap();
    assert_eq!(artifacts.archive_files.len(), 1);
    let archive_file = &artifacts.archive_files[0];
    let mut zip = zip::ZipArchive::new(File::open(&archive_file.path).unwrap()).unwrap();
    assert!(zip.len() >= 2);
    assert!(zip.file_names().any(|name| name.starts_with("alice/one/")));
    assert!(zip.file_names().any(|name| name.starts_with("team/two/")));

    let sidecar: archive::CompletionSidecar =
        serde_json::from_slice(&fs::read(&artifacts.sidecar_path).unwrap()).unwrap();
    assert_eq!(sidecar.repository_count, 2);
    assert_eq!(sidecar.files.len(), 1);
    assert_eq!(sidecar.files[0].size_bytes, archive_file.size_bytes);

    let restored = tempfile::tempdir().unwrap();
    for index in 0..zip.len() {
        let mut entry = zip.by_index(index).unwrap();
        let destination = restored.path().join(entry.name());
        if entry.is_dir() {
            fs::create_dir_all(destination).unwrap();
        } else {
            fs::create_dir_all(destination.parent().unwrap()).unwrap();
            let mut file = File::create(destination).unwrap();
            std::io::copy(&mut entry, &mut file).unwrap();
        }
    }

    for (owner, name, expected_content) in [("alice", "one", "one\n"), ("team", "two", "two\n")] {
        let repository = restored.path().join(owner).join(name);
        assert_eq!(
            successful_git(&repository, &["rev-parse", "--is-bare-repository"]),
            "true"
        );
        assert!(successful_git(&repository, &["show-ref", "--tags"]).contains("refs/tags/v1"));
        let worktree = tempfile::tempdir().unwrap();
        let clone = Command::new("git")
            .args([
                "clone",
                "--quiet",
                "--",
                &repository.to_string_lossy(),
                "checkout",
            ])
            .current_dir(worktree.path())
            .output()
            .unwrap();
        assert!(
            clone.status.success(),
            "{}",
            String::from_utf8_lossy(&clone.stderr)
        );
        assert_eq!(
            fs::read(worktree.path().join("checkout/data.txt")).unwrap(),
            expected_content.as_bytes()
        );
    }
}
