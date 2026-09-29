use std::fmt::{Display, Formatter};
use std::time::Duration;

use reqwest::blocking::Client;
use reqwest::header::{HeaderValue, AUTHORIZATION, CONTENT_TYPE, USER_AGENT};
use serde::de::IgnoredAny;
use serde::Deserialize;

const API_URL_PREFIX: &str = "https://api.deepgram.com/v1";
const APPLICATION_USER_AGENT: &str = concat!("simple-ptt/", env!("CARGO_PKG_VERSION"));
const HTTP_TIMEOUT_SECS: u64 = 20;

#[derive(Debug)]
pub enum DeepgramApiError {
    PermissionDenied(String),
    Unauthorized(String),
    Other(String),
}

#[derive(Deserialize)]
struct DeepgramErrorResponse {
    category: Option<String>,
    details: Option<String>,
    message: Option<String>,
    request_id: Option<String>,
}

#[derive(Deserialize)]
struct ListProjectsResponse {
    /// Only counted, so each project is parsed and discarded.
    #[serde(default)]
    projects: Vec<IgnoredAny>,
}

impl Display for DeepgramApiError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::PermissionDenied(message)
            | Self::Unauthorized(message)
            | Self::Other(message) => formatter.write_str(message),
        }
    }
}

/// How many projects `api_key` can see.
pub fn count_projects(api_key: &str) -> Result<usize, DeepgramApiError> {
    let client = deepgram_http_client()?;
    let response = client
        .get(format!("{}/projects", API_URL_PREFIX))
        .header(AUTHORIZATION, authorization_header_value(api_key)?)
        .send()
        .map_err(|error| {
            DeepgramApiError::Other(format!("project list request failed: {}", error))
        })?;

    if !response.status().is_success() {
        return Err(parse_deepgram_error_response(response, "project list"));
    }

    let projects_response: ListProjectsResponse = response.json().map_err(|error| {
        DeepgramApiError::Other(format!("project list response parsing failed: {}", error))
    })?;

    Ok(projects_response.projects.len())
}

fn deepgram_http_client() -> Result<Client, DeepgramApiError> {
    Client::builder()
        .timeout(Duration::from_secs(HTTP_TIMEOUT_SECS))
        .default_headers(default_headers()?)
        .build()
        .map_err(|error| DeepgramApiError::Other(format!("failed to build HTTP client: {}", error)))
}

fn default_headers() -> Result<reqwest::header::HeaderMap, DeepgramApiError> {
    let mut headers = reqwest::header::HeaderMap::new();
    headers.insert(USER_AGENT, HeaderValue::from_static(APPLICATION_USER_AGENT));
    headers.insert(CONTENT_TYPE, HeaderValue::from_static("application/json"));
    Ok(headers)
}

fn authorization_header_value(api_key: &str) -> Result<HeaderValue, DeepgramApiError> {
    HeaderValue::from_str(&format!("Token {}", api_key)).map_err(|error| {
        DeepgramApiError::Other(format!(
            "failed to build Deepgram authorization header: {}",
            error
        ))
    })
}

fn parse_deepgram_error_response(
    response: reqwest::blocking::Response,
    request_label: &str,
) -> DeepgramApiError {
    let status = response.status();
    let response_body = match response.text() {
        Ok(body) => body,
        Err(error) => {
            return DeepgramApiError::Other(format!(
                "{} error response parsing failed: {}",
                request_label, error
            ));
        }
    };

    let classify_error = |message: String, category: Option<&str>| match status {
        reqwest::StatusCode::UNAUTHORIZED => DeepgramApiError::Unauthorized(message),
        reqwest::StatusCode::FORBIDDEN => DeepgramApiError::PermissionDenied(message),
        _ if category == Some("INSUFFICIENT_PERMISSIONS") => {
            DeepgramApiError::PermissionDenied(message)
        }
        _ => DeepgramApiError::Other(message),
    };

    if let Ok(error_response) = serde_json::from_str::<DeepgramErrorResponse>(&response_body) {
        return classify_error(
            format!(
                "{} returned {} ({}){}{}",
                request_label,
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
            ),
            error_response.category.as_deref(),
        );
    }

    classify_error(
        format!("{} returned {} ({})", request_label, status, response_body),
        None,
    )
}

