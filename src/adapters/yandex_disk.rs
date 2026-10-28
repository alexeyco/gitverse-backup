use std::fs::File;
use std::num::NonZeroUsize;
use std::path::Path;
use std::time::Duration;

use reqwest::Url;
use reqwest::blocking::{Body, Client};
use reqwest::header::AUTHORIZATION;
use serde::Deserialize;

use crate::models::{ArchiveArtifacts, ArchivePackage};
use crate::ports::BackupDestination;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DiskDestination(String);

impl DiskDestination {
    pub fn parse(uri: &str) -> Result<Self, YandexDiskError> {
        let Some(path) = uri.strip_prefix("disk:/") else {
            return Err(YandexDiskError::InvalidDestination);
        };
        let path = path.trim_matches('/');
        if path.is_empty()
            || path.split('/').any(|part| {
                part.is_empty() || part == "." || part == ".." || part.chars().any(char::is_control)
            })
        {
            return Err(YandexDiskError::InvalidDestination);
        }
        Ok(Self(format!("disk:/{path}")))
    }

    pub fn child(&self, name: &str) -> Result<String, YandexDiskError> {
        if name.is_empty()
            || name == "."
            || name == ".."
            || name.contains('/')
            || name.contains('\\')
            || name.chars().any(char::is_control)
        {
            return Err(YandexDiskError::InvalidPath);
        }
        Ok(format!("{}/{}", self.0.trim_end_matches('/'), name))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

pub struct YandexDiskClient {
    client: Client,
    token: String,
    api_base: Url,
}

pub struct YandexDiskBackupDestination {
    client: YandexDiskClient,
    destination: DiskDestination,
}

impl YandexDiskBackupDestination {
    pub fn new(token: impl Into<String>, destination: &str) -> Result<Self, YandexDiskError> {
        let (client, destination) = YandexDiskClient::new(token, destination)?;
        Ok(Self {
            client,
            destination,
        })
    }
}

impl BackupDestination for YandexDiskBackupDestination {
    fn publish(&mut self, archive: &ArchivePackage, timestamp: &str) -> anyhow::Result<()> {
        Ok(publish_archive(
            &self.client,
            &self.destination,
            archive.artifacts(),
            timestamp,
        )?)
    }

    fn retain(&mut self, count: NonZeroUsize) -> anyhow::Result<()> {
        Ok(self
            .client
            .retain_completed_archives(&self.destination, count)?)
    }
}

impl YandexDiskClient {
    pub fn new(
        token: impl Into<String>,
        destination: &str,
    ) -> Result<(Self, DiskDestination), YandexDiskError> {
        let destination = DiskDestination::parse(destination)?;
        let api_base =
            Url::parse("https://cloud-api.yandex.net/v1/disk/").expect("constant API URL");
        Ok((Self::with_api_base(token, api_base)?, destination))
    }

    pub(crate) fn with_api_base(
        token: impl Into<String>,
        api_base: Url,
    ) -> Result<Self, YandexDiskError> {
        let client = Client::builder()
            .timeout(Duration::from_secs(120))
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .map_err(|_| YandexDiskError::Request)?;
        Ok(Self {
            client,
            token: token.into(),
            api_base,
        })
    }

    fn request(
        &self,
        method: reqwest::Method,
        endpoint: &str,
    ) -> reqwest::blocking::RequestBuilder {
        self.client
            .request(
                method,
                self.api_base.join(endpoint).expect("relative endpoint"),
            )
            .header(AUTHORIZATION, format!("OAuth {}", self.token))
    }

    pub fn ensure_directory(&self, path: &str) -> Result<(), YandexDiskError> {
        let response = self
            .request(reqwest::Method::PUT, "resources")
            .query(&[("path", path)])
            .send()
            .map_err(|_| YandexDiskError::Request)?;
        if response.status().is_success() || response.status() == reqwest::StatusCode::CONFLICT {
            Ok(())
        } else if response.status() == reqwest::StatusCode::NOT_FOUND {
            // Create parents recursively, then retry the requested directory.
            let Some((parent, _)) = path.rsplit_once('/') else {
                return Err(YandexDiskError::ApiStatus(404));
            };
            if parent == "disk:" {
                return Err(YandexDiskError::ApiStatus(404));
            }
            self.ensure_directory(parent)?;
            self.ensure_directory(path)
        } else {
            Err(YandexDiskError::ApiStatus(response.status().as_u16()))
        }
    }

    pub fn upload_file(&self, local: &Path, remote_path: &str) -> Result<(), YandexDiskError> {
        let size = std::fs::metadata(local)
            .map_err(|_| YandexDiskError::LocalFile)?
            .len();

        let link: UploadLink = self
            .request(reqwest::Method::GET, "resources/upload")
            .query(&[("path", remote_path), ("overwrite", "false")])
            .send()
            .map_err(|_| YandexDiskError::Request)?
            .error_for_status()
            .map_err(|e| YandexDiskError::ApiStatus(e.status().map(|s| s.as_u16()).unwrap_or(0)))?
            .json()
            .map_err(|_| YandexDiskError::InvalidResponse)?;
        let upload_url = Url::parse(&link.href).map_err(|_| YandexDiskError::InvalidResponse)?;

        // The upload URL is a temporary capability; send it without the OAuth header.
        if !matches!(upload_url.scheme(), "https" | "http") {
            return Err(YandexDiskError::InvalidResponse);
        }
        let file = File::open(local).map_err(|_| YandexDiskError::LocalFile)?;
        let response = self
            .client
            .put(upload_url)
            .body(Body::sized(file, size))
            .send()
            .map_err(|_| YandexDiskError::Upload)?;

        if response.status().is_success() {
            Ok(())
        } else {
            Err(YandexDiskError::UploadStatus(response.status().as_u16()))
        }
    }

    pub fn move_file(&self, from: &str, to: &str) -> Result<(), YandexDiskError> {
        let response = self
            .request(reqwest::Method::POST, "resources/move")
            .query(&[("from", from), ("path", to), ("overwrite", "false")])
            .send()
            .map_err(|_| YandexDiskError::Request)?;

        if response.status() == reqwest::StatusCode::ACCEPTED {
            let operation: OperationLink = response
                .json()
                .map_err(|_| YandexDiskError::InvalidResponse)?;
            self.wait_for_operation(&operation.href)
        } else if response.status().is_success() {
            Ok(())
        } else {
            Err(YandexDiskError::ApiStatus(response.status().as_u16()))
        }
    }

    pub fn delete_path(&self, path: &str) -> Result<(), YandexDiskError> {
        let response = self
            .request(reqwest::Method::DELETE, "resources")
            .query(&[("path", path), ("permanently", "true")])
            .send()
            .map_err(|_| YandexDiskError::Request)?;

        if response.status() == reqwest::StatusCode::NOT_FOUND || response.status().is_success() {
            return Ok(());
        }
        if response.status() == reqwest::StatusCode::ACCEPTED {
            let operation: OperationLink = response
                .json()
                .map_err(|_| YandexDiskError::InvalidResponse)?;
            return self.wait_for_operation(&operation.href);
        }
        Err(YandexDiskError::ApiStatus(response.status().as_u16()))
    }

    pub fn resource_exists(&self, path: &str) -> Result<bool, YandexDiskError> {
        let response = self
            .request(reqwest::Method::GET, "resources")
            .query(&[("path", path)])
            .send()
            .map_err(|_| YandexDiskError::Request)?;
        if response.status() == reqwest::StatusCode::NOT_FOUND {
            Ok(false)
        } else if response.status().is_success() {
            Ok(true)
        } else {
            Err(YandexDiskError::ApiStatus(response.status().as_u16()))
        }
    }

    fn wait_for_operation(&self, href: &str) -> Result<(), YandexDiskError> {
        let operation_url = Url::parse(href).map_err(|_| YandexDiskError::InvalidResponse)?;
        if operation_url.origin() != self.api_base.origin() {
            return Err(YandexDiskError::InvalidResponse);
        }
        for _ in 0..120 {
            let response = self
                .request_url(reqwest::Method::GET, operation_url.clone())
                .send()
                .map_err(|_| YandexDiskError::Request)?
                .error_for_status()
                .map_err(|e| {
                    YandexDiskError::ApiStatus(e.status().map(|s| s.as_u16()).unwrap_or(0))
                })?;
            let status: OperationStatus = response
                .json()
                .map_err(|_| YandexDiskError::InvalidResponse)?;
            match status.status.as_str() {
                "success" => return Ok(()),
                "failed" => return Err(YandexDiskError::OperationFailed),
                _ => std::thread::sleep(Duration::from_secs(1)),
            }
        }
        Err(YandexDiskError::OperationTimeout)
    }

    fn request_url(&self, method: reqwest::Method, url: Url) -> reqwest::blocking::RequestBuilder {
        self.client
            .request(method, url)
            .header(AUTHORIZATION, format!("OAuth {}", self.token))
    }

    pub fn retain_completed_archives(
        &self,
        destination: &DiskDestination,
        retention_count: NonZeroUsize,
    ) -> Result<(), YandexDiskError> {
        let mut offset = 0usize;
        let mut entries = Vec::new();

        loop {
            let listing: ResourceListing = self
                .request(reqwest::Method::GET, "resources")
                .query(&[("path", destination.as_str()), ("limit", "1000")])
                .query(&[("offset", offset)])
                .send()
                .map_err(|_| YandexDiskError::Request)?
                .error_for_status()
                .map_err(|e| {
                    YandexDiskError::ApiStatus(e.status().map(|s| s.as_u16()).unwrap_or(0))
                })?
                .json()
                .map_err(|_| YandexDiskError::InvalidResponse)?;
            let count = listing.embedded.items.len();
            entries.extend(listing.embedded.items);
            if count < 1000 {
                break;
            }
            offset += count;
        }

        let mut archives = entries
            .iter()
            .filter_map(|entry| {
                entry
                    .name
                    .strip_suffix(".complete.json")
                    .filter(|timestamp| valid_timestamp(timestamp))
                    .map(|timestamp| (timestamp.to_owned(), entry.path.clone()))
            })
            .collect::<Vec<_>>();
        archives.sort_by(|left, right| right.0.cmp(&left.0));

        for (timestamp, marker_path) in archives.into_iter().skip(retention_count.get()) {
            // Remove the completion marker first so interrupted cleanup never looks complete.
            self.delete_path(&marker_path)?;
            let payload_prefix = format!("{timestamp}.zip.part-");
            let mut payloads = entries
                .iter()
                .filter(|entry| {
                    entry.name == format!("{timestamp}.zip")
                        || entry.name.starts_with(&payload_prefix)
                })
                .map(|entry| entry.path.clone())
                .collect::<Vec<_>>();
            payloads.sort();
            for payload in payloads {
                self.delete_path(&payload)?;
            }
        }
        Ok(())
    }
}

pub(crate) fn publish_archive(
    disk: &YandexDiskClient,
    destination: &DiskDestination,
    artifacts: &ArchiveArtifacts,
    timestamp: &str,
) -> Result<(), YandexDiskError> {
    let staging_root = destination.child(".staging")?;
    let staging = format!("{staging_root}/{timestamp}");
    disk.ensure_directory(destination.as_str())?;
    disk.ensure_directory(&staging_root)?;
    disk.ensure_directory(&staging)?;
    let sidecar_name = format!("{timestamp}.complete.json");
    let sidecar_final = destination.child(&sidecar_name)?;

    let mut promoted = Vec::new();
    let result = (|| {
        let mut staged = Vec::new();

        // Upload all payloads to staging before exposing any final archive paths.
        for archive in &artifacts.archive_files {
            let staged_name = archive.name.clone();
            let staged_path = format!("{staging}/{staged_name}");
            let final_path = destination.child(&archive.name)?;
            disk.upload_file(&archive.path, &staged_path)?;
            staged.push((staged_path, final_path));
        }
        for (staged_path, final_path) in staged {
            disk.move_file(&staged_path, &final_path)?;
            promoted.push(final_path);
        }

        // Publish the marker last; its presence means every payload is in place.
        disk.upload_file(&artifacts.sidecar_path, &sidecar_final)?;
        Ok(())
    })();
    if result.is_err() {
        // Preserve final files if another attempt already published this cycle's marker.
        if matches!(disk.resource_exists(&sidecar_final), Ok(false)) {
            for path in promoted {
                let _ = disk.delete_path(&path);
            }
        }
        let _ = disk.delete_path(&staging);
        return result;
    }
    let _ = disk.delete_path(&staging);
    Ok(())
}

#[derive(Deserialize)]
struct ResourceListing {
    #[serde(rename = "_embedded")]
    embedded: ResourceItems,
}

#[derive(Deserialize)]
struct ResourceItems {
    items: Vec<ResourceEntry>,
}

#[derive(Deserialize)]
struct ResourceEntry {
    name: String,
    path: String,
}

fn valid_timestamp(value: &str) -> bool {
    let bytes = value.as_bytes();
    bytes.len() == 20
        && [4, 7, 13, 16].iter().all(|index| bytes[*index] == b'-')
        && bytes[10] == b'T'
        && bytes[19] == b'Z'
        && bytes.iter().enumerate().all(|(index, byte)| {
            matches!(index, 4 | 7 | 10 | 13 | 16 | 19) || byte.is_ascii_digit()
        })
}

#[derive(Deserialize)]
struct UploadLink {
    href: String,
}

#[derive(Deserialize)]
struct OperationLink {
    href: String,
}

#[derive(Deserialize)]
struct OperationStatus {
    status: String,
}

#[derive(Debug, thiserror::Error)]
pub enum YandexDiskError {
    #[error("invalid Yandex Disk destination URI")]
    InvalidDestination,
    #[error("invalid Yandex Disk path")]
    InvalidPath,
    #[error("Yandex Disk API request failed")]
    Request,
    #[error("Yandex Disk API returned status {0}")]
    ApiStatus(u16),
    #[error("Yandex Disk upload failed")]
    Upload,
    #[error("Yandex Disk upload returned status {0}")]
    UploadStatus(u16),
    #[error("Yandex Disk returned an invalid response")]
    InvalidResponse,
    #[error("could not access local archive file")]
    LocalFile,
    #[error("Yandex Disk operation failed")]
    OperationFailed,
    #[error("Yandex Disk operation did not finish in time")]
    OperationTimeout,
}

#[cfg(test)]
mod tests {

    use super::*;
    use crate::adapters::archive::{ArchiveArtifacts, ArchiveFile};
    use reqwest::Url;
    use std::fs;
    use std::num::NonZeroUsize;
    use std::thread;
    use tiny_http::{Header, Response, Server};

    fn retention_listing() -> &'static str {
        r#"{
            "_embedded": {
                "items": [
                    {
                        "name": "2026-10-23T00-00-00Z.complete.json",
                        "path": "disk:/backups/2026-10-23T00-00-00Z.complete.json"
                    },
                    {
                        "name": "2026-10-24T00-00-00Z.complete.json",
                        "path": "disk:/backups/2026-10-24T00-00-00Z.complete.json"
                    },
                    {
                        "name": "2026-10-23T00-00-00Z.zip",
                        "path": "disk:/backups/2026-10-23T00-00-00Z.zip"
                    }
                ]
            }
        }"#
    }

    #[test]
    fn upload_files_at_the_configured_destination_path() {
        let destination = DiskDestination::parse("disk:/backups/gitverse").unwrap();
        assert_eq!(
            destination.child("20261025.zip").unwrap(),
            "disk:/backups/gitverse/20261025.zip"
        );
        assert!(DiskDestination::parse("disk:/backups/../private").is_err());
    }

    #[test]
    fn authorize_api_upload_link_without_forwarding_oauth_to_upload_url() {
        let server = Server::http("127.0.0.1:0").unwrap();
        let address = server.server_addr().to_ip().unwrap();
        let base = Url::parse(&format!("http://{address}/v1/disk/")).unwrap();
        let upload_server = Server::http("127.0.0.1:0").unwrap();
        let upload_address = upload_server.server_addr().to_ip().unwrap();
        let upload_url = format!("http://{upload_address}/upload");
        let api_thread = thread::spawn(move || {
            let request = server.recv().unwrap();
            assert_eq!(request.method().as_str(), "GET");
            assert_eq!(
                request
                    .headers()
                    .iter()
                    .find(|h| h.field.equiv("Authorization"))
                    .unwrap()
                    .value
                    .as_str(),
                "OAuth secret-value"
            );
            request
                .respond(
                    Response::from_string(format!(r#"{{"href":"{upload_url}"}}"#))
                        .with_header("Content-Type: application/json".parse::<Header>().unwrap()),
                )
                .unwrap();
        });
        let upload_thread = thread::spawn(move || {
            let mut request = upload_server.recv().unwrap();
            let auth = request
                .headers()
                .iter()
                .find(|h| h.field.equiv("Authorization"))
                .map(|h| h.value.as_str().to_owned());
            let mut body = Vec::new();
            request.as_reader().read_to_end(&mut body).unwrap();
            request.respond(Response::empty(201)).unwrap();
            (auth, body)
        });
        let (client, _) = (
            YandexDiskClient::with_api_base("secret-value", base).unwrap(),
            DiskDestination::parse("disk:/backups").unwrap(),
        );
        let temp = tempfile::NamedTempFile::new().unwrap();
        fs::write(temp.path(), b"archive bytes").unwrap();
        client
            .upload_file(temp.path(), "disk:/backups/.staging/20261025/20261025.zip")
            .unwrap();
        api_thread.join().unwrap();
        let (authorization, body) = upload_thread.join().unwrap();
        assert!(authorization.is_none());
        assert_eq!(body, b"archive bytes");
    }

    #[test]
    fn creates_missing_destination_parents_before_the_requested_directory() {
        let server = Server::http("127.0.0.1:0").unwrap();
        let base = Url::parse(&format!("http://{}/v1/disk/", server.server_addr())).unwrap();
        let worker = thread::spawn(move || {
            let first = server.recv().unwrap();
            assert!(first.url().contains("disk%3A%2Fbackups%2Fgitverse"));
            first.respond(Response::empty(404)).unwrap();

            let parent = server.recv().unwrap();
            assert!(parent.url().contains("disk%3A%2Fbackups"));
            parent.respond(Response::empty(201)).unwrap();

            let retry = server.recv().unwrap();
            assert!(retry.url().contains("disk%3A%2Fbackups%2Fgitverse"));
            retry.respond(Response::empty(201)).unwrap();
        });
        let client = YandexDiskClient::with_api_base("token", base).unwrap();

        client.ensure_directory("disk:/backups/gitverse").unwrap();

        worker.join().unwrap();
    }

    #[test]
    fn waits_for_an_accepted_move_operation_before_returning() {
        let server = Server::http("127.0.0.1:0").unwrap();
        let base = Url::parse(&format!("http://{}/v1/disk/", server.server_addr())).unwrap();
        let operation_url = format!("http://{}/v1/disk/operations/123", server.server_addr());
        let worker = thread::spawn(move || {
            let request = server.recv().unwrap();
            assert!(request.url().starts_with("/v1/disk/resources/move?"));
            request
                .respond(
                    Response::from_string(format!(r#"{{"href":"{operation_url}"}}"#))
                        .with_header("Content-Type: application/json".parse::<Header>().unwrap())
                        .with_status_code(202),
                )
                .unwrap();

            let poll = server.recv().unwrap();
            assert_eq!(poll.url(), "/v1/disk/operations/123");
            assert!(
                poll.headers()
                    .iter()
                    .any(|header| header.field.equiv("Authorization")
                        && header.value.as_str() == "OAuth token")
            );
            poll.respond(
                Response::from_string(r#"{"status":"success"}"#)
                    .with_header("Content-Type: application/json".parse::<Header>().unwrap()),
            )
            .unwrap();
        });
        let client = YandexDiskClient::with_api_base("token", base).unwrap();

        client
            .move_file("disk:/backups/staged.zip", "disk:/backups/final.zip")
            .unwrap();

        worker.join().unwrap();
    }

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
    fn mark_archive_complete_only_after_final_payload_is_promoted() {
        let server = Server::http("127.0.0.1:0").unwrap();
        let base = Url::parse(&format!("http://{}/v1/disk/", server.server_addr())).unwrap();
        let upload = Server::http("127.0.0.1:0").unwrap();
        let upload_url = format!("http://{}/put", upload.server_addr());
        let worker = thread::spawn(move || {
            let mut sequence = Vec::new();
            for _ in 0..3 {
                let request = server.recv().unwrap();
                sequence.push(request.url().to_owned());
                request.respond(Response::empty(201)).unwrap();
            }
            for expected in ["20261025.zip", "20261025.complete.json"] {
                let request = server.recv().unwrap();
                assert!(request.url().contains(expected));
                request
                    .respond(
                        Response::from_string(format!(r#"{{"href":"{upload_url}"}}"#)).with_header(
                            "Content-Type: application/json".parse::<Header>().unwrap(),
                        ),
                    )
                    .unwrap();
                let request = upload.recv().unwrap();
                request.respond(Response::empty(201)).unwrap();
                if expected.ends_with(".zip") {
                    let request = server.recv().unwrap();
                    sequence.push(request.url().to_owned());
                    request.respond(Response::empty(201)).unwrap();
                }
            }
            server
                .recv()
                .unwrap()
                .respond(Response::empty(204))
                .unwrap();
            sequence
        });
        let client = YandexDiskClient::with_api_base("token", base).unwrap();
        let destination = DiskDestination::parse("disk:/backups/gitverse").unwrap();
        let directory = tempfile::tempdir().unwrap();
        publish_archive(
            &client,
            &destination,
            &artifacts(directory.path()),
            "20261025",
        )
        .unwrap();
        let sequence = worker.join().unwrap();
        assert_eq!(sequence.len(), 4);
        assert!(sequence.last().unwrap().contains("20261025.zip"));
    }

    #[test]
    fn failed_sidecar_upload_reports_failure_and_preserves_completed_archive() {
        let server = Server::http("127.0.0.1:0").unwrap();
        let base = Url::parse(&format!("http://{}/v1/disk/", server.server_addr())).unwrap();
        let upload = Server::http("127.0.0.1:0").unwrap();
        let upload_url = format!("http://{}/put", upload.server_addr());
        let worker = thread::spawn(move || {
            for _ in 0..3 {
                let request = server.recv().unwrap();
                request.respond(Response::empty(201)).unwrap();
            }
            let request = server.recv().unwrap();
            request
                .respond(
                    Response::from_string(format!(r#"{{"href":"{upload_url}"}}"#))
                        .with_header("Content-Type: application/json".parse::<Header>().unwrap()),
                )
                .unwrap();
            upload
                .recv()
                .unwrap()
                .respond(Response::empty(201))
                .unwrap();
            let request = server.recv().unwrap();
            request.respond(Response::empty(201)).unwrap();
            let request = server.recv().unwrap();
            request
                .respond(Response::from_string("failed").with_status_code(500))
                .unwrap();
            let marker_check = server.recv().unwrap();
            assert!(marker_check.url().contains("complete.json"));
            marker_check.respond(Response::empty(404)).unwrap();
            // The missing marker means this attempt may remove its promoted payload.
            for _ in 0..2 {
                let request = server.recv().unwrap();
                assert!(request.url().contains("path="));
                request.respond(Response::empty(204)).unwrap();
            }
        });
        let client = YandexDiskClient::with_api_base("token", base).unwrap();
        let destination = DiskDestination::parse("disk:/backups/gitverse").unwrap();
        let directory = tempfile::tempdir().unwrap();
        let result = publish_archive(
            &client,
            &destination,
            &artifacts(directory.path()),
            "20261025",
        );
        assert!(result.is_err());
        worker.join().unwrap();
    }

    #[test]
    fn cleanup_does_not_delete_payloads_when_a_completion_marker_exists() {
        let server = Server::http("127.0.0.1:0").unwrap();
        let base = Url::parse(&format!("http://{}/v1/disk/", server.server_addr())).unwrap();
        let upload = Server::http("127.0.0.1:0").unwrap();
        let upload_url = format!("http://{}/put", upload.server_addr());
        let worker = thread::spawn(move || {
            for _ in 0..3 {
                server
                    .recv()
                    .unwrap()
                    .respond(Response::empty(201))
                    .unwrap();
            }
            let request = server.recv().unwrap();
            request
                .respond(
                    Response::from_string(format!(r#"{{"href":"{upload_url}"}}"#))
                        .with_header("Content-Type: application/json".parse::<Header>().unwrap()),
                )
                .unwrap();
            upload
                .recv()
                .unwrap()
                .respond(Response::empty(201))
                .unwrap();
            server
                .recv()
                .unwrap()
                .respond(Response::empty(201))
                .unwrap();
            server
                .recv()
                .unwrap()
                .respond(Response::from_string("conflict").with_status_code(409))
                .unwrap();

            let marker_check = server.recv().unwrap();
            assert!(marker_check.url().contains("complete.json"));
            marker_check.respond(Response::empty(200)).unwrap();
            let staging_cleanup = server.recv().unwrap();
            assert!(staging_cleanup.url().contains(".staging"));
            staging_cleanup.respond(Response::empty(204)).unwrap();
        });
        let client = YandexDiskClient::with_api_base("token", base).unwrap();
        let destination = DiskDestination::parse("disk:/backups/gitverse").unwrap();
        let directory = tempfile::tempdir().unwrap();

        assert!(
            publish_archive(
                &client,
                &destination,
                &artifacts(directory.path()),
                "20261025",
            )
            .is_err()
        );
        worker.join().unwrap();
    }

    fn retention_server(
        items: &[(&str, &str)],
    ) -> (YandexDiskClient, thread::JoinHandle<Vec<String>>) {
        let server = Server::http("127.0.0.1:0").unwrap();
        let base = Url::parse(&format!("http://{}/v1/disk/", server.server_addr())).unwrap();
        let items = items
            .iter()
            .map(|(name, path)| format!(r#"{{"name":"{name}","path":"{path}"}}"#))
            .collect::<Vec<_>>()
            .join(",");
        let worker = thread::spawn(move || {
            let mut operations = Vec::new();
            let listing = server.recv().unwrap();
            operations.push(format!("GET {}", listing.url()));
            listing
                .respond(
                    Response::from_string(format!(r#"{{"_embedded":{{"items":[{items}]}}}}"#))
                        .with_header("Content-Type: application/json".parse::<Header>().unwrap()),
                )
                .unwrap();
            while let Ok(request) = server.recv() {
                operations.push(format!("DELETE {}", request.url()));
                request.respond(Response::empty(204)).unwrap();
                if operations.len() == 3 {
                    break;
                }
            }
            operations
        });
        (
            YandexDiskClient::with_api_base("token", base).unwrap(),
            worker,
        )
    }

    #[test]
    fn retention_deletes_old_marker_before_payload_and_preserves_recent_archives() {
        let (client, worker) = retention_server(&[
            (
                "2026-10-23T00-00-00Z.complete.json",
                "disk:/backups/2026-10-23T00-00-00Z.complete.json",
            ),
            (
                "2026-10-23T00-00-00Z.zip.part-0001",
                "disk:/backups/2026-10-23T00-00-00Z.zip.part-0001",
            ),
            (
                "2026-10-24T00-00-00Z.complete.json",
                "disk:/backups/2026-10-24T00-00-00Z.complete.json",
            ),
            (
                "2026-10-24T00-00-00Z.zip",
                "disk:/backups/2026-10-24T00-00-00Z.zip",
            ),
            (
                "2026-10-25T00-00-00Z.complete.json",
                "disk:/backups/2026-10-25T00-00-00Z.complete.json",
            ),
        ]);
        let destination = DiskDestination::parse("disk:/backups").unwrap();
        client
            .retain_completed_archives(&destination, NonZeroUsize::new(2).unwrap())
            .unwrap();
        let operations = worker.join().unwrap();
        assert_eq!(operations.len(), 3);
        assert!(operations[1].contains("complete.json"));
        assert!(operations[2].contains("zip.part-0001"));
    }

    #[test]
    fn retention_stops_before_payload_when_marker_delete_fails() {
        let server = Server::http("127.0.0.1:0").unwrap();
        let base = Url::parse(&format!("http://{}/v1/disk/", server.server_addr())).unwrap();
        let worker = thread::spawn(move || {
            server
                .recv()
                .unwrap()
                .respond(Response::from_string(retention_listing()))
                .unwrap();
            let marker_delete = server.recv().unwrap();
            assert!(marker_delete.url().contains("complete.json"));
            marker_delete
                .respond(Response::from_string("failed").with_status_code(500))
                .unwrap();
        });
        let client = YandexDiskClient::with_api_base("token", base).unwrap();
        let destination = DiskDestination::parse("disk:/backups").unwrap();
        assert!(
            client
                .retain_completed_archives(&destination, NonZeroUsize::new(1).unwrap())
                .is_err()
        );
        worker.join().unwrap();
    }

    #[test]
    fn retention_leaves_old_archive_unmarked_when_payload_delete_fails() {
        let server = Server::http("127.0.0.1:0").unwrap();
        let base = Url::parse(&format!("http://{}/v1/disk/", server.server_addr())).unwrap();
        let worker = thread::spawn(move || {
            server
                .recv()
                .unwrap()
                .respond(Response::from_string(retention_listing()))
                .unwrap();
            let marker_delete = server.recv().unwrap();
            assert!(marker_delete.url().contains("complete.json"));
            marker_delete.respond(Response::empty(204)).unwrap();
            let payload_delete = server.recv().unwrap();
            assert!(payload_delete.url().contains(".zip"));
            payload_delete
                .respond(Response::from_string("failed").with_status_code(500))
                .unwrap();
        });
        let client = YandexDiskClient::with_api_base("token", base).unwrap();
        let destination = DiskDestination::parse("disk:/backups").unwrap();
        assert!(
            client
                .retain_completed_archives(&destination, NonZeroUsize::new(1).unwrap())
                .is_err()
        );
        worker.join().unwrap();
    }
}
