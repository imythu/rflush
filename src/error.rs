use crate::rss;

#[derive(Debug, thiserror::Error)]
pub enum AppError {
    #[error("invalid config: {message}")]
    InvalidConfig { message: String },
    #[error("RSS `{name}` fetch failed: {source}")]
    FetchRss {
        name: String,
        #[source]
        source: reqwest::Error,
    },
    #[error("RSS `{name}` returned invalid XML: {source}")]
    ParseRss {
        name: String,
        #[source]
        source: rss::RssParseError,
    },
    #[error("RSS `{name}` has no torrent with guid `{guid}` after refresh")]
    TorrentMissing { name: String, guid: String },
    #[error("failed to create directory {path}: {source}")]
    CreateDir {
        path: String,
        #[source]
        source: std::io::Error,
    },
    #[error("failed to read directory {path}: {source}")]
    ReadDir {
        path: String,
        #[source]
        source: std::io::Error,
    },
    #[error("failed to remove path {path}: {source}")]
    RemovePath {
        path: String,
        #[source]
        source: std::io::Error,
    },
    #[error("database error: {message}")]
    Database { message: String },
    #[error("RSS `{name}` rate-limited by remote server")]
    RateLimited { name: String },
    #[error("server error: {message}")]
    Server { message: String },
}
