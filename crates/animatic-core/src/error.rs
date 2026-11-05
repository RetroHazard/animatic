use std::path::PathBuf;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
    #[error("database error: {0}")]
    Db(#[from] rusqlite::Error),
    #[error("legacy databank: {0}")]
    Legacy(String),
    #[error("settings: {0}")]
    Settings(String),
    #[error("XML: {0}")]
    Xml(String),
    #[error("JSON: {0}")]
    Json(#[from] serde_json::Error),
    #[error("rename script: {0}")]
    Rename(String),
    #[error("file not found: {0}")]
    NotFound(PathBuf),
    #[error("cancelled")]
    Cancelled,
    #[error("{0}")]
    Other(String),
}

pub type Result<T, E = Error> = std::result::Result<T, E>;

impl From<quick_xml::Error> for Error {
    fn from(e: quick_xml::Error) -> Self {
        Error::Xml(e.to_string())
    }
}
