// The canonical driving contract, embedded at compile time: the binary can
// only ever serve the contract text of the commit it was built from —
// "served at the deployed sha" holds by construction (DEC-06).
use axum::http::{header, HeaderValue};
use axum::response::IntoResponse;

pub const TEXT: &str = include_str!("../../docs/sigiled-contract.md");

pub async fn serve() -> impl IntoResponse {
    let version = HeaderValue::from_str(&crate::version())
        .unwrap_or_else(|_| HeaderValue::from_static("unknown"));
    (
        [
            (
                header::CONTENT_TYPE,
                HeaderValue::from_static("text/markdown; charset=utf-8"),
            ),
            (
                header::HeaderName::from_static("x-sigiled-version"),
                version,
            ),
        ],
        TEXT,
    )
}
