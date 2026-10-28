//! Local hybrid search (D42–D47): an embedding store in its own SQLite file
//! with sqlite-vec vectors and FTS5 keywords, the embedder and reranker
//! clients, chunking, indexing and search.

pub mod chunk;
pub mod client;
pub mod config;
pub mod service;
pub mod sources;
pub mod store;

pub use config::SearchConfig;
pub use service::SearchService;
