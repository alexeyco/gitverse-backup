# Design: back up GitVerse repositories to Yandex Disk

## Approach

Implement one Rust binary with a composition root in `src/main.rs`. The backup use case depends on separate ports for repository discovery, mirroring, archive building, and backup destination operations. GitVerse, Git, ZIP, and Yandex Disk adapters implement those ports. Keep archive and repository workflow data in `src/models.rs`. Use Mockall mocks to test the use case and scheduler without concrete network, process, archive, or clock dependencies. Load `.env` for local use through Figment's environment provider, validate the required configuration once at startup, run the first cycle immediately, then schedule cycles from the five-field cron expression in the configured IANA timezone. Keep at most one cycle active and skip a cron occurrence that arrives while a cycle is running.

Use `GET https://api.gitverse.ru/user/repos` with bearer authentication and follow pagination links until the complete listing has been read. Treat the returned listing as the definition of token visibility. For each repository, use its HTTPS `clone_url` and `full_name`; derive owner and repository names from `full_name` for the ZIP entry path.

Create one isolated temporary bare Git mirror for each listed repository in the cycle. Preserve branches, tags, and history, and do not fetch Git LFS objects. Continue attempting the remaining repositories after a mirror failure. If the listing or any repository mirror fails, report the cycle as unsuccessful, discard temporary data, and do not create or upload a cycle archive. After every repository mirrors successfully, create exactly one ZIP containing all bare mirrors under `{owner}/{repo}/` entry paths. Use Deflate level 9. If the ZIP is over 1,000,000,000 bytes, split its byte stream into ordered parts each no larger than 1,000,000,000 bytes.

Name each archive with the cycle start time in UTC using `YYYY-MM-DDTHH-mm-ssZ`, for example `2026-10-25T18-30-00Z.zip`. A split archive uses `<timestamp>.zip.part-0001`, `<timestamp>.zip.part-0002`, and so on. Create one completion sidecar named `<timestamp>.complete.json`, listing the archive or parts, sizes, and repository count.

Use Yandex Disk's REST API with `Authorization: OAuth <token>` for resource and upload-link requests. Parse `disk:/backups/gitverse` as the root destination. Stage a single ZIP or all parts under `disk:/backups/gitverse/.staging/<timestamp>/`. Upload each file by requesting an upload URL with OAuth from the REST API and PUTting bytes to the returned URL without forwarding OAuth. Never log the OAuth token or returned upload URL. After the staged files upload successfully, move each to its final timestamped path under the destination. Upload the completion sidecar last to the final destination; only then is the archive published. If any archive-file upload, move, or completion-sidecar upload fails, mark the cycle unsuccessful, do not run retention, and leave older completed archives untouched. Cleanup may remove stale staging and incomplete final artifacts only after verifying that they do not belong to a completed archive. Do not use WebDAV.

After publication, identify complete archives by their completion sidecars, sort by the timestamp in each sidecar name, and retain the newest `BACKUP_RETENTION_COUNT`. For each older archive, delete its completion sidecar first. If sidecar deletion fails, do not delete that archive's files or parts and report a cleanup failure. If deleting a file or part fails after marker deletion, leave the sidecar absent so leftover files cannot be mistaken as complete, and report the cleanup failure. Never delete any retained complete archive. Do not run retention after a failed cycle. Never delete a completed archive while cleaning incomplete artifacts.

For an unsuccessful cycle, emit structured logs for failed repositories and a cycle summary; keep the service running so the next cron occurrence can retry. Do not log credentials.

## Configuration

| Variable                  | Required | Meaning                                                                              |
| ------------------------- | -------- | ------------------------------------------------------------------------------------ |
| `GITVERSE_TOKEN`          | Yes      | Token for GitVerse API requests and HTTPS Git reads.                                 |
| `BACKUP_CRON`             | Yes      | Standard five-field cron expression: minute, hour, day of month, month, day of week. |
| `TZ`                      | Yes      | IANA timezone used to interpret `BACKUP_CRON`.                                       |
| `YANDEX_DISK_OAUTH_TOKEN` | Yes      | OAuth token for Yandex Disk REST API requests.                                       |
| `YANDEX_DISK_DESTINATION` | Yes      | Yandex Disk path URI, for example `disk:/backups/gitverse`.                          |
| `BACKUP_RETENTION_COUNT`  | Yes      | Positive integer count of recent completed cycle archives to retain.                 |

The service reads settings from the process environment and loads `.env` for local development. `.env.example` documents all required variables.

## Data layout

For destination `disk:/backups/gitverse` and cycle timestamp `2026-10-25T18-30-00Z`, stage files under `disk:/backups/gitverse/.staging/2026-10-25T18-30-00Z/`. After staging succeeds, promote a single archive to `disk:/backups/gitverse/2026-10-25T18-30-00Z.zip`, or split parts to `disk:/backups/gitverse/2026-10-25T18-30-00Z.zip.part-0001` and subsequent ordered names. Upload `disk:/backups/gitverse/2026-10-25T18-30-00Z.complete.json` last. The ZIP contains repository mirrors at `alice/tool/`, `org/service/`, and similar owner/repository paths. To restore, find a timestamp with a completion sidecar, download the named archive file or all parts, concatenate split parts in order if needed, extract the ZIP, and use the extracted bare mirrors.

ASSUMPTION: archive names use UTC `YYYY-MM-DDTHH-mm-ssZ`, and ZIP entries use `{owner}/{repo}/` paths.

ASSUMPTION: staging and a completion sidecar mark a multipart archive as published only after all parts are uploaded.

ASSUMPTION: ZIP uses Deflate level 9; each part is limited to 1,000,000,000 bytes inclusive (decimal 1 GB).

ASSUMPTION: `BACKUP_RETENTION_COUNT` means the number of most recent successfully completed archives; a failed cycle does not delete older archives.

## Files to add or update

- `Cargo.toml` and `Cargo.lock` — single-binary Rust project and runtime dependencies, including Figment, Mockall, and `anyhow`.
- `.github/dependabot.yml` — weekly updates for Cargo dependencies, GitHub Actions, and Docker base images.
- `src/main.rs` — load settings, compose production adapters and the scheduler, then start the immediate cycle and cron loop.
- `src/settings.rs` — load env values through Figment, load `.env` locally, and validate required configuration.
- `src/models.rs` — shared repository, mirror, archive-artifact, and archive-package types.
- `src/ports.rs` — independently mockable repository-source, mirror, archive-builder, and backup-destination contracts.
- `src/features/backup.rs` — cycle-wide use case, failure handling, retention sequencing, and structured cycle logs.
- `src/features/scheduler.rs` — cron scheduling, clock access, waiting, and shutdown handling behind an injectable runtime.
- `src/adapters/gitverse.rs` — authenticated repository listing, pagination, and API response handling.
- `src/adapters/git.rs` — isolated bare mirror creation and workspace cleanup.
- `src/adapters/archive.rs` — cycle-wide ZIP creation, Deflate compression, ordered part splitting, and completion sidecar metadata.
- `src/adapters/yandex_disk.rs` — destination URI parsing, OAuth-authenticated REST requests, staging promotion, and retention operations.
- `tests/config.rs`, `tests/gitverse.rs`, `tests/git_mirror.rs`, `tests/archive.rs`, `tests/yandex_disk.rs`, `tests/backup.rs`, and `tests/scheduler.rs` — configuration, pagination, full-cycle mirror archive, ZIP/parts restore, REST OAuth and destination paths, publication/retention failures, and cron behavior.
- `.env.example` and `.gitignore` — local settings example and temporary artifacts.
- `Dockerfile` — multi-stage build and minimal runtime image.
- `.github/workflows/ci.yml` — format, lint, and test checks.
- `.github/workflows/release.yml` — tagged GHCR image publication.
- `Makefile` — local check and image build targets.
- `README.md` and `docs/architecture.md` — English quick start, cron and timezone configuration, single-archive layout, completion sidecar, part reconstruction, retention behavior, and Mermaid architecture overview.

## Dependency boundaries

- The use case owns cycle policy and receives each external capability through a separate port.
- The production adapters are constructed in `main.rs`; tests use generated mocks for each port.
- `ArchivePackage` owns its temporary workspace so archive files remain available until publication finishes.
- Adapter-specific errors are wrapped at the port boundary with `anyhow`; the use case reports stable, credential-safe errors.

## Trade-offs

- A cycle-wide ZIP is one restorable unit containing every repository in the completed listing; any repository failure prevents publishing an incomplete set.
- Staging followed by final names and a completion sidecar gives operators a clear rule for recognizing completed archives. Unmarked final files are incomplete artifacts, not backups.
- Retention applies only to sidecar-marked archives after a successful new publication; a failed cycle never prunes the last known completed copies.
- Splitting the ZIP byte stream into parts of at most 1,000,000,000 bytes handles Yandex's per-file upload limit while preserving a simple concatenation-based restore process.
- Token-scoped API results are the only listing source. The user accepted that GitVerse documents this endpoint inconsistently about organization and collaborator visibility.
- Git LFS is excluded by explicit user choice; the bare mirrors preserve Git refs and Git objects only.
