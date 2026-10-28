use std::num::NonZeroUsize;

use crate::ports::{ArchiveBuilder, BackupDestination, RepositoryMirror, RepositorySource};

pub fn run_cycle_with<C, M, A, D>(
    source: &mut C,
    mirror: &mut M,
    archive_builder: &mut A,
    destination: &mut D,
    gitverse_token: &str,
    timestamp: &str,
    retention_count: NonZeroUsize,
) -> Result<(), BackupError>
where
    C: RepositorySource,
    M: RepositoryMirror,
    A: ArchiveBuilder,
    D: BackupDestination,
{
    let repositories = source.repositories().map_err(BackupError::Gitverse)?;
    let mut mirrors = Vec::with_capacity(repositories.len());
    let mut failures = 0usize;

    // Try every repository in this cycle, then fail the cycle as a whole if any failed.
    for repository in &repositories {
        match mirror.mirror(repository, gitverse_token) {
            Ok(repository_mirror) => mirrors.push(repository_mirror),
            Err(_) => {
                failures += 1;
                eprintln!(
                    "event=repository_mirror_failed repository={}",
                    repository.full_name
                );
            }
        }
    }

    if failures != 0 {
        eprintln!("event=backup_cycle_failed repository_failures={failures}");
        return Err(BackupError::RepositoryFailures(failures));
    }

    let archive = archive_builder
        .build(&mirrors, timestamp)
        .map_err(BackupError::Archive)?;

    // The archive owns its own workspace; release mirror workspaces before publication.
    drop(mirrors);

    destination
        .publish(&archive, timestamp)
        .map_err(BackupError::YandexDisk)?;

    destination
        .retain(retention_count)
        .map_err(BackupError::Retention)?;

    eprintln!(
        "event=backup_cycle_completed repositories={} archive_files={}",
        repositories.len(),
        archive.artifacts().archive_files.len()
    );
    Ok(())
}

#[derive(Debug, thiserror::Error)]
pub enum BackupError {
    #[error("GitVerse repository listing failed")]
    Gitverse(#[source] anyhow::Error),
    #[error("{0} repositories could not be mirrored")]
    RepositoryFailures(usize),
    #[error("cycle archive creation failed")]
    Archive(#[source] anyhow::Error),
    #[error("Yandex Disk publication failed")]
    YandexDisk(#[source] anyhow::Error),
    #[error("Yandex Disk retention cleanup failed")]
    Retention(#[source] anyhow::Error),
}

#[cfg(test)]
mod tests {

    use super::*;
    use crate::adapters::archive::{ArchiveArtifacts, ArchiveFile};
    use crate::adapters::{git, gitverse};
    use crate::models::{ArchivePackage, BareMirror};
    use crate::ports;
    use std::fs;
    use std::num::NonZeroUsize;

    fn artifacts(directory: &std::path::Path) -> ArchiveArtifacts {
        let archive = directory.join("20261025.zip");
        let sidecar = directory.join("20261025.complete.json");
        fs::write(&archive, b"archive").unwrap();
        fs::write(&sidecar, b"{}").unwrap();
        ArchiveArtifacts {
            archive_files: vec![ArchiveFile {
                name: "20261025.zip".into(),
                size_bytes: 7,
                path: archive,
            }],
            sidecar_path: sidecar,
        }
    }

    #[test]
    fn failed_mirror_does_not_publish_cycle_and_remaining_repositories_are_attempted() {
        let repositories = vec![
            gitverse::Repository {
                full_name: "one/first".into(),
                clone_url: "https://gitverse.ru/one/first.git".into(),
            },
            gitverse::Repository {
                full_name: "two/second".into(),
                clone_url: "https://gitverse.ru/two/second.git".into(),
            },
        ];
        let attempted = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let recorded = attempted.clone();
        let mut source = ports::MockRepositorySource::new();
        source
            .expect_repositories()
            .return_once(|| Ok(repositories));
        let mut mirror = ports::MockRepositoryMirror::new();
        mirror
            .expect_mirror()
            .times(2)
            .returning(move |repository, _| {
                recorded.lock().unwrap().push(repository.full_name.clone());
                Err(anyhow::Error::new(git::GitError::InvalidRepositoryName))
            });
        let mut archive_builder = ports::MockArchiveBuilder::new();
        let mut destination = ports::MockBackupDestination::new();
        let result = run_cycle_with(
            &mut source,
            &mut mirror,
            &mut archive_builder,
            &mut destination,
            "token",
            "20261025T120000Z",
            NonZeroUsize::new(2).unwrap(),
        );
        assert!(matches!(result, Err(BackupError::RepositoryFailures(2))));
        assert_eq!(*attempted.lock().unwrap(), ["one/first", "two/second"]);
    }

    #[test]
    fn a_later_cycle_retries_a_repository_that_failed_previously() {
        let attempts = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let recorded = attempts.clone();
        let mut source = ports::MockRepositorySource::new();
        source.expect_repositories().times(2).returning(|| {
            Ok(vec![gitverse::Repository {
                full_name: "one/repository".into(),
                clone_url: "https://gitverse.ru/one/repository.git".into(),
            }])
        });
        let mut mirror = ports::MockRepositoryMirror::new();
        mirror.expect_mirror().times(2).returning(move |_, _| {
            recorded.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            Err(anyhow::Error::new(git::GitError::InvalidRepositoryName))
        });
        let mut archive_builder = ports::MockArchiveBuilder::new();
        let mut destination = ports::MockBackupDestination::new();
        for _cron_occurrence in 0..2 {
            let result = run_cycle_with(
                &mut source,
                &mut mirror,
                &mut archive_builder,
                &mut destination,
                "token",
                "20261025T120000Z",
                NonZeroUsize::new(2).unwrap(),
            );
            assert!(matches!(result, Err(BackupError::RepositoryFailures(1))));
        }
        assert_eq!(attempts.load(std::sync::atomic::Ordering::Relaxed), 2);
    }

    #[test]
    fn successful_cycle_runs_each_port_in_order() {
        let mut sequence = mockall::Sequence::new();
        let mut source = ports::MockRepositorySource::new();
        source
            .expect_repositories()
            .times(1)
            .in_sequence(&mut sequence)
            .return_once(|| {
                Ok(vec![gitverse::Repository {
                    full_name: "owner/repository".into(),
                    clone_url: "https://gitverse.ru/owner/repository.git".into(),
                }])
            });

        let mirror_workspace = tempfile::tempdir().unwrap();
        let mirror_path = mirror_workspace.path().join("repository.git");
        fs::create_dir(&mirror_path).unwrap();
        let repository_mirror = BareMirror::new(
            "owner".into(),
            "repository".into(),
            mirror_path,
            mirror_workspace,
        );
        let mut mirror = ports::MockRepositoryMirror::new();
        mirror
            .expect_mirror()
            .times(1)
            .in_sequence(&mut sequence)
            .return_once(|_, _| Ok(repository_mirror));

        let archive_workspace = tempfile::tempdir().unwrap();
        let archive_artifacts = artifacts(archive_workspace.path());
        let archive = ArchivePackage::new(archive_workspace, archive_artifacts);
        let mut archive_builder = ports::MockArchiveBuilder::new();
        archive_builder
            .expect_build()
            .times(1)
            .in_sequence(&mut sequence)
            .return_once(|_, _| Ok(archive));

        let mut destination = ports::MockBackupDestination::new();
        destination
            .expect_publish()
            .times(1)
            .withf(|_, timestamp| timestamp == "20261025T120000Z")
            .in_sequence(&mut sequence)
            .return_once(|_, _| Ok(()));
        destination
            .expect_retain()
            .times(1)
            .withf(|count| count.get() == 3)
            .in_sequence(&mut sequence)
            .return_once(|_| Ok(()));

        run_cycle_with(
            &mut source,
            &mut mirror,
            &mut archive_builder,
            &mut destination,
            "token",
            "20261025T120000Z",
            NonZeroUsize::new(3).unwrap(),
        )
        .unwrap();
    }
}
