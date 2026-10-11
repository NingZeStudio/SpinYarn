use axum::http::{HeaderValue, StatusCode};
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
    #[error("Forbidden: {0}")]
    Forbidden(String),
    #[error("Too many requests: {0}")]
    TooManyRequests(String),
    #[error("Unprocessable entity: {0}")]
    UnprocessableEntity(String),
}

impl ApiError {
    fn code(&self) -> &'static str {
        match self {
            ApiError::Internal(_) => "INTERNAL_ERROR",
            ApiError::NotFound(_) => "NOT_FOUND",
            ApiError::BadRequest(_) => "BAD_REQUEST",
            ApiError::Forbidden(_) => "FORBIDDEN",
            ApiError::TooManyRequests(_) => "TOO_MANY_REQUESTS",
            ApiError::UnprocessableEntity(_) => "UNPROCESSABLE_ENTITY",
        }
    }

    fn status(&self) -> StatusCode {
        match self {
            ApiError::Internal(_) => StatusCode::INTERNAL_SERVER_ERROR,
            ApiError::NotFound(_) => StatusCode::NOT_FOUND,
            ApiError::BadRequest(_) => StatusCode::BAD_REQUEST,
            ApiError::Forbidden(_) => StatusCode::FORBIDDEN,
            ApiError::TooManyRequests(_) => StatusCode::TOO_MANY_REQUESTS,
            ApiError::UnprocessableEntity(_) => StatusCode::UNPROCESSABLE_ENTITY,
        }
    }

    /// Client-facing message: NotFound/BadRequest/Forbidden/TooManyRequests/
    /// UnprocessableEntity carry their (already sanitized) reason; Internal
    /// returns a generic line — the detail goes to the logs instead.
    fn public_message(&self) -> String {
        match self {
            ApiError::Internal(detail) => {
                tracing::error!("internal error: {}", detail);
                "internal server error".to_string()
            }
            ApiError::NotFound(m)
            | ApiError::BadRequest(m)
            | ApiError::Forbidden(m)
            | ApiError::TooManyRequests(m)
            | ApiError::UnprocessableEntity(m) => m.clone(),
        }
    }
}

impl axum::response::IntoResponse for ApiError {
    fn into_response(self) -> axum::response::Response {
        let is_rate_limited = matches!(self, ApiError::TooManyRequests(_));
        let body = axum::Json(serde_json::json!({
            "success": false,
            "error": {
                "code": self.code(),
                "message": self.public_message(),
            }
        }));

        let mut resp = (self.status(), body).into_response();
        if is_rate_limited {
            // Tell clients when to retry (matches the 30s gate timeout).
            resp.headers_mut().insert(
                axum::http::header::RETRY_AFTER,
                HeaderValue::from_static("30"),
            );
        }
        resp
    }
}

/// Invalid JSON / missing fields arrive here (via `Result<Json<T>,
/// JsonRejection>` extractors) so they keep the envelope shape instead of
/// Axum's default plain-text rejection. Matched on the rejection variant
/// (not the Display text, which carries no serde detail in axum 0.7):
/// data errors (missing/wrong-typed fields) stay 422 (keeps `test.sh`
/// green); syntax errors and the rest are 400.
impl From<axum::extract::rejection::JsonRejection> for ApiError {
    fn from(rej: axum::extract::rejection::JsonRejection) -> Self {
        use axum::extract::rejection::JsonRejection as R;
        let msg = rej.to_string();
        match rej {
            R::JsonDataError(_) => ApiError::UnprocessableEntity(msg),
            _ => ApiError::BadRequest(msg),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::response::IntoResponse;

    #[test]
    fn test_forbidden_status_and_code() {
        let resp = ApiError::Forbidden("admin required".to_string()).into_response();
        assert_eq!(resp.status(), StatusCode::FORBIDDEN);
    }

    #[test]
    fn test_too_many_requests_has_retry_after() {
        let resp = ApiError::TooManyRequests("busy".to_string()).into_response();
        assert_eq!(resp.status(), StatusCode::TOO_MANY_REQUESTS);
        let v = resp.headers().get(axum::http::header::RETRY_AFTER).unwrap();
        assert_eq!(v, "30");
    }

    #[test]
    fn test_bad_request_status() {
        let resp = ApiError::BadRequest("bad".to_string()).into_response();
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    }
}
