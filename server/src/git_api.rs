use std::process::Stdio;
use std::sync::Arc;

use axum::extract::{Query, State};
use axum::Json;
use serde::{Deserialize, Serialize};
use tokio::process::Command;

use crate::error::ApiError;
use crate::state::AppState;

const US: char = '\u{1f}'; // unit separator
const RS: char = '\u{1e}'; // record separator

/// Run git in the workspace. Deploy key (if present at $GIT_SSH_KEY, injected
/// by MGR at container creation) is wired via GIT_SSH_COMMAND; identity has
/// mgr defaults, overridable by env.
async fn git(st: &AppState, args: &[&str]) -> Result<String, ApiError> {
    let mut cmd = Command::new("git");
    cmd.args(args)
        .current_dir(&st.workspace)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());

    let key = std::env::var("GIT_SSH_KEY").unwrap_or_else(|_| "/secrets/deploy_key".into());
    if std::path::Path::new(&key).exists() {
        cmd.env(
            "GIT_SSH_COMMAND",
            format!("ssh -i {key} -o IdentitiesOnly=yes -o StrictHostKeyChecking=accept-new"),
        );
    }
    for (var, default) in [
        ("GIT_AUTHOR_NAME", "mgr-session"),
        ("GIT_AUTHOR_EMAIL", "mgr@016180.xyz"),
        ("GIT_COMMITTER_NAME", "mgr-session"),
        ("GIT_COMMITTER_EMAIL", "mgr@016180.xyz"),
    ] {
        if std::env::var(var).is_err() {
            cmd.env(var, default);
        }
    }

    let out = cmd.output().await.map_err(|e| ApiError::internal(format!("spawn git: {e}")))?;
    if !out.status.success() {
        return Err(ApiError::bad_request(format!(
            "git {} failed ({}): {}",
            args.first().unwrap_or(&""),
            out.status.code().unwrap_or(-1),
            String::from_utf8_lossy(&out.stderr).trim(),
        )));
    }
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}

#[derive(Serialize)]
pub struct StatusResp {
    pub branch: String,
    pub dirty: bool,
    pub files: Vec<String>,
}

pub async fn status(State(st): State<Arc<AppState>>) -> Result<Json<StatusResp>, ApiError> {
    let raw = git(&st, &["status", "--porcelain=v1", "-b"]).await?;
    let mut lines = raw.lines();
    let branch = lines
        .next()
        .and_then(|l| l.strip_prefix("## "))
        .unwrap_or("?")
        .split("...")
        .next()
        .unwrap_or("?")
        .to_string();
    let files: Vec<String> = lines.map(|l| l.to_string()).collect();
    Ok(Json(StatusResp { branch, dirty: !files.is_empty(), files }))
}

#[derive(Deserialize)]
pub struct DiffQuery {
    pub r#ref: Option<String>,
}

pub async fn diff(
    State(st): State<Arc<AppState>>,
    Query(q): Query<DiffQuery>,
) -> Result<String, ApiError> {
    match q.r#ref {
        Some(r) => git(&st, &["diff", &r]).await,
        None => git(&st, &["diff", "HEAD"]).await,
    }
}

#[derive(Deserialize)]
pub struct CommitReq {
    pub message: String,
}

/// Commit = add -A + commit + immediate push of the current branch (§3.5
/// push-early: a local-only commit on an ephemeral container is lost work).
pub async fn commit(
    State(st): State<Arc<AppState>>,
    Json(r): Json<CommitReq>,
) -> Result<Json<serde_json::Value>, ApiError> {
    if r.message.trim().is_empty() {
        return Err(ApiError::bad_request("commit message must not be empty"));
    }
    git(&st, &["add", "-A"]).await?;
    let staged = git(&st, &["status", "--porcelain"]).await?;
    if staged.trim().is_empty() {
        let sha = git(&st, &["rev-parse", "HEAD"]).await?.trim().to_string();
        return Ok(Json(serde_json::json!({ "committed": false, "pushed": false, "sha": sha })));
    }
    git(&st, &["commit", "-m", &r.message]).await?;
    let sha = git(&st, &["rev-parse", "HEAD"]).await?.trim().to_string();
    git(&st, &["push", "origin", "HEAD"]).await?;
    Ok(Json(serde_json::json!({ "committed": true, "pushed": true, "sha": sha })))
}

#[derive(Deserialize)]
pub struct LogQuery {
    pub r#ref: Option<String>,
    pub limit: Option<u32>,
}

#[derive(Serialize)]
pub struct LogEntry {
    pub sha: String,
    pub author: String,
    pub date: String,
    pub message: String,
}

/// Log on any ref — first-class (§5: weak models must be able to do the
/// job-recap flow without exec).
pub async fn log(
    State(st): State<Arc<AppState>>,
    Query(q): Query<LogQuery>,
) -> Result<Json<Vec<LogEntry>>, ApiError> {
    let limit = q.limit.unwrap_or(20).min(200).to_string();
    let n = format!("-n{limit}");
    let fmt = format!("--format=%H{US}%an{US}%aI{US}%s{RS}");
    let mut args = vec!["log", &n, &fmt];
    let r; // keep borrow alive
    if let Some(ref_) = q.r#ref {
        r = ref_;
        args.push(&r);
    }
    let raw = git(&st, &args).await?;
    let entries = raw
        .split(RS)
        .filter(|rec| !rec.trim().is_empty())
        .filter_map(|rec| {
            let f: Vec<&str> = rec.trim().split(US).collect();
            (f.len() == 4).then(|| LogEntry {
                sha: f[0].into(),
                author: f[1].into(),
                date: f[2].into(),
                message: f[3].into(),
            })
        })
        .collect();
    Ok(Json(entries))
}

#[derive(Serialize)]
pub struct Branch {
    pub name: String,
    pub sha: String,
}

/// Branch list — local + remote, first-class (§5).
pub async fn branches(State(st): State<Arc<AppState>>) -> Result<Json<Vec<Branch>>, ApiError> {
    git(&st, &["fetch", "--prune", "origin"]).await?;
    let raw = git(
        &st,
        &["for-each-ref", "refs/heads", "refs/remotes/origin",
          "--format=%(refname:short) %(objectname)"],
    )
    .await?;
    let out = raw
        .lines()
        .filter_map(|l| {
            let (name, sha) = l.rsplit_once(' ')?;
            (name != "origin" && name != "origin/HEAD").then(|| Branch {
                name: name.to_string(),
                sha: sha.to_string(),
            })
        })
        .collect();
    Ok(Json(out))
}

#[derive(Deserialize)]
pub struct ShowQuery {
    pub r#ref: String,
    pub path: Option<String>,
}

/// Show a file at any ref, or the commit itself (stat) if no path (§5).
pub async fn show(
    State(st): State<Arc<AppState>>,
    Query(q): Query<ShowQuery>,
) -> Result<String, ApiError> {
    match q.path {
        Some(p) => git(&st, &["show", &format!("{}:{}", q.r#ref, p)]).await,
        None => git(&st, &["show", "--stat", "--format=medium", &q.r#ref]).await,
    }
}
