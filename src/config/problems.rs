//! Config values that Save rejects. Startup and the Settings window report
//! the same problems, so a hand-edited `config.toml` is never applied or
//! ignored without a message. Startup names each value by its config key;
//! the settings window names it by its row (`ConfigProblem::describe`).

use std::fmt;

use super::{Config, TransformationProvider, MIC_GAIN_MAX_DB, MIC_GAIN_MIN_DB};
use crate::correction_binding::CorrectionBinding;
use crate::hotkey_binding::parse_hotkey_binding;

/// One invalid value, at its config key.
#[derive(Clone, Debug, PartialEq)]
pub struct ConfigProblem {
    pub key: &'static str,
    pub kind: ProblemKind,
}

/// What is wrong with a value. A kind that refers to another key names it
/// the same way as the problem's own key.
#[derive(Clone, Debug, PartialEq)]
pub enum ProblemKind {
    InvalidShortcut { value: String, error: String },
    InvalidCorrectionKey { value: String, error: String },
    SharesKeyWith { value: String, other_key: &'static str, other_value: String },
    SameAs { value: String, other_key: &'static str },
    NotPositive(f64),
    GainOutOfRange(f32),
    UnsupportedProvider { value: String },
    MissingModel(TransformationProvider),
}

impl ConfigProblem {
    fn new(key: &'static str, kind: ProblemKind) -> Self {
        Self { key, kind }
    }

    /// The problem as `name: what is wrong`, with every config key named by
    /// `name` and every configured value in double quotes.
    pub fn describe(&self, name: impl Fn(&str) -> String) -> String {
        let detail = match &self.kind {
            ProblemKind::InvalidShortcut { value, error } => {
                format!("\"{value}\" is not a valid shortcut: {error}")
            }
            ProblemKind::InvalidCorrectionKey { value, error } => {
                format!("\"{value}\" is not valid: {error}")
            }
            ProblemKind::SharesKeyWith { value, other_key, other_value } => format!(
                "\"{value}\" shares a key with {} \"{other_value}\"; they must be different",
                name(other_key)
            ),
            ProblemKind::SameAs { value, other_key } => format!(
                "\"{value}\" is also {}; they must be different",
                name(other_key)
            ),
            ProblemKind::NotPositive(value) => format!("\"{value}\" is not a positive number"),
            ProblemKind::GainOutOfRange(gain_db) => format!(
                "\"{gain_db} dB\" is outside the {MIC_GAIN_MIN_DB} to {MIC_GAIN_MAX_DB} dB range"
            ),
            ProblemKind::UnsupportedProvider { value } => format!(
                "\"{value}\" is not supported; supported providers: {}",
                TransformationProvider::ALL.map(TransformationProvider::as_str).join(", ")
            ),
            ProblemKind::MissingModel(provider) => format!(
                "required for provider \"{provider}\", which has no built-in default model"
            ),
        };
        format!("{}: {detail}", name(self.key))
    }
}

/// The problem with every key named as it appears in `config.toml`.
impl fmt::Display for ConfigProblem {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.describe(str::to_owned))
    }
}

/// Every problem in `config` that does not need the audio device or a
/// credential to find, in config file order.
pub fn problems(config: &Config) -> Vec<ConfigProblem> {
    let mut problems = Vec::new();

    let record_hotkey = match parse_hotkey_binding(&config.ui.hotkey) {
        Ok(binding) => Some(binding),
        Err(error) => {
            problems.push(ConfigProblem::new(
                "ui.hotkey",
                ProblemKind::InvalidShortcut { value: config.ui.hotkey.clone(), error },
            ));
            None
        }
    };
    let correction_key = match CorrectionBinding::parse(&config.ui.correction_key) {
        Ok(binding) => Some(binding),
        Err(error) => {
            problems.push(ConfigProblem::new(
                "ui.correction_key",
                ProblemKind::InvalidCorrectionKey {
                    value: config.ui.correction_key.clone(),
                    error,
                },
            ));
            None
        }
    };
    if let (Some(record_hotkey), Some(correction_key)) = (record_hotkey, correction_key) {
        if correction_key.overlaps(record_hotkey) {
            problems.push(ConfigProblem::new(
                "ui.correction_key",
                ProblemKind::SharesKeyWith {
                    value: config.ui.correction_key.clone(),
                    other_key: "ui.hotkey",
                    other_value: config.ui.hotkey.clone(),
                },
            ));
        }
    }

    if !is_positive(config.ui.font_size) {
        problems.push(ConfigProblem::new(
            "ui.font_size",
            ProblemKind::NotPositive(config.ui.font_size),
        ));
    }
    if let Some(footer_font_size) = config.ui.footer_font_size {
        if !is_positive(footer_font_size) {
            problems.push(ConfigProblem::new(
                "ui.footer_font_size",
                ProblemKind::NotPositive(footer_font_size),
            ));
        }
    }

    problems.extend(mic_gain_problem(config.mic.gain));

    let configured_provider = config
        .transformation
        .provider
        .as_deref()
        .map(str::trim)
        .filter(|provider| !provider.is_empty());
    if let Some(configured_provider) = configured_provider {
        match configured_provider.parse::<TransformationProvider>() {
            Ok(provider) => {
                if provider.resolve_model(&config.transformation.model).is_none() {
                    problems.push(missing_model_problem(provider));
                }
            }
            Err(_) => problems.push(unsupported_provider_problem(configured_provider)),
        }
        match parse_hotkey_binding(&config.transformation.hotkey) {
            Ok(transform_hotkey) => {
                if record_hotkey == Some(transform_hotkey) {
                    problems.push(ConfigProblem::new(
                        "transformation.hotkey",
                        ProblemKind::SameAs {
                            value: config.transformation.hotkey.clone(),
                            other_key: "ui.hotkey",
                        },
                    ));
                }
                if correction_key.is_some_and(|key| key.overlaps(transform_hotkey)) {
                    problems.push(ConfigProblem::new(
                        "transformation.hotkey",
                        ProblemKind::SharesKeyWith {
                            value: config.transformation.hotkey.clone(),
                            other_key: "ui.correction_key",
                            other_value: config.ui.correction_key.clone(),
                        },
                    ));
                }
            }
            Err(error) => problems.push(ConfigProblem::new(
                "transformation.hotkey",
                ProblemKind::InvalidShortcut {
                    value: config.transformation.hotkey.clone(),
                    error,
                },
            )),
        }
    }

    problems
}

/// The problem with a `mic.gain` outside the range the Settings slider
/// covers, including NaN and infinity. The audio stream refuses to start
/// with such a gain instead of amplifying by it or substituting another.
pub fn mic_gain_problem(gain_db: f32) -> Option<ConfigProblem> {
    (!(MIC_GAIN_MIN_DB..=MIC_GAIN_MAX_DB).contains(&gain_db))
        .then(|| ConfigProblem::new("mic.gain", ProblemKind::GainOutOfRange(gain_db)))
}

/// The problem with an unset `transformation.model` for a provider that has
/// no default model.
pub fn missing_model_problem(provider: TransformationProvider) -> ConfigProblem {
    ConfigProblem::new("transformation.model", ProblemKind::MissingModel(provider))
}

/// The problem with a `transformation.provider` that is not supported.
pub fn unsupported_provider_problem(provider: &str) -> ConfigProblem {
    ConfigProblem::new(
        "transformation.provider",
        ProblemKind::UnsupportedProvider { value: provider.trim().to_owned() },
    )
}

fn is_positive(value: f64) -> bool {
    value.is_finite() && value > 0.0
}

#[cfg(test)]
mod tests {
    use super::{mic_gain_problem, problems, ConfigProblem, ProblemKind};
    use crate::config::Config;

    fn keys(config: &Config) -> Vec<&'static str> {
        problems(config).into_iter().map(|problem| problem.key).collect()
    }

    fn messages(config: &Config) -> Vec<String> {
        problems(config).iter().map(ToString::to_string).collect()
    }

    #[test]
    fn the_default_config_has_no_problems() {
        assert_eq!(problems(&Config::default()), Vec::<ConfigProblem>::new());
    }

    #[test]
    fn an_out_of_range_gain_is_one_problem_naming_the_key_and_range() {
        let mut config = Config::default();
        config.mic.gain = 40.0;

        assert_eq!(messages(&config), ["mic.gain: \"40 dB\" is outside the 0 to 10 dB range"]);
    }

    #[test]
    fn gains_inside_the_slider_range_are_not_problems() {
        for gain_db in [0.0, 4.5, 10.0] {
            assert_eq!(mic_gain_problem(gain_db), None, "{gain_db}");
        }
        for gain_db in [-0.5, 10.5, f32::NAN, f32::INFINITY, f32::NEG_INFINITY] {
            assert!(mic_gain_problem(gain_db).is_some(), "{gain_db}");
        }
    }

    #[test]
    fn an_unparseable_record_hotkey_is_one_problem_quoting_the_value() {
        let mut config = Config::default();
        config.ui.hotkey = "Ctrl+Banana".to_owned();

        let messages = messages(&config);
        assert_eq!(messages.len(), 1, "{messages:?}");
        assert!(
            messages[0].starts_with("ui.hotkey: \"Ctrl+Banana\" is not a valid shortcut: "),
            "{}",
            messages[0]
        );
    }

    #[test]
    fn a_correction_key_equal_to_the_record_hotkey_is_one_problem() {
        let mut config = Config::default();
        config.ui.hotkey = "F5".to_owned();
        config.ui.correction_key = "F5".to_owned();

        assert_eq!(
            messages(&config),
            ["ui.correction_key: \"F5\" shares a key with ui.hotkey \"F5\"; they must be different"]
        );
    }

    #[test]
    fn describe_names_every_key_the_problem_mentions() {
        let mut config = Config::default();
        config.ui.hotkey = "F5".to_owned();
        config.ui.correction_key = "F5".to_owned();

        assert_eq!(
            problems(&config)[0].describe(|key| key.to_ascii_uppercase()),
            "UI.CORRECTION_KEY: \"F5\" shares a key with UI.HOTKEY \"F5\"; they must be different"
        );
    }

    #[test]
    fn a_modifier_correction_chord_inside_the_record_hotkey_is_a_problem() {
        let mut config = Config::default();
        config.ui.correction_key = "Alt+Cmd".to_owned();
        assert_eq!(keys(&config), Vec::<&str>::new());

        config.ui.hotkey = "Alt+Cmd+F5".to_owned();
        assert_eq!(keys(&config), ["ui.correction_key"]);
    }

    #[test]
    fn zero_and_non_finite_font_sizes_are_problems() {
        let mut config = Config::default();
        config.ui.font_size = 0.0;
        config.ui.footer_font_size = Some(f64::NAN);

        assert_eq!(
            messages(&config),
            [
                "ui.font_size: \"0\" is not a positive number",
                "ui.footer_font_size: \"NaN\" is not a positive number",
            ]
        );
    }

    #[test]
    fn an_unsupported_provider_is_one_problem_listing_the_supported_providers() {
        let mut config = Config::default();
        config.transformation.provider = Some(" galadriel ".to_owned());

        let messages = messages(&config);
        assert_eq!(messages.len(), 1, "{messages:?}");
        assert!(
            messages[0].starts_with(
                "transformation.provider: \"galadriel\" is not supported; supported providers: anthropic,"
            ),
            "{}",
            messages[0]
        );
    }

    #[test]
    fn an_unset_model_is_a_problem_only_for_a_provider_without_a_default() {
        let mut config = Config::default();
        config.transformation.provider = Some("openai".to_owned());
        assert_eq!(keys(&config), Vec::<&str>::new());

        config.transformation.provider = Some("ollama".to_owned());
        assert_eq!(
            messages(&config),
            ["transformation.model: required for provider \"ollama\", which has no built-in default model"]
        );

        config.transformation.model = "llama3.2".to_owned();
        assert_eq!(keys(&config), Vec::<&str>::new());
    }

    #[test]
    fn transform_hotkey_problems_count_only_with_a_provider() {
        let mut config = Config::default();
        config.transformation.hotkey = config.ui.hotkey.clone();
        assert_eq!(keys(&config), Vec::<&str>::new());

        config.transformation.provider = Some("openai".to_owned());
        assert_eq!(
            problems(&config)[0].kind,
            ProblemKind::SameAs { value: "F5".to_owned(), other_key: "ui.hotkey" }
        );

        config.transformation.hotkey = "Nope".to_owned();
        assert_eq!(keys(&config), ["transformation.hotkey"]);
    }

    #[test]
    fn every_problem_is_reported_at_once() {
        let mut config = Config::default();
        config.ui.hotkey = String::new();
        config.ui.font_size = -1.0;
        config.mic.gain = 11.0;
        config.transformation.provider = Some("galadriel".to_owned());

        assert_eq!(
            keys(&config),
            ["ui.hotkey", "ui.font_size", "mic.gain", "transformation.provider"]
        );
    }
}
