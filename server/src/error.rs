use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde_json::json;

pub struct ApiError {
    pub status: StatusCode,
    pub detail: String,
}

impl ApiError {
    pub fn bad_request(detail: impl Into<String>) -> Self {
        Self { status: StatusCode::BAD_REQUEST, detail: detail.into() }
    }
    pub fn not_found(detail: impl Into<String>) -> Self {
        Self { status: StatusCode::NOT_FOUND, detail: detail.into() }
    }
    pub fn forbidden(detail: impl Into<String>) -> Self {
        Self { status: StatusCode::FORBIDDEN, detail: detail.into() }
    }
    pub fn internal(detail: impl Into<String>) -> Self {
        Self { status: StatusCode::INTERNAL_SERVER_ERROR, detail: detail.into() }
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        (self.status, Json(json!({ "detail": self.detail }))).into_response()
    }
}

impl From<std::io::Error> for ApiError {
    fn from(e: std::io::Error) -> Self {
        match e.kind() {
            std::io::ErrorKind::NotFound => ApiError::not_found(e.to_string()),
            std::io::ErrorKind::PermissionDenied => ApiError::forbidden(e.to_string()),
            _ => ApiError::internal(e.to_string()),
        }
    }
}
