use std::sync::Arc;

use time::{Date, Month, OffsetDateTime};

use crate::deepgram_api::{fetch_month_to_date_spend, DeepgramApiError};
use crate::settings::LiveConfigStore;
use crate::state::{AppState, DeepgramConnectionStatus};

const FOOTER_PERMISSION_DENIED_MESSAGE: &str =
    "Admin- or owner-level project API key required for billing reporting.";
const PROJECT_ID_ENV_VAR: &str = "DEEPGRAM_PROJECT_ID";

#[derive(Clone)]
pub struct BillingController {
    config_store: LiveConfigStore,
    state: Arc<AppState>,
}

impl BillingController {
    pub fn new(state: Arc<AppState>, config_store: LiveConfigStore) -> Self {
        Self {
            config_store,
            state,
        }
    }

    pub fn refresh_month_to_date_spend(&self) {
        let today = current_local_date();
        let footer_label = billing_footer_label(today);
        let month_start = month_start_for(today);

        let current_config = self.config_store.current();
        let Some(project_id) = current_config.resolve_deepgram_project_id() else {
            self.state.set_overlay_footer_text("");
            return;
        };

        let Ok(api_key) = current_config.resolve_deepgram_api_key() else {
            self.state.set_overlay_footer_text("");
            return;
        };

        self.state
            .set_overlay_footer_text(format!("{}: ...", footer_label));
        let state = Arc::clone(&self.state);
        std::thread::Builder::new()
            .name("billing-refresh".into())
            .spawn(move || {
                match fetch_month_to_date_spend(&api_key, &project_id, month_start, today) {
                    Ok(month_to_date_spend) => {
                        state.set_deepgram_connection_status(DeepgramConnectionStatus::Connected);
                        state.set_overlay_footer_text(format!(
                            "{}: {}",
                            footer_label,
                            format_usd(month_to_date_spend)
                        ));
                    }
                    Err(error) => {
                        log::warn!("failed to refresh Deepgram billing breakdown: {}", error);
                        let connection_status = match &error {
                            DeepgramApiError::PermissionDenied(_) => {
                                DeepgramConnectionStatus::Connected
                            }
                            DeepgramApiError::Unauthorized(_) | DeepgramApiError::Other(_) => {
                                DeepgramConnectionStatus::Disconnected
                            }
                        };
                        state.set_deepgram_connection_status(connection_status);
                        state.set_overlay_footer_text(match error {
                            DeepgramApiError::PermissionDenied(_) => {
                                FOOTER_PERMISSION_DENIED_MESSAGE.to_owned()
                            }
                            DeepgramApiError::Unauthorized(_) | DeepgramApiError::Other(_) => {
                                format!("{}: unavailable", footer_label)
                            }
                        });
                    }
                }
            })
            .expect("failed to spawn billing refresh thread");
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
    use super::{billing_footer_label, format_usd};
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
