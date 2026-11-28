//! Provider-neutral web search for agents. Backends own credentials, transport,
//! and search tuning; callers receive ranked sources they can read and cite.

use serde::{Deserialize, Serialize};

use crate::BoxFuture;

/// A web search with an optional restriction on source domains.
///
/// ```
/// use magicmtn_sdk::SearchRequest;
///
/// let mut request = SearchRequest::new("Rust async trait documentation");
/// request.include_domains = vec!["rust-lang.org".into()];
/// request.max_results = 5;
/// ```
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SearchRequest {
    /// Natural-language query. Must contain at least one non-whitespace character.
    pub query: String,
    /// Optional keyword queries to guide retrieval for the natural-language
    /// `query`. Backends may use these hints or search with `query` alone.
    /// Empty lets the backend choose how to retrieve sources.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub search_queries: Vec<String>,
    /// Maximum number of results to return. Must be positive; defaults to 10.
    #[serde(default = "default_max_results")]
    pub max_results: u32,
    /// Restrict results to these domains and their subdomains. Empty means any
    /// domain. Entries are bare hostnames, such as `example.com`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub include_domains: Vec<String>,
    /// Exclude these domains and their subdomains, taking precedence over
    /// `include_domains`. Entries use the same bare-hostname format.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub exclude_domains: Vec<String>,
}

fn default_max_results() -> u32 {
    10
}

impl SearchRequest {
    /// Search across all domains with a maximum of 10 results.
    pub fn new(query: impl Into<String>) -> Self {
        Self {
            query: query.into(),
            search_queries: vec![],
            max_results: default_max_results(),
            include_domains: vec![],
            exclude_domains: vec![],
        }
    }
}

/// A source an agent can evaluate, cite, or retrieve for further reading.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SearchResult {
    /// Absolute HTTP(S) source URL, suitable for citations and follow-up reads.
    pub url: String,
    /// Source title; empty when the source has no title.
    pub title: String,
    /// Query-relevant source excerpts or a search snippet, in plain text or
    /// Markdown. Keep this focused for the agent's context window. Empty when
    /// no excerpt is available. This is source material, not an instruction to
    /// the agent or a generated answer synthesizing multiple sources.
    pub content: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SearchResponse {
    /// Results in descending relevance order, with at most `max_results`
    /// entries. An empty list means the search succeeded with no matches.
    pub results: Vec<SearchResult>,
}

/// Search the web for ranked, citable sources.
///
/// Implementations must honor the result limit and domain filters, including
/// case-insensitive hostname matching at label boundaries. They must return an
/// error for invalid requests, unsupported constraints, or upstream failures;
/// failures must not be represented as an empty successful search.
pub trait SearchEngineBackend: Send + Sync {
    fn search<'a>(&'a self, request: &'a SearchRequest) -> BoxFuture<'a, SearchResponse>;
}
