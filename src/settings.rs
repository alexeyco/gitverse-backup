use std::collections::HashMap;
use std::num::NonZeroUsize;
use std::str::FromStr;

use chrono_tz::Tz;
use cron::Schedule;
use figment::Figment;
use figment::providers::{Env, Serialized};
use serde::Deserialize;

#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    #[error("missing required environment variable {0}")]
    Missing(&'static str),
    #[error("invalid value for environment variable {0}")]
    Invalid(&'static str),
    #[error("failed to load .env")]
    Dotenv,
    #[error("failed to extract environment configuration")]
    Extract,
}

pub struct Config {
    pub gitverse: GitverseConfig,
    pub backup: BackupConfig,
    pub yandex_disk: YandexDiskConfig,
}

pub struct GitverseConfig {
    pub token: String,
}

pub struct BackupConfig {
    pub backup_schedule: Schedule,
    pub timezone: Tz,
    pub retention_count: NonZeroUsize,
}

pub struct YandexDiskConfig {
    pub oauth_token: String,
    pub destination: String,
}

#[derive(Deserialize)]
struct RawConfig {
    gitverse_token: Option<String>,
    backup_cron: Option<String>,
    tz: Option<String>,
    yandex_disk_oauth_token: Option<String>,
    yandex_disk_destination: Option<String>,
    backup_retention_count: Option<String>,
}

impl Config {
    pub fn load() -> Result<Self, ConfigError> {
        if let Err(error) = dotenvy::dotenv() {
            if !error.not_found() {
                return Err(ConfigError::Dotenv);
            }
        }

        // Keep environment values as strings; tokens that look numeric are still opaque text.
        let values = Env::raw()
            .only(&[
                "gitverse_token",
                "backup_cron",
                "tz",
                "yandex_disk_oauth_token",
                "yandex_disk_destination",
                "backup_retention_count",
            ])
            .iter()
            .map(|(key, value)| (key.to_string(), value))
            .collect::<HashMap<_, _>>();
        let raw: RawConfig = Figment::from(Serialized::defaults(values))
            .extract()
            .map_err(|_| ConfigError::Extract)?;

        Self::from_raw(raw)
    }

    fn from_raw(raw: RawConfig) -> Result<Self, ConfigError> {
        let gitverse_token = required(raw.gitverse_token, "GITVERSE_TOKEN")?;
        let backup_cron = required(raw.backup_cron, "BACKUP_CRON")?;
        let timezone_name = required(raw.tz, "TZ")?;
        let yandex_disk_oauth_token =
            required(raw.yandex_disk_oauth_token, "YANDEX_DISK_OAUTH_TOKEN")?;
        let yandex_disk_destination =
            required(raw.yandex_disk_destination, "YANDEX_DISK_DESTINATION")?;
        let retention = required(raw.backup_retention_count, "BACKUP_RETENTION_COUNT")?;

        if backup_cron.split_whitespace().count() != 5 {
            return Err(ConfigError::Invalid("BACKUP_CRON"));
        }
        let schedule = Schedule::from_str(&format!("0 {backup_cron}"))
            .map_err(|_| ConfigError::Invalid("BACKUP_CRON"))?;
        let timezone = Tz::from_str(&timezone_name).map_err(|_| ConfigError::Invalid("TZ"))?;
        let backup_retention_count = retention
            .parse::<usize>()
            .ok()
            .and_then(NonZeroUsize::new)
            .ok_or(ConfigError::Invalid("BACKUP_RETENTION_COUNT"))?;

        if !valid_destination(&yandex_disk_destination) {
            return Err(ConfigError::Invalid("YANDEX_DISK_DESTINATION"));
        }

        Ok(Self {
            gitverse: GitverseConfig {
                token: gitverse_token,
            },
            backup: BackupConfig {
                backup_schedule: schedule,
                timezone,
                retention_count: backup_retention_count,
            },
            yandex_disk: YandexDiskConfig {
                oauth_token: yandex_disk_oauth_token,
                destination: yandex_disk_destination,
            },
        })
    }
}

fn valid_destination(uri: &str) -> bool {
    let Some(path) = uri.strip_prefix("disk:/") else {
        return false;
    };
    let path = path.trim_matches('/');
    !path.is_empty()
        && path.split('/').all(|part| {
            !part.is_empty() && part != "." && part != ".." && !part.chars().any(char::is_control)
        })
}

fn required(value: Option<String>, name: &'static str) -> Result<String, ConfigError> {
    value
        .map(|value| value.trim().to_owned())
        .filter(|value| !value.is_empty())
        .ok_or(ConfigError::Missing(name))
}

#[cfg(test)]
mod tests {

    #![expect(
        clippy::result_large_err,
        reason = "Figment::Jail requires closures that return figment::Error"
    )]
    use super::*;
    use figment::Jail;

    fn set_valid_environment(jail: &mut Jail) {
        jail.clear_env();
        jail.set_env("GITVERSE_TOKEN", "gitverse-token");
        jail.set_env("BACKUP_CRON", "0 3 * * *");
        jail.set_env("TZ", "Europe/Moscow");
        jail.set_env("YANDEX_DISK_OAUTH_TOKEN", "yandex-token");
        jail.set_env("YANDEX_DISK_DESTINATION", "disk:/backups/gitverse");
        jail.set_env("BACKUP_RETENTION_COUNT", "7");
    }

    #[test]
    fn loads_environment_configuration() {
        Jail::expect_with(|jail| {
            set_valid_environment(jail);
            jail.set_env("GITVERSE_TOKEN", "123456");
            let config = Config::load().unwrap();

            assert_eq!(config.gitverse.token, "123456");
            assert_eq!(config.backup.timezone, "Europe/Moscow".parse().unwrap());
            assert_eq!(config.yandex_disk.destination, "disk:/backups/gitverse");
            assert_eq!(config.backup.retention_count.get(), 7);
            Ok(())
        });
    }

    #[test]
    fn rejects_invalid_cron_timezone_and_retention_count() {
        Jail::expect_with(|jail| {
            set_valid_environment(jail);
            jail.set_env("BACKUP_CRON", "0 3 * *");
            assert!(Config::load().is_err());

            jail.set_env("BACKUP_CRON", "0 3 * * *");
            jail.set_env("TZ", "Not/A_Timezone");
            assert!(Config::load().is_err());

            jail.set_env("TZ", "Europe/Moscow");
            for retention in ["0", "-1", "invalid"] {
                jail.set_env("BACKUP_RETENTION_COUNT", retention);
                assert!(Config::load().is_err());
            }
            Ok(())
        });
    }

    #[test]
    fn rejects_cron_with_seconds_instead_of_five_fields() {
        Jail::expect_with(|jail| {
            set_valid_environment(jail);
            jail.set_env("BACKUP_CRON", "0 0 3 * * *");
            assert!(Config::load().is_err());
            Ok(())
        });
    }

    #[test]
    fn rejects_missing_configuration_and_invalid_destinations() {
        Jail::expect_with(|jail| {
            jail.clear_env();
            jail.set_env("GITVERSE_TOKEN", "gitverse-token");
            jail.set_env("BACKUP_CRON", "0 3 * * *");
            jail.set_env("TZ", "Europe/Moscow");
            jail.set_env("YANDEX_DISK_OAUTH_TOKEN", "yandex-token");
            jail.set_env("YANDEX_DISK_DESTINATION", "disk:/backups/gitverse");
            assert!(Config::load().is_err());

            for destination in [
                "https://disk.example/backups",
                "disk:/",
                "disk:/backups/../private",
            ] {
                jail.set_env("BACKUP_RETENTION_COUNT", "7");
                jail.set_env("YANDEX_DISK_DESTINATION", destination);
                assert!(Config::load().is_err());
            }
            Ok(())
        });
    }
}
