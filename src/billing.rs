use std::sync::{Arc, Mutex, PoisonError};

use time::{Date, Month, OffsetDateTime};

use crate::deepgram_api::{fetch_month_to_date_spend, DeepgramApiError};
use crate::settings::LiveConfigStore;
use crate::state::{AppState, DeepgramApiKeyFingerprint, DeepgramConnectionStatus};

const FOOTER_PERMISSION_DENIED_MESSAGE: &str =
    "Admin- or owner-level project API key required for billing reporting.";
const PROJECT_ID_ENV_VAR: &str = "DEEPGRAM_PROJECT_ID";

#[derive(Clone)]
pub struct BillingController {
    config_store: LiveConfigStore,
    generations: RefreshGenerations,
    state: Arc<AppState>,
}

impl BillingController {
    pub fn new(state: Arc<AppState>, config_store: LiveConfigStore) -> Self {
        Self {
            config_store,
            generations: RefreshGenerations::default(),
            state,
        }
    }

    pub fn refresh_month_to_date_spend(&self) {
        let today = current_local_date();
        let footer_label = billing_footer_label(today);
        let month_start = month_start_for(today);

        let current_config = self.config_store.current();
        let Some(project_id) = current_config.resolve_deepgram_project_id() else {
            self.generations
                .begin(|| self.state.set_overlay_footer_text(""));
            return;
        };

        let Ok(api_key) = current_config.resolve_deepgram_api_key() else {
            self.generations
                .begin(|| self.state.set_overlay_footer_text(""));
            return;
        };

        let api_key_fingerprint = DeepgramApiKeyFingerprint::of(&api_key);
        let generation = self.generations.begin(|| {
            self.state
                .set_overlay_footer_text(format!("{}: ...", footer_label))
        });
        let generations = self.generations.clone();
        let state = Arc::clone(&self.state);
        std::thread::Builder::new()
            .name("billing-refresh".into())
            .spawn(move || {
                let (connection_status, footer_text) =
                    match fetch_month_to_date_spend(&api_key, &project_id, month_start, today) {
                        Ok(month_to_date_spend) => (
                            DeepgramConnectionStatus::Connected,
                            format!("{}: {}", footer_label, format_usd(month_to_date_spend)),
                        ),
                        Err(error) => {
                            log::warn!("failed to refresh Deepgram billing breakdown: {}", error);
                            match error {
                                DeepgramApiError::PermissionDenied(_) => (
                                    DeepgramConnectionStatus::Connected,
                                    FOOTER_PERMISSION_DENIED_MESSAGE.to_owned(),
                                ),
                                DeepgramApiError::Unauthorized(_) | DeepgramApiError::Other(_) => (
                                    DeepgramConnectionStatus::Disconnected,
                                    format!("{}: unavailable", footer_label),
                                ),
                            }
                        }
                    };

                let published = generations.publish_if_latest(generation, || {
                    state.set_deepgram_connection_status(connection_status, api_key_fingerprint);
                    state.set_overlay_footer_text(footer_text);
                });
                if !published {
                    log::info!("dropped a Deepgram billing result superseded by a later refresh");
                }
            })
            .expect("failed to spawn billing refresh thread");
    }
}

/// Orders billing refreshes. Every refresh starts a new generation, and a
/// refresh thread publishes its result only while its generation is still
/// the latest, so a slow refresh started with a previous key or project ID
/// never overwrites what a later refresh wrote. Both steps hold one lock, so
/// a publish cannot interleave with a newer refresh's start.
#[derive(Clone, Default)]
struct RefreshGenerations {
    latest: Arc<Mutex<u64>>,
}

impl RefreshGenerations {
    /// Starts a generation and runs `start` before any older one can publish.
    fn begin(&self, start: impl FnOnce()) -> u64 {
        let mut latest = self.latest.lock().unwrap_or_else(PoisonError::into_inner);
        *latest += 1;
        start();
        *latest
    }

    /// Runs `publish` if `generation` is still the latest; returns whether it ran.
    fn publish_if_latest(&self, generation: u64, publish: impl FnOnce()) -> bool {
        let latest = self.latest.lock().unwrap_or_else(PoisonError::into_inner);
        if *latest != generation {
            return false;
        }

        publish();
        true
    }
}

pub fn deepgram_project_id_env_var() -> &'static str {
    PROJECT_ID_ENV_VAR
}

fn current_local_date() -> Date {
    OffsetDateTime::now_local()
        .unwrap_or_else(|_| OffsetDateTime::now_utc())
        .date()
}

fn month_start_for(date: Date) -> Date {
    date.replace_day(1).expect("day 1 must always be valid")
}

fn billing_footer_label(date: Date) -> String {
    format!(
        "Deepgram ({} {})",
        month_abbreviation(date.month()),
        date.year()
    )
}

#[cfg(test)]
mod tests {
    use super::{billing_footer_label, format_usd, RefreshGenerations};
    use std::cell::Cell;
    use time::{Date, Month};

    #[test]
    fn billing_footer_label_uses_deepgram_prefix() {
        let date = Date::from_calendar_date(2026, Month::April, 3).unwrap();

        assert_eq!(billing_footer_label(date), "Deepgram (Apr 2026)");
    }

    #[test]
    fn format_usd_rounds_sub_dollar_amounts_to_cents() {
        assert_eq!(format_usd(0.0), "$0.00");
        assert_eq!(format_usd(0.004), "$0.00");
        assert_eq!(format_usd(0.005), "$0.01");
        assert_eq!(format_usd(0.126), "$0.13");
    }

    /// A refresh with key A is in flight when a save starts a refresh with key B
    /// (or clears the footer because the project ID was removed): A's result
    /// must not overwrite it.
    #[test]
    fn superseded_refresh_does_not_publish() {
        let generations = RefreshGenerations::default();
        let started_with_old_key = generations.begin(|| {});
        let started_with_new_key = generations.begin(|| {});

        let old_published = Cell::new(false);
        assert!(!generations.publish_if_latest(started_with_old_key, || old_published.set(true)));
        assert!(!old_published.get());

        let new_published = Cell::new(false);
        assert!(generations.publish_if_latest(started_with_new_key, || new_published.set(true)));
        assert!(new_published.get());
    }

    #[test]
    fn latest_refresh_publishes_even_after_an_older_one_finished() {
        let generations = RefreshGenerations::default();
        let first = generations.begin(|| {});
        assert!(generations.publish_if_latest(first, || {}));
        assert!(generations.publish_if_latest(first, || {}));

        let second = generations.begin(|| {});
        assert!(!generations.publish_if_latest(first, || {}));
        assert!(generations.publish_if_latest(second, || {}));
    }
}

fn month_abbreviation(month: Month) -> &'static str {
    match month {
        Month::January => "Jan",
        Month::February => "Feb",
        Month::March => "Mar",
        Month::April => "Apr",
        Month::May => "May",
        Month::June => "Jun",
        Month::July => "Jul",
        Month::August => "Aug",
        Month::September => "Sep",
        Month::October => "Oct",
        Month::November => "Nov",
        Month::December => "Dec",
    }
}

fn format_usd(amount: f64) -> String {
    if amount >= 100.0 {
        return format!("${:.0}", amount);
    }

    format!("${:.2}", amount)
}
