use std::sync::{Arc, Condvar, Mutex};
use std::time::Duration;

use chrono::{DateTime, Utc};
use chrono_tz::Tz;
use cron::Schedule;

#[cfg_attr(test, mockall::automock)]
pub trait SchedulerRuntime {
    fn install_shutdown_handler(&mut self) -> Result<(), SchedulerError>;
    fn now(&self) -> DateTime<Utc>;
    fn wait(&mut self, timeout: Duration) -> Result<bool, SchedulerError>;
}

#[derive(Default)]
pub struct SystemSchedulerRuntime {
    shutdown: Shutdown,
}

impl SchedulerRuntime for SystemSchedulerRuntime {
    fn install_shutdown_handler(&mut self) -> Result<(), SchedulerError> {
        let shutdown = self.shutdown.clone();
        ctrlc::set_handler(move || shutdown.request()).map_err(|_| SchedulerError::SignalHandler)
    }

    fn now(&self) -> DateTime<Utc> {
        Utc::now()
    }

    fn wait(&mut self, timeout: Duration) -> Result<bool, SchedulerError> {
        self.shutdown.wait(timeout)
    }
}

pub fn run_with<F>(
    runtime: &mut impl SchedulerRuntime,
    schedule: Schedule,
    timezone: Tz,
    mut cycle: F,
) -> Result<(), SchedulerError>
where
    F: FnMut(DateTime<Utc>),
{
    runtime.install_shutdown_handler()?;
    cycle(runtime.now());

    loop {
        let now = runtime.now();
        let Some(next) = next_occurrence(&schedule, timezone, now) else {
            eprintln!("event=scheduler_stopped reason=no_future_occurrence");
            return Err(SchedulerError::NoFutureOccurrence);
        };
        let delay = (next - now).to_std().unwrap_or_default();
        if !runtime.wait(delay)? {
            eprintln!("event=scheduler_stopped reason=shutdown");
            return Ok(());
        }
        cycle(runtime.now());
    }
}

pub fn next_occurrence(
    schedule: &Schedule,
    timezone: Tz,
    now: DateTime<Utc>,
) -> Option<DateTime<Utc>> {
    schedule
        .after(&now.with_timezone(&timezone))
        .next()
        .map(|occurrence| occurrence.with_timezone(&Utc))
}

#[derive(Clone)]
struct Shutdown(Arc<(Mutex<bool>, Condvar)>);

impl Shutdown {
    fn new() -> Self {
        Self(Arc::new((Mutex::new(false), Condvar::new())))
    }

    fn request(&self) {
        let (state, wake) = &*self.0;
        if let Ok(mut requested) = state.lock() {
            *requested = true;
            wake.notify_all();
        }
    }

    fn wait(&self, timeout: Duration) -> Result<bool, SchedulerError> {
        let (state, wake) = &*self.0;
        let requested = state.lock().map_err(|_| SchedulerError::State)?;
        if *requested {
            return Ok(false);
        }
        let (requested, _) = wake
            .wait_timeout_while(requested, timeout, |requested| !*requested)
            .map_err(|_| SchedulerError::State)?;
        Ok(!*requested)
    }
}

impl Default for Shutdown {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Debug, thiserror::Error)]
pub enum SchedulerError {
    #[error("could not install the shutdown signal handler")]
    SignalHandler,
    #[error("cron schedule has no future occurrence")]
    NoFutureOccurrence,
    #[error("shutdown state was poisoned")]
    State,
}

#[cfg(test)]
mod tests {
    use std::thread;
    use std::time::Duration;

    use super::Shutdown;

    #[test]
    fn shutdown_interrupts_the_wait_for_a_cron_occurrence() {
        let shutdown = Shutdown::new();
        let requester = shutdown.clone();
        let thread = thread::spawn(move || {
            thread::sleep(Duration::from_millis(10));
            requester.request();
        });

        assert!(!shutdown.wait(Duration::from_secs(60)).unwrap());
        thread.join().unwrap();
    }
}

#[cfg(test)]
mod schedule_tests {

    use super::*;
    use chrono::{TimeZone, Utc};
    use chrono_tz::Europe::Moscow;
    use cron::Schedule;
    use std::str::FromStr;

    #[test]
    fn starts_the_first_cycle_before_waiting_for_a_cron_occurrence() {
        let schedule = Schedule::from_str("0 0 * * * * *").unwrap();
        let mut runtime = MockSchedulerRuntime::new();
        runtime
            .expect_install_shutdown_handler()
            .times(1)
            .return_once(|| Ok(()));
        runtime.expect_now().returning(Utc::now);
        runtime.expect_wait().times(1).return_once(|_| Ok(false));

        let cycles = std::cell::Cell::new(0);
        run_with(&mut runtime, schedule, Moscow, |_| {
            cycles.set(cycles.get() + 1);
        })
        .unwrap();

        assert_eq!(cycles.get(), 1);
    }

    #[test]
    fn skips_a_cron_occurrence_that_passed_while_a_cycle_was_active() {
        let schedule = Schedule::from_str("0 * * * * * *").unwrap();
        let finished = Utc.with_ymd_and_hms(2026, 10, 25, 12, 1, 30).unwrap();

        let next = next_occurrence(&schedule, Moscow, finished).unwrap();

        assert_eq!(next, Utc.with_ymd_and_hms(2026, 10, 25, 12, 2, 0).unwrap());
    }
}
