use thiserror::Error;

#[derive(Error, Debug)]
pub enum ApiError {
    #[error("Internal error: {0}")]
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
}

impl axum::response::IntoResponse for ApiError {
    fn into_response(self) -> axum::response::Response {
        let body = axum::Json(serde_json::json!({
            "success": false,
            "error": {
                "code": self.code(),
                "message": self.to_string(),
            }
        }));

        (self.status(), body).into_response()
    }
}
