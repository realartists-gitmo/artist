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

pub struct ProviderMetadata {
    pub kind: ProviderKind,
    pub display_name: &'static str,
    pub default_base_url: Option<&'static str>,
}

pub const PROVIDERS: &[ProviderMetadata] = &[
    entry(ProviderKind::Anthropic, "Anthropic"),
    entry(ProviderKind::Azure, "Azure OpenAI"),
    entry(ProviderKind::Chatgpt, "ChatGPT"),
    entry(ProviderKind::Cohere, "Cohere"),
    entry(ProviderKind::Copilot, "GitHub Copilot"),
    entry(ProviderKind::Deepseek, "DeepSeek"),
    entry(ProviderKind::Gemini, "Google Gemini"),
    entry(ProviderKind::Groq, "Groq"),
    entry(ProviderKind::Huggingface, "Hugging Face"),
    entry(ProviderKind::Hyperbolic, "Hyperbolic"),
    entry(ProviderKind::Llamafile, "Llamafile"),
    entry(ProviderKind::Minimax, "MiniMax"),
    entry(ProviderKind::Mira, "Mira"),
    entry(ProviderKind::Mistral, "Mistral"),
    entry(ProviderKind::Moonshot, "Moonshot"),
    entry(ProviderKind::Ollama, "Ollama"),
    entry(ProviderKind::Openai, "OpenAI"),
    entry(ProviderKind::Openrouter, "OpenRouter"),
    entry(ProviderKind::Perplexity, "Perplexity"),
    entry(ProviderKind::Together, "Together AI"),
    entry(ProviderKind::Xai, "xAI"),
    entry(ProviderKind::Xiaomimimo, "Xiaomi MiMo"),
    entry(ProviderKind::Zai, "Z.ai"),
];

const fn entry(kind: ProviderKind, display_name: &'static str) -> ProviderMetadata {
    ProviderMetadata {
        kind,
        display_name,
        default_base_url: None,
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
        }
    }
}
