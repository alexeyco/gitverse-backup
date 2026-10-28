# Repository Backup Specification

## Purpose

Provide scheduled, restorable cycle-wide archives on Yandex Disk of the Git repositories GitVerse returns for a configured token.

## Requirements

### Requirement: Cron schedule and service configuration

The service SHALL read `GITVERSE_TOKEN`, five-field `BACKUP_CRON`, IANA timezone `TZ`, `YANDEX_DISK_OAUTH_TOKEN`, `YANDEX_DISK_DESTINATION`, and a positive `BACKUP_RETENTION_COUNT` from the environment. It SHALL support `.env` for local development, provide no retention default, and validate the cron, timezone, and retention count before starting.

#### Scenario: Read valid service configuration

**WHEN** all required settings contain valid tokens, a five-field cron expression, an IANA timezone, a `disk:/` destination, and a positive retention count
**THEN** the service accepts the settings and uses them for GitVerse, scheduling, Yandex Disk, and retention

#### Scenario: Reject an invalid cron, timezone, or retention count

**WHEN** the cron expression is invalid, `TZ` is not a valid IANA timezone, or the retention count is absent or not a positive integer
**THEN** the service reports a configuration error and does not start backup cycles

### Requirement: Schedule backup cycles from cron settings

The service SHALL run one backup cycle immediately, then schedule cycles according to the configured cron expression and timezone. It SHALL NOT run cycles concurrently and SHALL skip a cron occurrence if a cycle is still active.

#### Scenario: Start with valid schedule configuration

**WHEN** the service has valid cron and timezone settings
**THEN** it starts a backup cycle immediately and schedules later cycles using those settings

#### Scenario: Skip a cron occurrence while a cycle is active

**WHEN** a backup cycle is still running at the next scheduled cron occurrence
**THEN** the service skips that occurrence and does not start a concurrent cycle

### Requirement: Discover every GitVerse repository returned for a token

The service SHALL authenticate to the GitVerse Public API with the configured token, request the authenticated user's repository list, and process every repository returned across all pages. The backup scope SHALL be defined by the API results for that token.

#### Scenario: Process multiple repository pages

**WHEN** GitVerse returns repository pages with a next-page link
**THEN** the service follows pagination and processes repositories from every returned page

#### Scenario: Process repositories returned by the token

**WHEN** the API returns repositories with HTTPS clone URLs and readable Git data
**THEN** the service creates a full bare mirror for each returned repository as input to the cycle archive

#### Scenario: Reject a repository listing failure

**WHEN** GitVerse rejects the token or a repository-list request fails
**THEN** the cycle is reported as unsuccessful and the service does not publish an archive for that cycle

### Requirement: Create one compressed archive for a complete backup cycle

The service SHALL create one full bare Git mirror per listed repository, containing branches, tags, and history but excluding Git LFS objects. It SHALL package all cycle mirrors into a single ZIP using Deflate level 9, with entries under `{owner}/{repo}/`. It SHALL split ZIP byte streams larger than 1,000,000,000 bytes into ordered parts, each no larger than 1,000,000,000 bytes, and SHALL provide metadata sufficient to reconstruct and restore the complete ZIP.

#### Scenario: Archive multiple repositories in one ZIP

**WHEN** one cycle discovers and successfully mirrors multiple repositories
**THEN** it creates one ZIP containing each repository's bare mirror under its `{owner}/{repo}/` entry path

#### Scenario: Do not publish an incomplete cycle

**WHEN** one or more repository mirrors fail but processing of the other repositories finishes
**THEN** the cycle is reported as unsuccessful and no archive for that cycle is published

#### Scenario: Split an archive that exceeds the file limit

**WHEN** the complete cycle ZIP exceeds 1,000,000,000 bytes
**THEN** the service splits it into ordered parts no larger than 1,000,000,000 bytes each and records the part order and sizes in its completion metadata

#### Scenario: Reconstruct and restore all cycle mirrors

**WHEN** an operator downloads all parts for a completed cycle and joins them in order
**THEN** the reconstructed ZIP extracts to all cycle bare mirrors, each usable as a Git repository with its refs and history

### Requirement: Publish complete cycle archives through the Yandex Disk REST API

The service SHALL upload cycle archives to the configured `disk:/` destination through the Yandex Disk REST API. It SHALL authenticate API requests with `Authorization: OAuth <token>`, PUT file data to returned upload URLs without forwarding OAuth, and SHALL NOT log OAuth tokens or returned upload URLs.

#### Scenario: Upload a cycle archive to its timestamped destination

**WHEN** a complete cycle archive is ready for destination `disk:/backups/gitverse`
**THEN** its final name is `disk:/backups/gitverse/<UTC timestamp>.zip`, or `<UTC timestamp>.zip.part-0001` and later ordered part names if split, and it is staged before promotion

#### Scenario: Authorize REST requests without forwarding OAuth to upload URLs

**WHEN** the service requests an upload URL and then uploads a file to that returned URL
**THEN** it sends `Authorization: OAuth <token>` to the Yandex Disk API request only, omits authorization from the subsequent PUT request, and logs neither the token nor the returned URL

#### Scenario: Upload files at the configured destination path

**WHEN** the configured destination is `disk:/backups/gitverse`
**THEN** staging files, final timestamped archive files or parts, and completion sidecars are written beneath that destination

### Requirement: Publish only complete cycle archives

The service SHALL stage every archive file before promoting it to its final timestamped name. It SHALL upload a completion sidecar only after every final archive object is present; only archives with a completion sidecar SHALL count as published.

#### Scenario: Mark an archive complete only after all final files exist

**WHEN** all staged archive files have been moved to their final timestamped names
**THEN** the service uploads the completion sidecar last and the cycle archive is considered published only after that upload succeeds

#### Scenario: Preserve completed archives after incomplete publication

**WHEN** staging upload or promotion fails before the completion sidecar is uploaded
**THEN** the service does not treat the new timestamp as published and does not delete any completed archive

#### Scenario: Report sidecar upload failure without publishing or pruning

**WHEN** all archive files have reached their final names but uploading the completion sidecar fails
**THEN** the new timestamp remains unpublished, the cycle reports failure, retention does not run, and all previously completed archives remain untouched

### Requirement: Clean incomplete archive artifacts safely

The service SHALL clean staging or incomplete artifacts only when doing so cannot delete a completed archive.

#### Scenario: Clean incomplete artifacts without deleting completed archives

**WHEN** stale staging or unmarked final artifacts are cleaned
**THEN** the cleanup leaves every archive with a completion sidecar untouched

### Requirement: Retain a configured number of completed archives

After a successfully published cycle archive, the service SHALL retain the `BACKUP_RETENTION_COUNT` most recent archives with completion sidecars and select only older complete archives for deletion.

#### Scenario: Prune older complete archives after successful publication

**WHEN** a new cycle archive is published and more than `BACKUP_RETENTION_COUNT` completed archives exist
**THEN** the service keeps the most recent configured number and deletes older completed archives, their parts, and their sidecars

### Requirement: Delete old archive markers before payload files

For each archive selected for deletion, the service SHALL delete its completion sidecar before deleting any archive file or part. If sidecar deletion fails, it SHALL NOT delete that archive's payload and SHALL report a cleanup failure. If payload deletion fails after sidecar deletion, the sidecar SHALL remain absent, the leftover files SHALL not be treated as complete, and the service SHALL report a cleanup failure.

#### Scenario: Keep retention safe when sidecar deletion fails

**WHEN** retention selects an old completed archive but cannot delete its completion sidecar
**THEN** the service does not delete that archive's files or parts, reports a cleanup failure, and leaves the retained complete archives unchanged

#### Scenario: Keep retention safe when archive-file deletion fails

**WHEN** retention deletes an old archive's sidecar but deletion of a file or part then fails
**THEN** the sidecar remains absent, leftover files are not considered complete, the service reports a cleanup failure, and retained complete archives remain unchanged

### Requirement: Run retention only after successful publication

The service SHALL NOT run retention after a failed backup cycle and SHALL NOT delete any retained complete archive.

#### Scenario: Do not prune archives after a failed cycle

**WHEN** a cycle fails before its completion sidecar is uploaded
**THEN** the service does not run retention and leaves previously completed archives unchanged

### Requirement: Isolate repository failures and report cycle results

The service SHALL continue processing other listed repositories when one repository fails. It SHALL report the cycle as unsuccessful if listing or any repository backup fails, emit structured error logs without credentials, keep running, and retry failed repositories at a later cron occurrence.

#### Scenario: Continue processing after a repository failure

**WHEN** mirroring or archiving one repository fails
**THEN** the service continues processing the remaining repositories and reports an unsuccessful cycle without publishing an archive

#### Scenario: Retry repositories at a later cron occurrence

**WHEN** a repository failed in one cycle and remains in a later GitVerse listing
**THEN** the service attempts it again at a later scheduled cycle

### Requirement: Distribute and document the service

The project SHALL build a Docker image for the service and publish it to `ghcr.io/alexeyco/gitverse-backup` through GitHub Actions. User-facing repository documentation SHALL be written in English and SHALL describe configuration, image use, archive layout, sidecar completion, part reconstruction, retention, and architecture.

#### Scenario: Publish a tagged image

**WHEN** a release tag is pushed to the GitHub repository
**THEN** the release workflow builds and publishes the service image to GHCR

#### Scenario: Follow the repository documentation

**WHEN** an operator reads the README and architecture guide
**THEN** both documents explain how to configure and run the service and how to identify, reconstruct, and restore a completed cycle archive
