use std::fmt::{Display, Formatter};
use std::sync::Arc;

use serde::Deserialize;
use time::{Date, Month, OffsetDateTime};

use crate::settings::LiveConfigStore;
use crate::state::AppState;

const BILLING_BREAKDOWN_URL_PREFIX: &str = "https://api.deepgram.com/v1/projects";
const FOOTER_PERMISSION_DENIED_MESSAGE: &str =
    "Admin- or owner-level project API key required for billing reporting.";
const PROJECT_ID_ENV_VAR: &str = "DEEPGRAM_PROJECT_ID";

#[derive(Clone)]
pub struct BillingController {
    config_store: LiveConfigStore,
    state: Arc<AppState>,
}

#[derive(Deserialize)]
struct BillingBreakdownResponse {
    results: Vec<BillingBreakdownResult>,
}

#[derive(Deserialize)]
struct BillingBreakdownResult {
    dollars: f64,
}

#[derive(Deserialize)]
struct BillingErrorResponse {
    category: Option<String>,
    details: Option<String>,
    message: Option<String>,
    request_id: Option<String>,
}

enum BillingFetchError {
    PermissionDenied(String),
    Other(String),
}

impl Display for BillingFetchError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::PermissionDenied(message) | Self::Other(message) => formatter.write_str(message),
        }
    }
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
            .set_overlay_footer_text(format!("{}: refreshing…", footer_label));
        let state = Arc::clone(&self.state);
        std::thread::Builder::new()
            .name("billing-refresh".into())
            .spawn(move || {
                match fetch_month_to_date_spend(&api_key, &project_id, month_start, today) {
                    Ok(month_to_date_spend) => {
                        state.set_overlay_footer_text(format!(
                            "{}: {}",
                            footer_label,
                            format_usd(month_to_date_spend)
                        ));
                    }
                    Err(error) => {
                        log::warn!("failed to refresh Deepgram billing breakdown: {}", error);
                        state.set_overlay_footer_text(match error {
                            BillingFetchError::PermissionDenied(_) => {
                                FOOTER_PERMISSION_DENIED_MESSAGE.to_owned()
                            }
                            BillingFetchError::Other(_) => {
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

fn fetch_month_to_date_spend(
    api_key: &str,
    project_id: &str,
    month_start: Date,
    today: Date,
) -> Result<f64, BillingFetchError> {
    let client = reqwest::blocking::Client::new();
    let response = client
        .get(format!(
            "{}/{}/billing/breakdown",
            BILLING_BREAKDOWN_URL_PREFIX, project_id
        ))
        .header("Authorization", format!("Token {}", api_key))
        .query(&[
            ("start", format_date(month_start)),
            ("end", format_date(today)),
        ])
        .send()
        .map_err(|error| BillingFetchError::Other(format!("request failed: {}", error)))?;

    if !response.status().is_success() {
        let status = response.status();
        let response_body = response.text().map_err(|error| {
            BillingFetchError::Other(format!("error response parsing failed: {}", error))
        })?;

        if let Ok(error_response) = serde_json::from_str::<BillingErrorResponse>(&response_body) {
            let formatted_error = format!(
                "billing breakdown returned {} ({}){}{}",
                status,
                error_response
                    .message
                    .as_deref()
                    .unwrap_or("unknown Deepgram error"),
                error_response
                    .details
                    .as_deref()
                    .map(|details| format!(", details={}", details))
                    .unwrap_or_default(),
                error_response
                    .request_id
                    .as_deref()
                    .map(|request_id| format!(", request_id={}", request_id))
                    .unwrap_or_default()
            );

            if status == reqwest::StatusCode::FORBIDDEN
                || error_response.category.as_deref() == Some("INSUFFICIENT_PERMISSIONS")
            {
                return Err(BillingFetchError::PermissionDenied(formatted_error));
            }

            return Err(BillingFetchError::Other(formatted_error));
        }

        let formatted_error = format!("billing breakdown returned {} ({})", status, response_body);
        if status == reqwest::StatusCode::FORBIDDEN {
            return Err(BillingFetchError::PermissionDenied(formatted_error));
        }

        return Err(BillingFetchError::Other(formatted_error));
    }

    let billing_breakdown: BillingBreakdownResponse = response
        .json()
        .map_err(|error| BillingFetchError::Other(format!("response parsing failed: {}", error)))?;

    Ok(billing_breakdown
        .results
        .iter()
        .map(|result| result.dollars)
        .sum())
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
        "Billing ({} {})",
        month_abbreviation(date.month()),
        date.year()
    )
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

fn format_date(date: Date) -> String {
    let month_number: u8 = match date.month() {
        Month::January => 1,
        Month::February => 2,
        Month::March => 3,
        Month::April => 4,
        Month::May => 5,
        Month::June => 6,
        Month::July => 7,
        Month::August => 8,
        Month::September => 9,
        Month::October => 10,
        Month::November => 11,
        Month::December => 12,
    };

    format!("{:04}-{:02}-{:02}", date.year(), month_number, date.day())
}

fn format_usd(amount: f64) -> String {
    if amount >= 100.0 {
        return format!("${:.0}", amount);
    }

    if amount >= 1.0 {
        return format!("${:.2}", amount);
    }

    format!("${:.4}", amount)
}
