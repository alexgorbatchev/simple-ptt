use std::collections::VecDeque;
use std::sync::{Arc, Mutex};

use crate::deepgram_api::count_projects;
use crate::state::DeepgramApiKeyFingerprint;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DeepgramCheckRequest {
    pub resolved_api_key: String,
}

impl DeepgramCheckRequest {
    pub fn new(resolved_api_key: String) -> Self {
        Self {
            resolved_api_key: resolved_api_key.trim().to_owned(),
        }
    }

    pub fn same_source_as(&self, other: &Self) -> bool {
        self.api_key_fingerprint() == other.api_key_fingerprint()
    }

    pub fn api_key_fingerprint(&self) -> DeepgramApiKeyFingerprint {
        DeepgramApiKeyFingerprint::of(&self.resolved_api_key)
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
        match count_projects(&request.resolved_api_key) {
            Ok(project_count) => DeepgramCheckUpdate::ConnectionChecked {
                message: deepgram_connection_message(project_count),
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

/// What a successful check reports: that the key connects, and how many
/// projects it can see.
fn deepgram_connection_message(project_count: usize) -> String {
    let projects = match project_count {
        0 => "no projects".to_owned(),
        1 => "1 project".to_owned(),
        count => format!("{} projects", count),
    };
    format!("Connected to Deepgram. Found {}.", projects)
}

#[cfg(test)]
mod tests {
    use super::{deepgram_connection_message, DeepgramCheckRequest};

    #[test]
    fn deepgram_check_request_is_the_same_source_for_the_same_trimmed_key() {
        let first = DeepgramCheckRequest::new("secret-key".to_owned());
        let second = DeepgramCheckRequest::new(" secret-key ".to_owned());
        let other_key = DeepgramCheckRequest::new("other-key".to_owned());

        assert!(first.same_source_as(&second));
        assert!(!first.same_source_as(&other_key));
    }

    #[test]
    fn connection_message_counts_the_projects_the_key_can_see() {
        assert_eq!(deepgram_connection_message(0), "Connected to Deepgram. Found no projects.");
        assert_eq!(deepgram_connection_message(1), "Connected to Deepgram. Found 1 project.");
        assert_eq!(deepgram_connection_message(3), "Connected to Deepgram. Found 3 projects.");
    }
}
