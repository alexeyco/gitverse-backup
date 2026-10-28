use std::path::PathBuf;

use serde::Deserialize;
use tempfile::TempDir;

#[derive(Clone, Debug, Deserialize, Eq, PartialEq)]
/// A GitVerse repository that the configured token can access.
pub struct Repository {
    pub full_name: String,
    pub clone_url: String,
}

pub struct BareMirror {
    /// The repository path components used in the archive layout.
    pub owner: String,
    pub name: String,
    path: PathBuf,
    // Keep the temporary Git mirror alive while archive creation reads it.
    _workspace: TempDir,
}

impl BareMirror {
    pub(crate) fn new(owner: String, name: String, path: PathBuf, workspace: TempDir) -> Self {
        Self {
            owner,
            name,
            path,
            _workspace: workspace,
        }
    }

    pub fn path(&self) -> &std::path::Path {
        &self.path
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
/// A completed archive part ready for publication.
pub struct ArchiveFile {
    pub name: String,
    pub size_bytes: u64,
    pub path: PathBuf,
}

pub struct ArchiveArtifacts {
    /// Payload files must be published before the completion sidecar.
    pub archive_files: Vec<ArchiveFile>,
    pub sidecar_path: PathBuf,
}

/// Archive files and their temporary workspace live until publication completes.
pub struct ArchivePackage {
    _workspace: TempDir,
    artifacts: ArchiveArtifacts,
}

impl ArchivePackage {
    pub(crate) fn new(workspace: TempDir, artifacts: ArchiveArtifacts) -> Self {
        Self {
            _workspace: workspace,
            artifacts,
        }
    }

    pub fn artifacts(&self) -> &ArchiveArtifacts {
        &self.artifacts
    }
}
