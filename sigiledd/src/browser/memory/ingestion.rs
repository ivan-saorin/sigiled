use super::*;
#[derive(Deserialize, Serialize)]
struct Source {
    source: String,
    #[serde(rename = "ref")]
    refid: String,
    path: Option<String>,
    last_run: String,
    chunks: u64,
    head: Option<String>,
}
impl Source {
    fn id(&self, n: &str) -> String {
        URL_SAFE_NO_PAD.encode(Sha256::digest(
            serde_json::to_vec(&(n, &self.source, &self.refid, &self.path)).unwrap(),
        ))
    }
}
#[derive(Deserialize, Serialize)]
struct Run {
    id: String,
    idx: String,
    source: String,
    #[serde(rename = "ref")]
    refid: String,
    path: Option<String>,
    status: String,
    started: String,
    finished: Option<String>,
    docs: u64,
    chunks: u64,
    deleted: u64,
    error: Option<String>,
    detail: Option<String>,
}
#[derive(Deserialize, Serialize)]
struct Ingests {
    idx: String,
    running: Option<Run>,
    runs: Vec<Run>,
    sources: Vec<Source>,
    last_ingest: Option<String>,
}
async fn recorded(svc: &Service, n: &str, token: &str) -> Result<Ingests, Error> {
    let (_, v) = svc
        .call(
            Method::GET,
            &format!("idx/{n}/ingests"),
            &[("limit", "20".into())],
            token,
            None,
        )
        .await?;
    let v: Ingests = decode(v)?;
    if v.idx != n || v.runs.len() > 51 || v.sources.len() > 10000 {
        return Err(unavailable());
    }
    Ok(v)
}
pub(super) async fn ingests(
    c: BrowserContext,
    State(s): State<AppState>,
    Path(n): Path<String>,
) -> Result<Json<Value>, Error> {
    index(&n)?;
    let v = recorded(&service(&s)?, &n, &c.access_token).await?;
    let enrollment = enrollment_for_index(&s, &n).map(|e| e.public());
    let sources:Vec<_>=v.sources.iter().map(|source|json!({"source_id":source.id(&n),"source":source.source,"ref":source.refid,"path":source.path,"last_run":source.last_run,"chunks":source.chunks,"head":source.head})).collect();
    public(
        json!({"accepted_enrollment":enrollment,"accepted_retry":"Use project Retry Memory setup; accepted documents are not legacy reindex sources.","idx":n,"running":v.running,"runs":v.runs,"sources":sources,"last_ingest":v.last_ingest,"history_limitation":"Recent process history; recorded completed sources survive restart."}),
        &c.access_token,
    )
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Reindex {
    source_id: String,
}
pub(super) async fn reindex(
    c: BrowserContext,
    State(s): State<AppState>,
    Path(n): Path<String>,
    Json(b): Json<Reindex>,
) -> Result<Response, Error> {
    index(&n)?;
    field(&b.source_id, 64, false)?;
    let svc = service(&s)?;
    svc.require(&c.access_token).await?;
    let data = recorded(&svc, &n, &c.access_token).await?;
    let source = data
        .sources
        .iter()
        .find(|r| r.id(&n) == b.source_id)
        .ok_or(Error(
            StatusCode::CONFLICT,
            "memory_recorded_source_changed",
        ))?;
    if source.source == "git" && accepted_repository(&s, &n, &source.refid) {
        return Err(Error(
            StatusCode::CONFLICT,
            "accepted_repository_requires_memory_setup_retry",
        ));
    }
    if !matches!(source.source.as_str(), "git" | "file" | "sqlite") {
        return Err(Error(
            StatusCode::BAD_REQUEST,
            "memory_source_reindex_unsupported",
        ));
    }
    let (status, v) = svc
        .call(
            Method::POST,
            &format!("idx/{n}/ingest"),
            &[],
            &c.access_token,
            Some(json!({"source":source.source,"ref":source.refid,"path":source.path,"tags":[]})),
        )
        .await?;
    let v: Run = decode(v)?;
    Ok((status, public(v, &c.access_token)?).into_response())
}

fn enrollment_for_index(s: &AppState, n: &str) -> Option<crate::enrollment::Enrollment> {
    s.registry
        .descriptors()
        .into_iter()
        .find_map(|(project, d)| {
            let e = d.memory_enrollment?;
            (e.namespace.as_deref().unwrap_or(&project) == n).then_some(e)
        })
}
fn accepted_repository(s: &AppState, n: &str, repository: &str) -> bool {
    s.registry.descriptors().iter().any(|(project, d)| {
        d.memory_enrollment.as_ref().is_some_and(|e| {
            e.snapshot.as_ref().is_some_and(|snapshot| {
                snapshot.repository == repository
                    && (e.namespace.as_deref().unwrap_or(project) == n
                        || (n == "mem0" && snapshot.shared))
            })
        })
    })
}
#[cfg(test)]
mod enrollment_index_tests {
    use super::*;
    #[test]
    fn enrollment_corrected_namespace_and_shared_guard_do_not_claim_unrelated_indexes() {
        let state = AppState::test_without_runtime();
        state.registry.insert(crate::project::ProjectRecord::new(
            "atlas",
            &crate::manifest::Manifest::parse("").unwrap(),
            None,
        ));
        let mut d = state.registry.descriptor("atlas");
        let e = d.memory_enrollment.as_mut().unwrap();
        e.namespace = Some("atlas-separate".into());
        e.snapshot = Some(crate::enrollment_contract::Snapshot {
            project: "atlas".into(),
            repository: "tests/atlas".into(),
            owner: e.owner.clone(),
            revision: "1".into(),
            commit: "a".repeat(40),
            documents: vec![],
            shared: true,
        });
        state
            .registry
            .descriptors
            .write()
            .unwrap()
            .insert("atlas".into(), d);
        assert!(enrollment_for_index(&state, "atlas-separate").is_some());
        assert!(enrollment_for_index(&state, "atlas").is_none());
        assert!(enrollment_for_index(&state, "unrelated").is_none());
        assert!(accepted_repository(&state, "atlas-separate", "tests/atlas"));
        assert!(accepted_repository(&state, "mem0", "tests/atlas"));
        assert!(!accepted_repository(&state, "atlas", "tests/atlas"));
        assert!(!accepted_repository(&state, "unrelated", "tests/atlas"));
    }
}
