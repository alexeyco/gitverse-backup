use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Write};
use std::path::Path;

use serde::{Deserialize, Serialize};
use zip::write::SimpleFileOptions;
use zip::{CompressionMethod, ZipWriter};

pub use crate::models::{ArchiveArtifacts, ArchiveFile};
use crate::models::{ArchivePackage, BareMirror};
use crate::ports::ArchiveBuilder;

pub const MAX_PART_BYTES: u64 = 1_000_000_000;

#[derive(Default)]
pub struct ZipArchiveBuilder;

impl ArchiveBuilder for ZipArchiveBuilder {
    fn build(&mut self, mirrors: &[BareMirror], timestamp: &str) -> anyhow::Result<ArchivePackage> {
        let workspace = tempfile::tempdir()?;
        let artifacts = create_cycle_archive(mirrors, workspace.path(), timestamp)?;
        Ok(ArchivePackage::new(workspace, artifacts))
    }
}

#[derive(Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct CompletionSidecar {
    pub timestamp: String,
    pub repository_count: usize,
    pub files: Vec<SidecarFile>,
}

#[derive(Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct SidecarFile {
    pub name: String,
    pub size_bytes: u64,
}

pub fn create_cycle_archive(
    mirrors: &[BareMirror],
    output_dir: &Path,
    timestamp: &str,
) -> Result<ArchiveArtifacts, ArchiveError> {
    if !valid_timestamp(timestamp) {
        return Err(ArchiveError::InvalidTimestamp);
    }

    fs::create_dir_all(output_dir).map_err(ArchiveError::Io)?;
    let zip_path = output_dir.join(format!("{timestamp}.zip"));
    let mut writer = ZipWriter::new(create_new(&zip_path)?);

    let options = SimpleFileOptions::default()
        .compression_method(CompressionMethod::Deflated)
        .compression_level(Some(9));

    // Stable entry order keeps identical mirrors byte-for-byte comparable.
    let mut mirrors = mirrors.iter().collect::<Vec<_>>();
    mirrors.sort_by(|left, right| (&left.owner, &left.name).cmp(&(&right.owner, &right.name)));

    let mut seen = std::collections::HashSet::new();
    for mirror in &mirrors {
        if !valid_component(&mirror.owner) || !valid_component(&mirror.name) {
            return Err(ArchiveError::InvalidRepositoryPath);
        }
        if !seen.insert((mirror.owner.as_str(), mirror.name.as_str())) {
            return Err(ArchiveError::DuplicateRepository);
        }
        add_directory_files(
            &mut writer,
            mirror.path(),
            &format!("{}/{}/", mirror.owner, mirror.name),
            options,
        )?;
    }

    // Write the completion marker only after every payload file is finalized.
    writer.finish().map_err(ArchiveError::Zip)?;

    let archive_size = fs::metadata(&zip_path).map_err(ArchiveError::Io)?.len();
    let archive_files = if archive_size > MAX_PART_BYTES {
        let parts = split_file(&zip_path, output_dir, timestamp, MAX_PART_BYTES)?;
        fs::remove_file(&zip_path).map_err(ArchiveError::Io)?;
        parts
    } else {
        vec![ArchiveFile {
            name: format!("{timestamp}.zip"),
            size_bytes: archive_size,
            path: zip_path,
        }]
    };

    // Consumers treat this sidecar as the commit marker for the whole cycle.
    let sidecar = CompletionSidecar {
        timestamp: timestamp.to_owned(),
        repository_count: mirrors.len(),
        files: archive_files
            .iter()
            .map(|file| SidecarFile {
                name: file.name.clone(),
                size_bytes: file.size_bytes,
            })
            .collect(),
    };
    let sidecar_path = output_dir.join(format!("{timestamp}.complete.json"));
    let sidecar_bytes = serde_json::to_vec_pretty(&sidecar).map_err(ArchiveError::Json)?;
    let mut sidecar_file = create_new(&sidecar_path)?;

    sidecar_file
        .write_all(&sidecar_bytes)
        .map_err(ArchiveError::Io)?;
    sidecar_file.sync_all().map_err(ArchiveError::Io)?;

    Ok(ArchiveArtifacts {
        archive_files,
        sidecar_path,
    })
}

pub(crate) fn split_file(
    source: &Path,
    output_dir: &Path,
    timestamp: &str,
    max_part_bytes: u64,
) -> Result<Vec<ArchiveFile>, ArchiveError> {
    if max_part_bytes == 0 || !valid_timestamp(timestamp) {
        return Err(ArchiveError::InvalidPartLimit);
    }

    let source_size = fs::metadata(source).map_err(ArchiveError::Io)?.len();
    let mut input = File::open(source).map_err(ArchiveError::Io)?;
    let mut remaining = source_size;
    let mut parts = Vec::new();
    let mut index = 1usize;

    // Numbered parts are written in order so they can be concatenated directly.
    while remaining > 0 {
        let part_name = format!("{timestamp}.zip.part-{index:04}");
        let part_path = output_dir.join(&part_name);
        let mut output = create_new(&part_path)?;
        let requested = remaining.min(max_part_bytes);
        let copied = io::copy(&mut Read::by_ref(&mut input).take(requested), &mut output)
            .map_err(ArchiveError::Io)?;
        if copied != requested {
            return Err(ArchiveError::UnexpectedEndOfFile);
        }
        output.sync_all().map_err(ArchiveError::Io)?;
        parts.push(ArchiveFile {
            name: part_name,
            size_bytes: copied,
            path: part_path,
        });
        remaining -= copied;
        index += 1;
    }

    Ok(parts)
}

fn add_directory_files(
    writer: &mut ZipWriter<File>,
    directory: &Path,
    prefix: &str,
    options: SimpleFileOptions,
) -> Result<(), ArchiveError> {
    writer
        .add_directory(prefix, options)
        .map_err(ArchiveError::Zip)?;

    let mut entries = fs::read_dir(directory)
        .map_err(ArchiveError::Io)?
        .map(|entry| entry.map_err(ArchiveError::Io))
        .collect::<Result<Vec<_>, _>>()?;
    entries.sort_by_key(|entry| entry.file_name());

    for entry in entries {
        let path = entry.path();
        let file_type = entry.file_type().map_err(ArchiveError::Io)?;
        if file_type.is_symlink() {
            return Err(ArchiveError::SymlinkInMirror);
        }

        if file_type.is_dir() {
            add_directory_files(
                writer,
                &path,
                &format!("{}{}/", prefix, entry.file_name().to_string_lossy()),
                options,
            )?;
        } else if file_type.is_file() {
            let entry_name = format!("{}{}", prefix, entry.file_name().to_string_lossy());
            let mut file = File::open(path).map_err(ArchiveError::Io)?;
            writer
                .start_file(entry_name, options)
                .map_err(ArchiveError::Zip)?;
            io::copy(&mut file, writer).map_err(ArchiveError::Io)?;
        }
    }

    Ok(())
}

fn create_new(path: &Path) -> Result<File, ArchiveError> {
    OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .map_err(ArchiveError::Io)
}

fn valid_timestamp(value: &str) -> bool {
    !value.is_empty()
        && value
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || character == '-')
}

fn valid_component(value: &str) -> bool {
    !value.is_empty()
        && value != "."
        && value != ".."
        && !value.contains(['/', '\\'])
        && !value.chars().any(char::is_control)
}

#[derive(Debug, thiserror::Error)]
pub enum ArchiveError {
    #[error("invalid archive timestamp")]
    InvalidTimestamp,
    #[error("invalid archive part size limit")]
    InvalidPartLimit,
    #[error("repository path cannot be represented in an archive")]
    InvalidRepositoryPath,
    #[error("duplicate repository in cycle archive")]
    DuplicateRepository,
    #[error("bare mirror contains a symbolic link")]
    SymlinkInMirror,
    #[error("unexpected end of archive while splitting parts")]
    UnexpectedEndOfFile,
    #[error("archive filesystem operation failed")]
    Io(#[source] io::Error),
    #[error("ZIP archive operation failed")]
    Zip(#[source] zip::result::ZipError),
    #[error("completion sidecar serialization failed")]
    Json(#[source] serde_json::Error),
}

#[cfg(test)]
mod tests {

    use super::*;
    use std::io::Read;

    fn bare_mirror(owner: &str, name: &str) -> BareMirror {
        let workspace = tempfile::tempdir().unwrap();
        let path = workspace.path().join("repository.git");
        std::fs::create_dir(&path).unwrap();
        BareMirror::new(owner.to_owned(), name.to_owned(), path, workspace)
    }

    #[test]
    fn splits_archive_bytes_into_ordered_reconstructable_parts_within_limit() {
        let directory = tempfile::tempdir().unwrap();
        let source = directory.path().join("source.zip");
        let contents = (0..25).map(|value| value as u8).collect::<Vec<_>>();
        fs::write(&source, &contents).unwrap();

        let parts = split_file(&source, directory.path(), "20261025T120000Z", 7).unwrap();
        assert_eq!(
            parts
                .iter()
                .map(|part| part.name.as_str())
                .collect::<Vec<_>>(),
            [
                "20261025T120000Z.zip.part-0001",
                "20261025T120000Z.zip.part-0002",
                "20261025T120000Z.zip.part-0003",
                "20261025T120000Z.zip.part-0004",
            ]
        );
        assert!(parts.iter().all(|part| part.size_bytes <= 7));

        let mut reconstructed = Vec::new();
        for part in &parts {
            File::open(&part.path)
                .unwrap()
                .read_to_end(&mut reconstructed)
                .unwrap();
        }
        assert_eq!(reconstructed, contents);
    }

    #[test]
    fn rejects_invalid_archive_timestamps() {
        let result = create_cycle_archive(&[], Path::new("unused"), "../escape");
        assert!(matches!(result, Err(ArchiveError::InvalidTimestamp)));
    }

    #[test]
    fn rejects_duplicate_repository_paths() {
        let mirrors = [bare_mirror("owner", "repo"), bare_mirror("owner", "repo")];
        let output = tempfile::tempdir().unwrap();
        let result = create_cycle_archive(&mirrors, output.path(), "2026-10-28T00-00-00Z");
        assert!(matches!(result, Err(ArchiveError::DuplicateRepository)));
    }
}
