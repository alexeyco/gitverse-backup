# Proposal: back up GitVerse repositories to Yandex Disk

## Why

The `gitverse-backup` repository is a greenfield project. The user wants a small Rust service that creates scheduled offsite archives of all Git repositories returned for a GitVerse token.

## What Changes

- Add a single Rust service that starts a backup cycle immediately and then follows a five-field cron schedule in a required IANA timezone. Skip scheduled occurrences while a cycle is still running.
- List all repositories returned by GitVerse across all API pages. Create one full bare Git mirror per repository, then package all mirrors into one Deflate level 9 ZIP for the cycle. ZIP entries use `{owner}/{repo}/`; Git LFS is excluded.
- Store the cycle archive at `disk:/backups/gitverse/<UTC timestamp>.zip`, splitting archives larger than 1,000,000,000 bytes into parts no larger than 1,000,000,000 bytes. Restore by joining the ordered parts and extracting the ZIP.
- Upload through the Yandex Disk REST API with OAuth, using staging before moving every archive object to its final timestamped name. Upload a completion sidecar last; only archives with that marker count as published.
- If any repository fails, continue processing the remaining repositories but do not publish an archive for the cycle. On successful publication, retain the newest configured number of complete archives and delete older complete archive files, parts, and markers. A failed cycle does not run retention.
- Require `BACKUP_RETENTION_COUNT` as a positive integer with no default. Configure GitVerse token, cron, timezone, Yandex OAuth token, and `disk:/` destination through environment variables and `.env` for local runs.
- Build a Docker image and publish it to `ghcr.io/alexeyco/gitverse-backup` through GitHub Actions. Write user-facing documentation in English, with a concise README and separate architecture guide.
- Configure Dependabot to check Cargo dependencies, GitHub Actions, and Docker base images weekly.

## Impact on Capabilities

- `repository-backup` — new capability: GitVerse discovery, cycle-wide bare mirror archives, cron scheduling, Yandex Disk REST publication, completion markers, retention, failure handling, and container distribution.

## Assumptions

ASSUMPTION: archive names use UTC `YYYY-MM-DDTHH-mm-ssZ`, and ZIP entries use `{owner}/{repo}/` paths.

ASSUMPTION: staging and a completion sidecar mark a multipart archive as published only after all parts are uploaded.

ASSUMPTION: ZIP uses Deflate level 9; each part is limited to 1,000,000,000 bytes inclusive (decimal 1 GB).

ASSUMPTION: `BACKUP_RETENTION_COUNT` means the number of most recent successfully completed archives; a failed cycle does not delete older archives.

## Rejected Risks

REJECTED-RISK: GitVerse documentation is inconsistent about whether `GET /user/repos` lists only owned repositories or all repositories visible through organizations and collaboration — the user defines scope as every repository returned by the API for the supplied token.
