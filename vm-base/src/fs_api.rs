use std::path::{Component, Path, PathBuf};
use std::sync::Arc;

use axum::extract::{Query, State};
use axum::Json;
use base64::Engine;
use serde::{Deserialize, Serialize};

use crate::error::ApiError;
use crate::state::AppState;

/// Normalize without touching the filesystem (target may not exist yet),
/// rejecting any `..` traversal, then require the result to sit under an
/// allowed root (§5: /workspace + declared mounts). The seal is structural:
/// this API simply cannot express a path outside the container's own view.
fn resolve(st: &AppState, raw: &str) -> Result<PathBuf, ApiError> {
    let p = Path::new(raw);
    if !p.is_absolute() {
        return Err(ApiError::bad_request("path must be absolute"));
    }
    let mut clean = PathBuf::new();
    for c in p.components() {
        match c {
            Component::ParentDir => return Err(ApiError::forbidden("path traversal rejected")),
            Component::CurDir => {}
            other => clean.push(other),
        }
    }
    let allowed = std::iter::once(&st.workspace).chain(st.extra_roots.iter());
    for root in allowed {
        if clean.starts_with(root) {
            return Ok(clean);
        }
    }
    Err(ApiError::forbidden(format!("path outside allowed roots: {}", clean.display())))
}

#[derive(Deserialize)]
pub struct PathQuery {
    pub path: String,
}

#[derive(Serialize)]
pub struct Entry {
    pub name: String,
    pub kind: &'static str,
    pub size: u64,
}

pub async fn list(
    State(st): State<Arc<AppState>>,
    Query(q): Query<PathQuery>,
) -> Result<Json<Vec<Entry>>, ApiError> {
    let dir = resolve(&st, &q.path)?;
    let mut rd = tokio::fs::read_dir(&dir).await?;
    let mut out = Vec::new();
    while let Some(e) = rd.next_entry().await? {
        let meta = e.metadata().await?;
        out.push(Entry {
            name: e.file_name().to_string_lossy().into_owned(),
            kind: if meta.is_dir() { "dir" } else { "file" },
            size: meta.len(),
        });
    }
    out.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(Json(out))
}

#[derive(Deserialize)]
pub struct ReadReq {
    pub path: String,
}

#[derive(Serialize)]
pub struct ReadResp {
    pub encoding: &'static str,
    pub content: String,
}

pub async fn read(
    State(st): State<Arc<AppState>>,
    Json(r): Json<ReadReq>,
) -> Result<Json<ReadResp>, ApiError> {
    let path = resolve(&st, &r.path)?;
    let bytes = tokio::fs::read(&path).await?;
    Ok(Json(match String::from_utf8(bytes) {
        Ok(text) => ReadResp { encoding: "utf-8", content: text },
        Err(e) => ReadResp {
            encoding: "base64",
            content: base64::engine::general_purpose::STANDARD.encode(e.into_bytes()),
        },
    }))
}

#[derive(Deserialize)]
pub struct WriteReq {
    pub path: String,
    pub content: String,
    #[serde(default)]
    pub encoding: Option<String>, // "utf-8" (default) | "base64"
}

pub async fn write(
    State(st): State<Arc<AppState>>,
    Json(r): Json<WriteReq>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let path = resolve(&st, &r.path)?;
    let bytes = match r.encoding.as_deref() {
        None | Some("utf-8") => r.content.into_bytes(),
        Some("base64") => base64::engine::general_purpose::STANDARD
            .decode(&r.content)
            .map_err(|e| ApiError::bad_request(format!("bad base64: {e}")))?,
        Some(other) => return Err(ApiError::bad_request(format!("unknown encoding '{other}'"))),
    };
    if let Some(parent) = path.parent() {
        tokio::fs::create_dir_all(parent).await?;
    }
    let written = bytes.len();
    tokio::fs::write(&path, bytes).await?;
    Ok(Json(serde_json::json!({ "written": written })))
}

#[derive(Deserialize)]
pub struct DeleteReq {
    pub path: String,
    #[serde(default)]
    pub recursive: bool,
}

pub async fn delete(
    State(st): State<Arc<AppState>>,
    Json(r): Json<DeleteReq>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let path = resolve(&st, &r.path)?;
    let meta = tokio::fs::symlink_metadata(&path).await?;
    if meta.is_dir() {
        if r.recursive {
            tokio::fs::remove_dir_all(&path).await?;
        } else {
            tokio::fs::remove_dir(&path).await?;
        }
    } else {
        tokio::fs::remove_file(&path).await?;
    }
    Ok(Json(serde_json::json!({ "deleted": true })))
}
