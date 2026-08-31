#[derive(Debug, thiserror::Error)]
pub enum AppError {
    #[error("invalid config: {message}")]
    InvalidConfig { message: String },
    #[error("failed to create directory {path}: {source}")]
    CreateDir {
        path: String,
        #[source]
        source: std::io::Error,
    },
    #[error("database error: {message}")]
    Database { message: String },
    #[error("server error: {message}")]
    Server { message: String },
}
