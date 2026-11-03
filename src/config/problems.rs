//! Config values that Save rejects. Startup and the Settings window report
//! the same problems, so a hand-edited `config.toml` is never applied or
//! ignored without a message.

use std::fmt;

use super::{Config, MIC_GAIN_MAX_DB, MIC_GAIN_MIN_DB};
use crate::correction_binding::CorrectionBinding;
use crate::hotkey_binding::parse_hotkey_binding;

/// One invalid value, named by its config key.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ConfigProblem {
    pub key: &'static str,
    pub message: String,
}

impl ConfigProblem {
    pub fn new(key: &'static str, message: String) -> Self {
        Self { key, message }
    }
}

impl fmt::Display for ConfigProblem {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}: {}", self.key, self.message)
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
                format!("\"{}\" is not a valid shortcut: {}", config.ui.hotkey, error),
            ));
            None
        }
    };
    let correction_key = match CorrectionBinding::parse(&config.ui.correction_key) {
        Ok(binding) => Some(binding),
        Err(error) => {
            problems.push(ConfigProblem::new(
                "ui.correction_key",
                format!("\"{}\" is not valid: {}", config.ui.correction_key, error),
            ));
            None
        }
    };
    if let (Some(record_hotkey), Some(correction_key)) = (record_hotkey, correction_key) {
        if correction_key.overlaps(record_hotkey) {
            problems.push(ConfigProblem::new(
                "ui.correction_key",
                format!(
                    "\"{}\" shares a key with ui.hotkey \"{}\"; they must be different",
                    config.ui.correction_key, config.ui.hotkey
                ),
            ));
        }
    }

    if !is_positive(config.ui.font_size) {
        problems.push(ConfigProblem::new(
            "ui.font_size",
            format!("{} is not a positive number", config.ui.font_size),
        ));
    }
    if let Some(footer_font_size) = config.ui.footer_font_size {
        if !is_positive(footer_font_size) {
            problems.push(ConfigProblem::new(
                "ui.footer_font_size",
                format!("{} is not a positive number", footer_font_size),
            ));
        }
    }

    problems.extend(mic_gain_problem(config.mic.gain));

    let transformation_enabled = config
        .transformation
        .provider
        .as_deref()
        .is_some_and(|provider| !provider.trim().is_empty());
    if transformation_enabled {
        if let Err(error) = config.resolve_transformation_config() {
            problems.push(ConfigProblem::new("transformation.provider", error));
        }
        match parse_hotkey_binding(&config.transformation.hotkey) {
            Ok(transform_hotkey) => {
                if record_hotkey == Some(transform_hotkey) {
                    problems.push(ConfigProblem::new(
                        "transformation.hotkey",
                        format!(
                            "\"{}\" is also ui.hotkey; they must be different",
                            config.transformation.hotkey
                        ),
                    ));
                }
                if correction_key.is_some_and(|key| key.overlaps(transform_hotkey)) {
                    problems.push(ConfigProblem::new(
                        "transformation.hotkey",
                        format!(
                            "\"{}\" shares a key with ui.correction_key \"{}\"; they must be different",
                            config.transformation.hotkey, config.ui.correction_key
                        ),
                    ));
                }
            }
            Err(error) => problems.push(ConfigProblem::new(
                "transformation.hotkey",
                format!(
                    "\"{}\" is not a valid shortcut: {}",
                    config.transformation.hotkey, error
                ),
            )),
        }
    }

    problems
}

/// The problem with a `mic.gain` outside the range the Settings slider
/// covers, including NaN and infinity. The audio stream refuses to start
/// with such a gain instead of amplifying by it or substituting another.
pub fn mic_gain_problem(gain_db: f32) -> Option<ConfigProblem> {
    (!(MIC_GAIN_MIN_DB..=MIC_GAIN_MAX_DB).contains(&gain_db)).then(|| {
        ConfigProblem::new(
            "mic.gain",
            format!(
                "{} dB is outside the {} to {} dB range",
                gain_db, MIC_GAIN_MIN_DB, MIC_GAIN_MAX_DB
            ),
        )
    })
}

fn is_positive(value: f64) -> bool {
    value.is_finite() && value > 0.0
}

#[cfg(test)]
mod tests {
    use super::{mic_gain_problem, problems, ConfigProblem};
    use crate::config::Config;

    fn keys(config: &Config) -> Vec<&'static str> {
        problems(config).into_iter().map(|problem| problem.key).collect()
    }

    #[test]
    fn the_default_config_has_no_problems() {
        assert_eq!(problems(&Config::default()), Vec::<ConfigProblem>::new());
    }

    #[test]
    fn an_out_of_range_gain_is_one_problem_naming_the_key_and_range() {
        let mut config = Config::default();
        config.mic.gain = 40.0;

        assert_eq!(
            problems(&config)
                .iter()
                .map(ToString::to_string)
                .collect::<Vec<_>>(),
            ["mic.gain: 40 dB is outside the 0 to 10 dB range"]
        );
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

        let problems = problems(&config);
        assert_eq!(problems.len(), 1, "{problems:?}");
        assert_eq!(problems[0].key, "ui.hotkey");
        assert!(
            problems[0].message.starts_with("\"Ctrl+Banana\" is not a valid shortcut: "),
            "{}",
            problems[0].message
        );
    }

    #[test]
    fn a_correction_key_equal_to_the_record_hotkey_is_one_problem() {
        let mut config = Config::default();
        config.ui.hotkey = "F5".to_owned();
        config.ui.correction_key = "F5".to_owned();

        assert_eq!(
            problems(&config),
            [ConfigProblem {
                key: "ui.correction_key",
                message: "\"F5\" shares a key with ui.hotkey \"F5\"; they must be different"
                    .to_owned(),
            }]
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
            problems(&config)
                .iter()
                .map(ToString::to_string)
                .collect::<Vec<_>>(),
            [
                "ui.font_size: 0 is not a positive number",
                "ui.footer_font_size: NaN is not a positive number",
            ]
        );
    }

    #[test]
    fn an_unsupported_provider_is_one_problem_listing_the_supported_providers() {
        let mut config = Config::default();
        config.transformation.provider = Some("galadriel".to_owned());

        let problems = problems(&config);
        assert_eq!(problems.len(), 1, "{problems:?}");
        assert_eq!(problems[0].key, "transformation.provider");
        assert!(
            problems[0]
                .message
                .starts_with("unsupported transformation provider 'galadriel'; supported providers: anthropic,"),
            "{}",
            problems[0].message
        );
    }

    #[test]
    fn transform_hotkey_problems_count_only_with_a_provider() {
        let mut config = Config::default();
        config.transformation.hotkey = config.ui.hotkey.clone();
        assert_eq!(keys(&config), Vec::<&str>::new());

        config.transformation.provider = Some("openai".to_owned());
        assert_eq!(keys(&config), ["transformation.hotkey"]);

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
