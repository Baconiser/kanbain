//! Search settings (Settings → Search): embedders by URL (E6), the
//! reranker (E10) and retrieval defaults (§5 of the embedding plan).

use serde::{Deserialize, Serialize};
use ts_rs::TS;

/// Which HTTP API an embedder speaks.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, TS)]
#[serde(rename_all = "lowercase")]
pub enum EmbedderProvider {
    /// `POST <base>/embeddings` (OpenAI, Ollama, LM Studio, llama.cpp, vLLM,
    /// TEI's OpenAI route). The base URL usually ends in `/v1`.
    Openai,
    /// Hugging Face TEI's native `POST <base>/embed`.
    Tei,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(default)]
pub struct EmbedderConfig {
    pub provider: EmbedderProvider,
    pub base_url: String,
    pub model: String,
    /// Requested output dimension, for models with Matryoshka truncation.
    pub dimensions: Option<u32>,
    /// Prepended to queries, e.g. `search_query: ` for nomic-embed.
    pub query_prefix: String,
    /// Prepended to documents, e.g. `search_document: `.
    pub document_prefix: String,
    pub batch_size: u32,
    /// Inputs are cut to about this many tokens.
    pub max_input_tokens: u32,
    pub timeout_secs: u32,
}

impl Default for EmbedderConfig {
    fn default() -> Self {
        Self {
            provider: EmbedderProvider::Openai,
            base_url: String::new(),
            model: String::new(),
            dimensions: None,
            query_prefix: String::new(),
            document_prefix: String::new(),
            batch_size: 32,
            max_input_tokens: 2048,
            timeout_secs: 30,
        }
    }
}

/// Which HTTP API a reranker speaks.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "lowercase")]
pub enum RerankerProvider {
    /// TEI's `POST <base>/rerank`.
    Tei,
    /// Jina/Cohere-compatible `POST <base>/rerank` (base usually ends in
    /// `/v1`); also llama.cpp `--reranking`.
    Jina,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(default)]
pub struct RerankerConfig {
    pub provider: RerankerProvider,
    pub base_url: String,
    pub model: String,
    pub timeout_secs: u32,
}

impl Default for RerankerConfig {
    fn default() -> Self {
        Self {
            provider: RerankerProvider::Tei,
            base_url: String::new(),
            model: String::new(),
            timeout_secs: 5,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(default)]
pub struct SearchConfig {
    /// Embedder for tickets, comments, attachments and runs. Without one,
    /// search uses keywords only.
    pub text: Option<EmbedderConfig>,
    /// Embedder for code; `None` uses the text embedder (E8).
    pub code: Option<EmbedderConfig>,
    pub reranker: Option<RerankerConfig>,
    /// Candidates taken from each of the vector and keyword searches.
    pub candidates: u32,
    /// Reciprocal rank fusion constant.
    pub rrf_k: u32,
    /// Hits returned when the request names no limit.
    pub limit: u32,
    /// Longest excerpt per hit, in characters (D43).
    pub excerpt_chars: u32,
}

impl Default for SearchConfig {
    fn default() -> Self {
        Self {
            text: None,
            code: None,
            reranker: None,
            candidates: 50,
            rrf_k: 60,
            limit: 10,
            excerpt_chars: 1500,
        }
    }
}

/// Whether `url` points off this machine and its private networks (E9).
pub fn is_remote_url(url: &str) -> bool {
    let Ok(parsed) = reqwest::Url::parse(url.trim()) else {
        return false;
    };
    match parsed.host() {
        Some(url::Host::Domain(host)) => {
            let host = host.to_ascii_lowercase();
            !(host == "localhost"
                || host.ends_with(".localhost")
                || host.ends_with(".local")
                || host.ends_with(".internal")
                || host == "host.docker.internal"
                || !host.contains('.'))
        }
        Some(url::Host::Ipv4(ip)) => {
            !(ip.is_loopback() || ip.is_private() || ip.is_link_local() || ip.is_unspecified())
        }
        Some(url::Host::Ipv6(ip)) => {
            let unique_local = (ip.segments()[0] & 0xfe00) == 0xfc00;
            !(ip.is_loopback() || ip.is_unspecified() || unique_local)
        }
        None => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn remote_urls() {
        for local in [
            "http://localhost:11434/v1",
            "http://127.0.0.1:8080",
            "http://192.168.1.20:8080",
            "http://10.0.0.5",
            "http://tei:80",
            "http://host.docker.internal:8080",
            "http://[::1]:80",
            "http://gpu-box.local:8080",
        ] {
            assert!(!is_remote_url(local), "{local}");
        }
        for remote in [
            "https://api.openai.com/v1",
            "http://8.8.8.8",
            "https://api.jina.ai/v1",
        ] {
            assert!(is_remote_url(remote), "{remote}");
        }
    }
}
