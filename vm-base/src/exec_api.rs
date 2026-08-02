use std::process::Stdio;
use std::sync::Arc;
use std::time::Duration;

use axum::extract::State;
use axum::Json;
use serde::{Deserialize, Serialize};
use tokio::io::AsyncReadExt;
use tokio::process::Command;

use crate::error::ApiError;
use crate::state::AppState;

const MAX_CAPTURE: usize = 1 << 20; // 1 MiB per stream, then truncated
const DEFAULT_TIMEOUT: u64 = 300;
const MAX_TIMEOUT: u64 = 3600;

#[derive(Deserialize)]
pub struct ExecReq {
    pub cmd: String,
    pub cwd: Option<String>,
    pub timeout_secs: Option<u64>,
}

#[derive(Serialize)]
pub struct ExecResp {
    pub exit: i32,
    pub stdout: String,
    pub stderr: String,
    pub timed_out: bool,
    pub truncated: bool,
}

async fn drain(mut r: impl tokio::io::AsyncRead + Unpin) -> (Vec<u8>, bool) {
    let mut buf = Vec::new();
    let mut chunk = [0u8; 8192];
    let mut truncated = false;
    loop {
        match r.read(&mut chunk).await {
            Ok(0) | Err(_) => break,
            Ok(n) => {
                if buf.len() < MAX_CAPTURE {
                    let take = n.min(MAX_CAPTURE - buf.len());
                    buf.extend_from_slice(&chunk[..take]);
                    if take < n {
                        truncated = true;
                    }
                } else {
                    truncated = true; // keep draining so the child never blocks on a full pipe
                }
            }
        }
    }
    (buf, truncated)
}

/// Arbitrary command, full power, container-sized blast radius (§5: "option A
/// sealed in a disposable box"). Reaches exactly the container fs + declared
/// mounts — the seal is structural (§3.11), not policy.
pub async fn exec(
    State(st): State<Arc<AppState>>,
    Json(r): Json<ExecReq>,
) -> Result<Json<ExecResp>, ApiError> {
    let timeout = Duration::from_secs(r.timeout_secs.unwrap_or(DEFAULT_TIMEOUT).min(MAX_TIMEOUT));
    let cwd = r.cwd.unwrap_or_else(|| st.workspace.display().to_string());

    let mut child = Command::new("bash")
        .arg("-lc")
        .arg(&r.cmd)
        .current_dir(&cwd)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .map_err(|e| ApiError::bad_request(format!("spawn failed (cwd '{cwd}'): {e}")))?;

    let stdout = child.stdout.take().expect("piped");
    let stderr = child.stderr.take().expect("piped");
    let out_task = tokio::spawn(drain(stdout));
    let err_task = tokio::spawn(drain(stderr));

    let (exit, timed_out) = match tokio::time::timeout(timeout, child.wait()).await {
        Ok(status) => (
            status.map_err(|e| ApiError::internal(e.to_string()))?.code().unwrap_or(-1),
            false,
        ),
        Err(_) => {
            let _ = child.kill().await;
            (-1, true)
        }
    };

    let (out, out_trunc) = out_task.await.unwrap_or_default();
    let (err, err_trunc) = err_task.await.unwrap_or_default();

    Ok(Json(ExecResp {
        exit,
        stdout: String::from_utf8_lossy(&out).into_owned(),
        stderr: String::from_utf8_lossy(&err).into_owned(),
        timed_out,
        truncated: out_trunc || err_trunc,
    }))
}
