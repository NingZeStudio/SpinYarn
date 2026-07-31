use thiserror::Error;

#[derive(Error, Debug)]
pub enum ApiError {
    #[error("Internal error: {0}")]
    Internal(String),
}

impl axum::response::IntoResponse for ApiError {
    fn into_response(self) -> axum::response::Response {
        let body = axum::Json(serde_json::json!({
            "success": false,
            "error": {
                "code": "INTERNAL_ERROR",
                "message": self.to_string(),
            }
        }));

        (axum::http::StatusCode::INTERNAL_SERVER_ERROR, body).into_response()
    }
}
