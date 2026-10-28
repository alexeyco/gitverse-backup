## 1. Rust service foundation and configuration

- [x] 1.1 Add the single-binary Rust project in `Cargo.toml` and `src/main.rs`; create `src/settings.rs` to load `.env` locally and validate `GITVERSE_TOKEN`, `BACKUP_CRON`, `TZ`, `YANDEX_DISK_OAUTH_TOKEN`, `YANDEX_DISK_DESTINATION`, and `BACKUP_RETENTION_COUNT` before starting.
- [x] 1.2 Add `.env.example` and `.gitignore` entries for local configuration and temporary workspaces; document the environment contract in `README.md`.
- [x] 1.3 Add `tests/config.rs` coverage for the `Start with valid schedule configuration` and `Reject an invalid cron, timezone, or retention count` scenarios.
- [x] 1.4 Implement the five-field cron scheduler in `src/features/scheduler.rs`; add `tests/scheduler.rs` coverage for the `Skip a cron occurrence while a cycle is active` scenario and the immediate first cycle in `Start with valid schedule configuration`.

## 2. GitVerse discovery and cycle archive creation

- [x] 2.1 Implement `src/adapters/gitverse.rs` to call `GET /user/repos` with bearer authentication, follow all pagination links, and parse each repository's `full_name` and HTTPS `clone_url`.
- [x] 2.2 Implement `src/adapters/git.rs` to create one bare mirror per repository with branches, tags, and history, exclude Git LFS, and clean its temporary workspace.
- [x] 2.3 Implement `src/adapters/archive.rs` to package all successful mirrors into one Deflate level 9 ZIP with `{owner}/{repo}/` entry paths, split ZIP bytes into ordered parts no larger than 1,000,000,000 bytes when needed, and write a completion sidecar listing archive files and sizes.
- [x] 2.4 Add `tests/gitverse.rs` coverage for the `Process multiple repository pages`, `Process repositories returned by the token`, and `Reject a repository listing failure` scenarios.
- [x] 2.5 Add `tests/git_mirror.rs` and `tests/archive.rs` coverage for the `Archive multiple repositories in one ZIP` and `Reconstruct and restore all cycle mirrors` scenarios.
- [x] 2.6 Add `tests/archive.rs` coverage for the `Split an archive that exceeds the file limit` scenario and assert each part is no larger than 1,000,000,000 bytes.
- [x] 2.7 Add a cycle-level test for the `Do not publish an incomplete cycle` scenario: one repo mirror fails, remaining repos are attempted, and no archive or completion sidecar is uploaded.

## 3. Yandex Disk REST staging and publication

- [x] 3.1 Implement `src/adapters/yandex_disk.rs` to parse the destination URI, request OAuth-authenticated upload links, PUT to returned upload URLs without forwarding OAuth, move staged files to final timestamped names, and upload the completion sidecar last.
- [x] 3.2 Implement Yandex Disk staging and promotion in `src/adapters/yandex_disk.rs`; add coverage for the `Clean incomplete artifacts without deleting completed archives` scenario.
- [x] 3.3 Add `tests/yandex_disk.rs` integration coverage for the `Upload a cycle archive to its timestamped destination` and `Upload files at the configured destination path` scenarios.
- [x] 3.4 Add `tests/yandex_disk.rs` integration coverage for the `Authorize REST requests without forwarding OAuth to upload URLs` scenario; assert `Authorization: OAuth <token>` is sent on the API upload-link request, absent on the returned URL PUT, and neither token nor upload URL appears in logs.
- [x] 3.5 Add `tests/backup.rs` coverage for the `Mark an archive complete only after all final files exist`, `Preserve completed archives after incomplete publication`, and `Report sidecar upload failure without publishing or pruning` scenarios; assert failed sidecar upload leaves no completion marker, reports cycle failure, skips retention, and leaves prior completed archives untouched.

## 4. Retention, cycle results, release, and documentation

- [x] 4.1 Implement cycle-level failure reporting in `src/features/backup.rs`; add tests for the `Continue processing after a repository failure` and `Retry repositories at a later cron occurrence` scenarios.
- [x] 4.2 Implement completed-archive retention in `src/adapters/yandex_disk.rs`; delete each old archive's completion sidecar before its files or parts, stop if marker deletion fails, and leave the marker absent and report cleanup failure if later file deletion fails. Add `tests/backup.rs` coverage for the `Prune older complete archives after successful publication`, `Do not prune archives after a failed cycle`, `Keep retention safe when sidecar deletion fails`, and `Keep retention safe when archive-file deletion fails` scenarios. Assert operation order removes the marker before deleting payload files, marker deletion failure prevents payload deletion, a later payload deletion failure leaves no marker, and retained complete archives are never affected.
- [x] 4.3 Add `Dockerfile` with a multi-stage Rust build and a runtime image that starts the service as its container entrypoint.
- [x] 4.4 Add `.github/workflows/ci.yml` and `Makefile` checks for formatting, Clippy, and tests.
- [x] 4.5 Add `.github/workflows/release.yml` to build and publish `ghcr.io/alexeyco/gitverse-backup` when a release tag is pushed; cover the `Publish a tagged image` scenario.
- [x] 4.6 Write the English `README.md` quick start and `docs/architecture.md` with env settings, cron and timezone, one archive per cycle, completion sidecar, part reconstruction and the 1,000,000,000-byte per-part limit, retention behavior, and Mermaid architecture; cover the `Follow the repository documentation` scenario.
- [x] 4.7 Run scenario tests, validate the change with OpenSpec, format Markdown, and inspect the complete diff.

## 5. Ports and adapters architecture

- [x] 5.1 Extract shared workflow types into `src/models.rs` and define separate repository source, repository mirror, archive builder, and backup destination ports in `src/ports.rs`.
- [x] 5.2 Implement the ports in the GitVerse, Git, ZIP, and Yandex Disk adapters; keep archive workspace ownership in `ArchivePackage` until publication finishes.
- [x] 5.3 Compose production adapters in `src/main.rs` and inject the scheduler runtime and backup ports into their use cases.
- [x] 5.4 Replace the combined backup-service mock with independent Mockall mocks and test successful operation ordering, repository failure continuation, and Figment configuration under Jail.

## 6. Dependency update automation

- [x] 6.1 Add `.github/dependabot.yml` to check Cargo dependencies, GitHub Actions, and the Docker base image weekly.
