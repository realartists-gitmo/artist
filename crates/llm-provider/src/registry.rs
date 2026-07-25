use serde::{Deserialize, Serialize};

/// Rig providers that expose completion models (Voyage AI is embedding-only).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProviderKind {
    Anthropic,
    Azure,
    Chatgpt,
    Cohere,
    Copilot,
    Deepseek,
    Gemini,
    Groq,
    Huggingface,
    Hyperbolic,
    Llamafile,
    Minimax,
    Mira,
    Mistral,
    Moonshot,
    Ollama,
    Openai,
    Openrouter,
    Perplexity,
    Together,
    Xai,
    Xiaomimimo,
    Zai,
}

impl ProviderKind {
    pub fn slug(self) -> &'static str {
        match self {
            Self::Anthropic => "anthropic",
            Self::Azure => "azure",
            Self::Chatgpt => "chatgpt",
            Self::Cohere => "cohere",
            Self::Copilot => "copilot",
            Self::Deepseek => "deepseek",
            Self::Gemini => "gemini",
            Self::Groq => "groq",
            Self::Huggingface => "huggingface",
            Self::Hyperbolic => "hyperbolic",
            Self::Llamafile => "llamafile",
            Self::Minimax => "minimax",
            Self::Mira => "mira",
            Self::Mistral => "mistral",
            Self::Moonshot => "moonshot",
            Self::Ollama => "ollama",
            Self::Openai => "openai",
            Self::Openrouter => "openrouter",
            Self::Perplexity => "perplexity",
            Self::Together => "together",
            Self::Xai => "xai",
            Self::Xiaomimimo => "xiaomimimo",
            Self::Zai => "zai",
        }
    }
}

pub struct ProviderMetadata {
    pub kind: ProviderKind,
    pub display_name: &'static str,
    pub default_base_url: Option<&'static str>,
}

pub const PROVIDERS: &[ProviderMetadata] = &[
    entry(
        ProviderKind::Anthropic,
        "Anthropic",
        "https://api.anthropic.com/",
    ),
    entry(
        ProviderKind::Azure,
        "Azure OpenAI",
        "https://example.openai.azure.com/",
    ),
    entry(
        ProviderKind::Chatgpt,
        "ChatGPT",
        "https://chatgpt.com/backend-api/codex/",
    ),
    entry(ProviderKind::Cohere, "Cohere", "https://api.cohere.com/v2/"),
    entry(
        ProviderKind::Copilot,
        "GitHub Copilot",
        "https://api.githubcopilot.com/",
    ),
    entry(
        ProviderKind::Deepseek,
        "DeepSeek",
        "https://api.deepseek.com/",
    ),
    entry(
        ProviderKind::Gemini,
        "Google Gemini",
        "https://generativelanguage.googleapis.com/",
    ),
    entry(
        ProviderKind::Groq,
        "Groq",
        "https://api.groq.com/openai/v1/",
    ),
    entry(
        ProviderKind::Huggingface,
        "Hugging Face",
        "https://router.huggingface.co/",
    ),
    entry(
        ProviderKind::Hyperbolic,
        "Hyperbolic",
        "https://api.hyperbolic.xyz/",
    ),
    entry(
        ProviderKind::Llamafile,
        "Llamafile",
        "http://localhost:8080/",
    ),
    entry(
        ProviderKind::Minimax,
        "MiniMax",
        "https://api.minimax.io/v1/",
    ),
    entry(ProviderKind::Mira, "Mira", "https://api.mira.network/"),
    entry(ProviderKind::Mistral, "Mistral", "https://api.mistral.ai/"),
    entry(
        ProviderKind::Moonshot,
        "Moonshot",
        "https://api.moonshot.ai/v1/",
    ),
    entry(ProviderKind::Ollama, "Ollama", "http://localhost:11434/"),
    entry(ProviderKind::Openai, "OpenAI", "https://api.openai.com/v1/"),
    entry(
        ProviderKind::Openrouter,
        "OpenRouter",
        "https://openrouter.ai/api/v1/",
    ),
    entry(
        ProviderKind::Perplexity,
        "Perplexity",
        "https://api.perplexity.ai/",
    ),
    entry(
        ProviderKind::Together,
        "Together AI",
        "https://api.together.xyz/v1/",
    ),
    entry(ProviderKind::Xai, "xAI", "https://api.x.ai/v1/"),
    entry(
        ProviderKind::Xiaomimimo,
        "Xiaomi MiMo",
        "https://api.xiaomimimo.com/v1/",
    ),
    entry(ProviderKind::Zai, "Z.ai", "https://api.z.ai/api/paas/v4/"),
];

const fn entry(
    kind: ProviderKind,
    display_name: &'static str,
    base_url: &'static str,
) -> ProviderMetadata {
    ProviderMetadata {
        kind,
        display_name,
        default_base_url: Some(base_url),
    }
}

pub fn metadata(kind: ProviderKind) -> &'static ProviderMetadata {
    PROVIDERS
        .iter()
        .find(|item| item.kind == kind)
        .expect("all provider kinds registered")
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn registry_has_all_unique_completion_providers() {
        assert_eq!(PROVIDERS.len(), 23);
        for (index, provider) in PROVIDERS.iter().enumerate() {
            assert!(
                !PROVIDERS[..index]
                    .iter()
                    .any(|other| other.kind == provider.kind)
            );
            assert_eq!(metadata(provider.kind).display_name, provider.display_name);
            let url = url::Url::parse(provider.default_base_url.expect("completion URL"))
                .expect("registered base URL is valid");
            assert!(matches!(url.scheme(), "http" | "https"));
        }
    }

    #[test]
    fn completion_request_paths_are_not_duplicated() {
        for kind in [
            ProviderKind::Hyperbolic,
            ProviderKind::Llamafile,
            ProviderKind::Mira,
            ProviderKind::Mistral,
        ] {
            let base = url::Url::parse(metadata(kind).default_base_url.unwrap()).unwrap();
            assert_eq!(
                base.join("v1/chat/completions").unwrap().path(),
                "/v1/chat/completions"
            );
        }
        let mimo =
            url::Url::parse(metadata(ProviderKind::Xiaomimimo).default_base_url.unwrap()).unwrap();
        assert_eq!(
            mimo.join("/anthropic/v1/").unwrap().path(),
            "/anthropic/v1/"
        );
    }
}
