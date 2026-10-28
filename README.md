# gitverse-backup

`gitverse-backup` creates scheduled archives of GitVerse repositories visible
to a GitVerse access token and stores them on Yandex Disk.

## Quick start

Create a `.env` file from the example and set all required values:

```sh
cp .env.example .env
```

```dotenv
GITVERSE_TOKEN=your-gitverse-token
BACKUP_CRON=0 3 * * *
TZ=Europe/Moscow
YANDEX_DISK_OAUTH_TOKEN=your-yandex-disk-oauth-token
YANDEX_DISK_DESTINATION=disk:/backups/gitverse
BACKUP_RETENTION_COUNT=14
```

`BACKUP_CRON` uses the five-field cron format: minute, hour, day of month,
month, and day of week. `TZ` is an IANA timezone used to interpret that
schedule. Every scheduled cycle discovers all repositories visible to the
token and writes one timestamped UTC ZIP archive for the complete cycle.

Build and run the container with the environment file:

```sh
docker build -t gitverse-backup .
docker run --env-file .env gitverse-backup
```

## Archive format

Each cycle produces one ZIP named with its UTC timestamp, such as
`2026-10-25T00-00-00Z.zip`. The archive contains each full bare Git mirror at
`{owner}/{repo}/`, including Git refs and objects. ZIP entries use Deflate
level 9. Archives larger than 1,000,000,000 bytes are split into ordered
parts, each no larger than 1,000,000,000 bytes. A
`{timestamp}.complete.json` sidecar is published after every archive part;
its presence marks a complete archive.

To restore a multipart archive, concatenate the parts in numeric order and
extract the resulting ZIP. For a single-part archive, extract the ZIP
directly. Each extracted repository directory is a bare Git repository.

`BACKUP_RETENTION_COUNT` is required and must be a positive integer. After a
successful publication, the service retains that number of the newest
completed cycle archives and removes older completed archives. Failed cycles
do not prune existing archives.

See [the architecture guide](docs/architecture.md) for the publication and
retention flow.

## License

This project is licensed under the [MIT License](LICENSE).
