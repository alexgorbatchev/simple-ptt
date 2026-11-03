use std::fmt;
use std::str::FromStr;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TransformationProvider {
    Anthropic,
    Cohere,
    Deepseek,
    Gemini,
    Groq,
    Huggingface,
    Hyperbolic,
    Mira,
    Mistral,
    Moonshot,
    Ollama,
    Openai,
    Openrouter,
    Perplexity,
    Together,
    Xai,
}

impl TransformationProvider {
    pub const ALL: [Self; 16] = [
        Self::Anthropic,
        Self::Cohere,
        Self::Deepseek,
        Self::Gemini,
        Self::Groq,
        Self::Huggingface,
        Self::Hyperbolic,
        Self::Mira,
        Self::Mistral,
        Self::Moonshot,
        Self::Ollama,
        Self::Openai,
        Self::Openrouter,
        Self::Perplexity,
        Self::Together,
        Self::Xai,
    ];

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Anthropic => "anthropic",
            Self::Cohere => "cohere",
            Self::Deepseek => "deepseek",
            Self::Gemini => "gemini",
            Self::Groq => "groq",
            Self::Huggingface => "huggingface",
            Self::Hyperbolic => "hyperbolic",
            Self::Mira => "mira",
            Self::Mistral => "mistral",
            Self::Moonshot => "moonshot",
            Self::Ollama => "ollama",
            Self::Openai => "openai",
            Self::Openrouter => "openrouter",
            Self::Perplexity => "perplexity",
            Self::Together => "together",
            Self::Xai => "xai",
        }
    }

    /// Model used when `transformation.model` is unset. Only providers with
    /// a stable small model have one; the others need `transformation.model`.
    pub const fn default_model(self) -> Option<&'static str> {
        match self {
            Self::Anthropic => Some("claude-haiku-5-5"),
            Self::Gemini => Some("gemini-3.5-flash-lite"),
            Self::Openai => Some("gpt-5.4-mini"),
            Self::Cohere
            | Self::Deepseek
            | Self::Groq
            | Self::Huggingface
            | Self::Hyperbolic
            | Self::Mira
            | Self::Mistral
            | Self::Moonshot
            | Self::Ollama
            | Self::Openrouter
            | Self::Perplexity
            | Self::Together
            | Self::Xai => None,
        }
    }

    /// The model for `configured_model`, which is blank when unset, or
    /// `None` when it is unset and this provider has no default.
    pub fn resolve_model(self, configured_model: &str) -> Option<String> {
        match configured_model.trim() {
            "" => self.default_model().map(str::to_owned),
            model => Some(model.to_owned()),
        }
    }

    pub const fn api_key_env_vars(self) -> &'static [&'static str] {
        match self {
            Self::Anthropic => &["ANTHROPIC_API_KEY"],
            Self::Cohere => &["COHERE_API_KEY"],
            Self::Deepseek => &["DEEPSEEK_API_KEY"],
            Self::Gemini => &["GEMINI_API_KEY", "GOOGLE_API_KEY"],
            Self::Groq => &["GROQ_API_KEY"],
            Self::Huggingface => &["HUGGINGFACE_API_KEY", "HF_TOKEN"],
            Self::Hyperbolic => &["HYPERBOLIC_API_KEY"],
            Self::Mira => &["MIRA_API_KEY"],
            Self::Mistral => &["MISTRAL_API_KEY"],
            Self::Moonshot => &["MOONSHOT_API_KEY"],
            Self::Ollama => &[],
            Self::Openai => &["OPENAI_API_KEY"],
            Self::Openrouter => &["OPENROUTER_API_KEY"],
            Self::Perplexity => &["PERPLEXITY_API_KEY"],
            Self::Together => &["TOGETHER_API_KEY"],
            Self::Xai => &["XAI_API_KEY"],
        }
    }
}

impl FromStr for TransformationProvider {
    type Err = String;

    fn from_str(name: &str) -> Result<Self, Self::Err> {
        let name = name.trim();
        Self::ALL
            .into_iter()
            .find(|provider| provider.as_str().eq_ignore_ascii_case(name))
            .ok_or_else(|| {
                format!(
                    "unsupported transformation provider '{}'; supported providers: {}",
                    name,
                    Self::ALL.map(Self::as_str).join(", ")
                )
            })
    }
}

impl fmt::Display for TransformationProvider {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

#[cfg(test)]
mod tests {
    use super::TransformationProvider;

    #[test]
    fn an_unset_model_resolves_to_the_providers_own_default() {
        assert_eq!(TransformationProvider::Openai.resolve_model(""), Some("gpt-5.4-mini".to_owned()));
        assert_eq!(
            TransformationProvider::Anthropic.resolve_model("  "),
            Some("claude-haiku-5-5".to_owned())
        );
        assert_eq!(
            TransformationProvider::Gemini.resolve_model(""),
            Some("gemini-3.5-flash-lite".to_owned())
        );
    }

    #[test]
    fn a_provider_without_a_default_requires_a_model() {
        assert_eq!(TransformationProvider::Ollama.resolve_model(""), None);
        assert_eq!(
            TransformationProvider::Ollama.resolve_model(" llama3.2 "),
            Some("llama3.2".to_owned())
        );
    }

    #[test]
    fn provider_parser_canonicalizes_supported_names_and_rejects_unknown_names() {
        for provider in TransformationProvider::ALL {
            for name in [
                provider.to_string(),
                format!(" {} ", provider.to_string().to_ascii_uppercase()),
            ] {
                assert_eq!(name.parse(), Ok(provider));
            }
        }
        assert_eq!(" OpenAI ".parse(), Ok(TransformationProvider::Openai));
        assert_eq!(
            "galadriel".parse::<TransformationProvider>(),
            Err("unsupported transformation provider 'galadriel'; supported providers: \
                 anthropic, cohere, deepseek, gemini, groq, huggingface, hyperbolic, mira, mistral, \
                 moonshot, ollama, openai, openrouter, perplexity, together, xai"
                .to_owned())
        );
    }
}
