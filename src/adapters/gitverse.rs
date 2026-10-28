use std::collections::HashSet;
use std::time::Duration;

pub use crate::models::Repository;
use crate::ports::RepositorySource;
use reqwest::Url;
use reqwest::blocking::Client;
use reqwest::header::{ACCEPT, LINK};

const API_REPOSITORIES_URL: &str = "https://api.gitverse.ru/user/repos";
const ACCEPT_VALUE: &str = "application/vnd.gitverse.object+json;version=1";
const GITVERSE_HOST: &str = "gitverse.ru";

impl Repository {
    pub fn owner_and_name(&self) -> Result<(&str, &str), GitverseError> {
        let mut components = self.full_name.split('/');
        let owner = components.next().unwrap_or_default();
        let name = components.next().unwrap_or_default();

        if owner.is_empty()
            || name.is_empty()
            || components.next().is_some()
            || [owner, name].iter().any(|component| {
                *component == "."
                    || *component == ".."
                    || component.contains(['/', '\\'])
                    || component.chars().any(char::is_control)
            })
        {
            return Err(GitverseError::InvalidRepositoryName(self.full_name.clone()));
        }

        let clone_url = Url::parse(&self.clone_url)
            .map_err(|_| GitverseError::InvalidCloneUrl(self.full_name.clone()))?;
        if clone_url.scheme() != "https"
            || clone_url.host_str() != Some(GITVERSE_HOST)
            || !clone_url.username().is_empty()
            || clone_url.password().is_some()
        {
            return Err(GitverseError::InvalidCloneUrl(self.full_name.clone()));
        }

        Ok((owner, name))
    }
}

pub struct GitverseClient {
    client: Client,
    token: String,
    base_url: Url,
}

impl GitverseClient {
    pub fn new(token: impl Into<String>) -> Result<Self, GitverseError> {
        Self::with_base_url(
            token,
            Url::parse(API_REPOSITORIES_URL).expect("constant GitVerse API URL is valid"),
        )
    }

    pub(crate) fn with_base_url(
        token: impl Into<String>,
        base_url: Url,
    ) -> Result<Self, GitverseError> {
        let client = Client::builder()
            .timeout(Duration::from_secs(30))
            .build()
            .map_err(GitverseError::Http)?;

        Ok(Self {
            client,
            token: token.into(),
            base_url,
        })
    }

    pub fn repositories(&self) -> Result<Vec<Repository>, GitverseError> {
        let mut next_url = Some(self.base_url.clone());
        let mut visited = HashSet::new();
        let mut repositories = Vec::new();

        while let Some(url) = next_url {
            // The token is attached to each request, so never follow pagination off-origin.
            if !same_origin(&self.base_url, &url) {
                return Err(GitverseError::CrossOriginPagination);
            }
            if !visited.insert(url.as_str().to_owned()) {
                return Err(GitverseError::PaginationLoop);
            }

            let response = self
                .client
                .get(url)
                .bearer_auth(&self.token)
                .header(ACCEPT, ACCEPT_VALUE)
                .send()
                .map_err(GitverseError::Http)?
                .error_for_status()
                .map_err(GitverseError::Http)?;

            // Capture the next page before consuming the response body.
            next_url = response
                .headers()
                .get(LINK)
                .and_then(|value| value.to_str().ok())
                .and_then(next_link)
                .map(|link| {
                    self.base_url
                        .join(link)
                        .map_err(|_| GitverseError::InvalidPaginationUrl)
                })
                .transpose()?;

            let page = response
                .json::<Vec<Repository>>()
                .map_err(GitverseError::Http)?;
            for repository in page {
                repository.owner_and_name()?;
                repositories.push(repository);
            }
        }

        Ok(repositories)
    }
}

impl RepositorySource for GitverseClient {
    fn repositories(&mut self) -> anyhow::Result<Vec<Repository>> {
        Ok(GitverseClient::repositories(self)?)
    }
}

#[derive(Debug, thiserror::Error)]
pub enum GitverseError {
    #[error("GitVerse API request failed")]
    Http(#[source] reqwest::Error),
    #[error("GitVerse returned an invalid repository name: {0}")]
    InvalidRepositoryName(String),
    #[error("GitVerse returned an invalid HTTPS clone URL for repository {0}")]
    InvalidCloneUrl(String),
    #[error("GitVerse pagination link is invalid")]
    InvalidPaginationUrl,
    #[error("GitVerse pagination link points to another origin")]
    CrossOriginPagination,
    #[error("GitVerse pagination contains a loop")]
    PaginationLoop,
}

fn same_origin(left: &Url, right: &Url) -> bool {
    left.scheme() == right.scheme()
        && left.host_str() == right.host_str()
        && left.port_or_known_default() == right.port_or_known_default()
}

fn next_link(header: &str) -> Option<&str> {
    header.split(',').find_map(|entry| {
        let mut parameters = entry.trim().split(';');
        let target = parameters.next()?.trim();

        let has_next_relation = parameters.any(|parameter| {
            parameter
                .trim()
                .strip_prefix("rel=")
                .map(|relation| {
                    relation
                        .trim_matches('"')
                        .split_whitespace()
                        .any(|r| r == "next")
                })
                .unwrap_or(false)
        });

        if has_next_relation {
            target.strip_prefix('<')?.strip_suffix('>')
        } else {
            None
        }
    })
}

#[cfg(test)]
mod tests {

    use super::*;
    use std::sync::mpsc;
    use std::thread;
    use tiny_http::{Header, Response, Server};

    fn mock_server(
        responses: Vec<(u16, Option<&'static str>, &'static str)>,
    ) -> (
        String,
        mpsc::Receiver<Option<String>>,
        thread::JoinHandle<()>,
    ) {
        let server = Server::http("127.0.0.1:0").unwrap();
        let address = server.server_addr().to_ip().unwrap();
        let base_url = format!("http://{address}/user/repos");
        let (sender, receiver) = mpsc::channel();

        let handle = thread::spawn(move || {
            for (status, link, body) in responses {
                let request = server.recv().unwrap();
                let authorization = request
                    .headers()
                    .iter()
                    .find(|header| header.field.equiv("Authorization"))
                    .map(|header| header.value.as_str().to_owned());
                sender.send(authorization).unwrap();

                let mut response = Response::from_string(body).with_status_code(status);
                if let Some(link) = link {
                    response = response.with_header(Header::from_bytes("Link", link).unwrap());
                }
                request.respond(response).unwrap();
            }
        });

        (base_url, receiver, handle)
    }

    #[test]
    fn follows_all_repository_pages_with_bearer_authentication() {
        let (base_url, authorizations, server) = mock_server(vec![
            (
                200,
                Some("</user/repos?page=2>; rel=\"next\""),
                r#"[{"full_name":"alice/one","clone_url":"https://gitverse.ru/alice/one.git"}]"#,
            ),
            (
                200,
                None,
                r#"[{"full_name":"team/two","clone_url":"https://gitverse.ru/team/two.git"}]"#,
            ),
        ]);
        let client =
            GitverseClient::with_base_url("test-token", reqwest::Url::parse(&base_url).unwrap())
                .unwrap();

        let repositories = client.repositories().unwrap();
        server.join().unwrap();

        assert_eq!(repositories.len(), 2);
        assert_eq!(repositories[0].full_name, "alice/one");
        assert_eq!(repositories[1].full_name, "team/two");
        assert_eq!(
            authorizations.recv().unwrap().as_deref(),
            Some("Bearer test-token")
        );
        assert_eq!(
            authorizations.recv().unwrap().as_deref(),
            Some("Bearer test-token")
        );
    }

    #[test]
    fn rejects_repository_listing_failure_without_exposing_the_token() {
        let (base_url, authorizations, server) = mock_server(vec![(401, None, "unauthorized")]);
        let client =
            GitverseClient::with_base_url("test-token", reqwest::Url::parse(&base_url).unwrap())
                .unwrap();

        let error = client.repositories().unwrap_err();
        server.join().unwrap();

        assert_eq!(
            authorizations.recv().unwrap().as_deref(),
            Some("Bearer test-token")
        );
        assert!(!error.to_string().contains("test-token"));
    }

    #[test]
    fn rejects_invalid_or_unsafe_repository_fields() {
        for full_name in ["", "alice", "alice/../private", "alice/team/repo"] {
            let repository = Repository {
                full_name: full_name.to_owned(),
                clone_url: "https://gitverse.ru/alice/repo.git".to_owned(),
            };
            assert!(repository.owner_and_name().is_err());
        }

        let repository = Repository {
            full_name: "alice/repo".to_owned(),
            clone_url: "https://token@gitverse.ru/alice/repo.git".to_owned(),
        };
        assert!(repository.owner_and_name().is_err());
    }
}
