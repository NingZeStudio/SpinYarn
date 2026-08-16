use thiserror::Error;

#[derive(Error, Debug)]
pub enum ApiError {
    /// Server-side failure; the inner detail is logged but never exposed in the
    /// response (avoids leaking filesystem paths / implementation details).
    #[error("{0}")]
    Internal(String),
    #[error("Not found: {0}")]
    NotFound(String),
    #[error("Bad request: {0}")]
    BadRequest(String),
}

impl ApiError {
    fn code(&self) -> &'static str {
        match self {
            ApiError::Internal(_) => "INTERNAL_ERROR",
            ApiError::NotFound(_) => "NOT_FOUND",
            ApiError::BadRequest(_) => "BAD_REQUEST",
        }
    }

    fn status(&self) -> axum::http::StatusCode {
        match self {
            ApiError::Internal(_) => axum::http::StatusCode::INTERNAL_SERVER_ERROR,
            ApiError::NotFound(_) => axum::http::StatusCode::NOT_FOUND,
            ApiError::BadRequest(_) => axum::http::StatusCode::BAD_REQUEST,
        }
    }

    /// Client-facing message: NotFound/BadRequest carry their (already
    /// sanitized) reason; Internal returns a generic line — the detail goes to
    /// the logs instead.
    fn public_message(&self) -> String {
        match self {
            ApiError::Internal(detail) => {
                tracing::error!("internal error: {}", detail);
                "internal server error".to_string()
            }
            ApiError::NotFound(m) | ApiError::BadRequest(m) => m.clone(),
        }
    }
}

impl axum::response::IntoResponse for ApiError {
    fn into_response(self) -> axum::response::Response {
        let body = axum::Json(serde_json::json!({
            "success": false,
            "error": {
                "code": self.code(),
                "message": self.public_message(),
            }
        }));

        (self.status(), body).into_response()
    }
}
