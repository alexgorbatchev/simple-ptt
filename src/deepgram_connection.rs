use std::collections::VecDeque;
use std::sync::{Arc, Mutex};

use sha2::{Digest, Sha256};

use crate::deepgram_api::{list_projects, DeepgramProjectSummary};

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DeepgramCheckRequest {
    pub resolved_api_key: String,
    pub resolved_project_id: Option<String>,
}

impl DeepgramCheckRequest {
    pub fn new(resolved_api_key: String, resolved_project_id: Option<String>) -> Self {
        Self {
            resolved_api_key: resolved_api_key.trim().to_owned(),
            resolved_project_id: resolved_project_id
                .map(|value| value.trim().to_owned())
                .filter(|value| !value.is_empty()),
        }
    }

    pub fn same_source_as(&self, other: &Self) -> bool {
        self.api_key_fingerprint() == other.api_key_fingerprint()
            && self.resolved_project_id == other.resolved_project_id
    }

    fn api_key_fingerprint(&self) -> String {
        let mut hasher = Sha256::new();
        hasher.update(self.resolved_api_key.as_bytes());
        let digest = hasher.finalize();
        digest[..8]
            .iter()
            .map(|byte| format!("{:02x}", byte))
            .collect()
    }
}

#[derive(Clone, Debug)]
pub enum DeepgramCheckUpdate {
    ConnectionChecked {
        request: DeepgramCheckRequest,
        message: String,
    },
    ActionFailed {
        request: DeepgramCheckRequest,
        message: String,
    },
}

#[derive(Clone, Default)]
pub struct DeepgramConnectionController {
    state: Arc<Mutex<DeepgramConnectionState>>,
}

#[derive(Default)]
struct DeepgramConnectionState {
    pending_updates: VecDeque<DeepgramCheckUpdate>,
}

impl DeepgramConnectionController {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn has_pending_ui_update(&self) -> bool {
        self.state
            .lock()
            .map(|state| !state.pending_updates.is_empty())
            .unwrap_or(false)
    }

    pub fn take_update(&self) -> Option<DeepgramCheckUpdate> {
        self.state
            .lock()
            .ok()
            .and_then(|mut state| state.pending_updates.pop_front())
    }

    pub fn start_check(&self, request: DeepgramCheckRequest) {
        let controller = self.clone();
        std::thread::Builder::new()
            .name("deepgram-check".into())
            .spawn(move || {
                let update = controller.check_connection(request);
                controller.push_update(update);
            })
            .expect("failed to spawn Deepgram check worker thread");
    }

    fn check_connection(&self, request: DeepgramCheckRequest) -> DeepgramCheckUpdate {
        match list_projects(&request.resolved_api_key) {
            Ok(projects) => DeepgramCheckUpdate::ConnectionChecked {
                message: deepgram_connection_message(&request, &projects),
                request,
            },
            Err(error) => DeepgramCheckUpdate::ActionFailed {
                request,
                message: error.to_string(),
            },
        }
    }

    fn push_update(&self, update: DeepgramCheckUpdate) {
        if let Ok(mut state) = self.state.lock() {
            state.pending_updates.push_back(update);
        }
    }
}

fn deepgram_connection_message(
    request: &DeepgramCheckRequest,
    projects: &[DeepgramProjectSummary],
) -> String {
    let project_count_message = match projects.len() {
        0 => "no projects".to_owned(),
        1 => "1 project".to_owned(),
        count => format!("{} projects", count),
    };

    let Some(project_id) = request.resolved_project_id.as_deref() else {
        return format!("Connected to Deepgram. Found {}.", project_count_message);
    };

    let Some(project) = projects
        .iter()
        .find(|project| project.project_id == project_id)
    else {
        return format!(
            "Connected to Deepgram. Found {}, but configured project ID '{}' was not returned for this API key.",
            project_count_message, project_id
        );
    };

    let trimmed_project_name = project.name.trim();
    if trimmed_project_name.is_empty() {
        return format!(
            "Connected to Deepgram. Found {}. Project ID '{}' is available.",
            project_count_message, project_id
        );
    }

    format!(
        "Connected to Deepgram. Found {}. Project ID '{}' matches '{}'.",
        project_count_message, project_id, trimmed_project_name
    )
}

#[cfg(test)]
mod tests {
    use super::{deepgram_connection_message, DeepgramCheckRequest};
    use crate::deepgram_api::DeepgramProjectSummary;

    #[test]
    fn deepgram_check_request_matches_same_api_key_and_project_id() {
        let first = DeepgramCheckRequest::new("secret-key".to_owned(), Some("proj_123".to_owned()));
        let second =
            DeepgramCheckRequest::new(" secret-key ".to_owned(), Some("proj_123".to_owned()));
        let different_project =
            DeepgramCheckRequest::new("secret-key".to_owned(), Some("proj_456".to_owned()));

        assert!(first.same_source_as(&second));
        assert!(!first.same_source_as(&different_project));
    }

    #[test]
    fn connection_message_confirms_matching_project_id() {
        let request =
            DeepgramCheckRequest::new("secret-key".to_owned(), Some("proj_123".to_owned()));
        let projects = vec![DeepgramProjectSummary {
            project_id: "proj_123".to_owned(),
            name: "Primary Workspace".to_owned(),
        }];

        assert_eq!(
            deepgram_connection_message(&request, &projects),
            "Connected to Deepgram. Found 1 project. Project ID 'proj_123' matches 'Primary Workspace'."
        );
    }

    #[test]
    fn connection_message_reports_missing_configured_project_id() {
        let request =
            DeepgramCheckRequest::new("secret-key".to_owned(), Some("proj_missing".to_owned()));
        let projects = vec![DeepgramProjectSummary {
            project_id: "proj_123".to_owned(),
            name: "Primary Workspace".to_owned(),
        }];

        assert_eq!(
            deepgram_connection_message(&request, &projects),
            "Connected to Deepgram. Found 1 project, but configured project ID 'proj_missing' was not returned for this API key."
        );
    }

    #[test]
    fn connection_message_works_without_project_id() {
        let request = DeepgramCheckRequest::new("secret-key".to_owned(), None);
        let projects = vec![DeepgramProjectSummary {
            project_id: "proj_123".to_owned(),
            name: "Primary Workspace".to_owned(),
        }];

        assert_eq!(
            deepgram_connection_message(&request, &projects),
            "Connected to Deepgram. Found 1 project."
        );
    }
}
