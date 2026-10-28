mod adapters;
mod features;
mod models;
mod ports;
mod settings;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let config = settings::Config::load()?;

    let mut source = adapters::gitverse::GitverseClient::new(config.gitverse.token.clone())?;
    let mut mirror = adapters::git::GitRepositoryMirror;
    let mut archive_builder = adapters::archive::ZipArchiveBuilder;
    let mut destination = adapters::yandex_disk::YandexDiskBackupDestination::new(
        config.yandex_disk.oauth_token,
        &config.yandex_disk.destination,
    )?;

    let mut scheduler = features::scheduler::SystemSchedulerRuntime::default();

    features::scheduler::run_with(
        &mut scheduler,
        config.backup.backup_schedule,
        config.backup.timezone,
        |now| {
            let timestamp = now.format("%Y-%m-%dT%H-%M-%SZ").to_string();
            if features::backup::run_cycle_with(
                &mut source,
                &mut mirror,
                &mut archive_builder,
                &mut destination,
                &config.gitverse.token,
                &timestamp,
                config.backup.retention_count,
            )
            .is_err()
            {
                eprintln!("event=backup_cycle_failed timestamp={timestamp}");
            }
        },
    )?;

    Ok(())
}
