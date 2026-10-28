use std::num::NonZeroUsize;

use crate::models::{ArchivePackage, BareMirror, Repository};

#[cfg_attr(test, mockall::automock)]
pub trait RepositorySource {
    fn repositories(&mut self) -> anyhow::Result<Vec<Repository>>;
}

#[cfg_attr(test, mockall::automock)]
pub trait RepositoryMirror {
    fn mirror(&mut self, repository: &Repository, token: &str) -> anyhow::Result<BareMirror>;
}

#[cfg_attr(test, mockall::automock)]
pub trait ArchiveBuilder {
    fn build(&mut self, mirrors: &[BareMirror], timestamp: &str) -> anyhow::Result<ArchivePackage>;
}

#[cfg_attr(test, mockall::automock)]
pub trait BackupDestination {
    fn publish(&mut self, archive: &ArchivePackage, timestamp: &str) -> anyhow::Result<()>;
    fn retain(&mut self, count: NonZeroUsize) -> anyhow::Result<()>;
}
