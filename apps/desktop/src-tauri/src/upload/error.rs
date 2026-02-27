/// Error type for HuggingFace upload operations.
#[derive(Debug, thiserror::Error)]
pub enum HfError {
    #[error("HTTP error: {0}")]
    Http(String),
    #[error("unauthorized: invalid or missing HuggingFace token")]
    Unauthorized,
    #[error("clip quality {score:.2} is below threshold {threshold:.2}")]
    BelowQualityThreshold { score: f64, threshold: f64 },
    #[error("request error: {0}")]
    Request(#[from] reqwest::Error),
    #[error("JSON error: {0}")]
    Json(#[from] serde_json::Error),
    #[error("clip format error: {0}")]
    ClipFormat(#[from] crate::clip::format::ClipFormatError),
    #[error("upload consent not given")]
    ConsentRequired,
    #[error("empty token")]
    EmptyToken,
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
}

impl serde::Serialize for HfError {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        serializer.serialize_str(&self.to_string())
    }
}
