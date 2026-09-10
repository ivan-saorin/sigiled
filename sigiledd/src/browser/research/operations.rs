use super::*;
use std::sync::atomic::{AtomicBool, Ordering};
#[derive(Clone, Default)]
pub(super) struct Store {
    dir: Option<PathBuf>,
    lock: Arc<Mutex<()>>,
    frozen: Arc<AtomicBool>,
}
#[derive(Clone, Deserialize, Serialize)]
struct Intent {
    id: String,
    actor: String,
    project: String,
    payload: Value,
    key: String,
    #[serde(default)]
    run_id: Option<String>,
    #[serde(default)]
    handoff: Option<Value>,
}
impl Store {
    pub fn ensure_writable(&self) -> Result<(), Error> {
        if self.frozen.load(Ordering::SeqCst) {
            return Err(Error(
                StatusCode::SERVICE_UNAVAILABLE,
                "research_operation_store_repair_required",
            ));
        }
        self.transaction(false, |_| Ok(()))
    }

    pub fn new(dir: Option<PathBuf>) -> Self {
        Self {
            dir,
            ..Self::default()
        }
    }
    fn transaction<T>(
        &self,
        write: bool,
        f: impl FnOnce(&mut BTreeMap<String, Intent>) -> Result<T, Error>,
    ) -> Result<T, Error> {
        let _guard = self.lock.lock().unwrap();
        let fail = || {
            Error(
                StatusCode::SERVICE_UNAVAILABLE,
                "research_operation_store_repair_required",
            )
        };
        let dir = self.dir.as_ref().ok_or(Error(
            StatusCode::SERVICE_UNAVAILABLE,
            "research_operation_storage_required",
        ))?;
        if write && self.frozen.load(Ordering::SeqCst) {
            return Err(fail());
        }
        let path = dir.join("research-operations.json");
        let mut data = match std::fs::read(&path) {
            Ok(bytes) if bytes.len() <= 32 * 1024 * 1024 => {
                serde_json::from_slice(&bytes).map_err(|_| fail())?
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => BTreeMap::new(),
            _ => return Err(fail()),
        };
        let result = f(&mut data)?;
        if write {
            let saved = (|| -> std::io::Result<()> {
                use std::io::Write;
                durable_dir(dir)?;
                let bytes = serde_json::to_vec(&data)?;
                if bytes.len() > 32 * 1024 * 1024 {
                    return Err(std::io::Error::other("capacity"));
                }
                let temp = dir.join(format!("research-operations.{}.tmp", uuid()?));
                let mut opts = std::fs::OpenOptions::new();
                opts.create_new(true).write(true);
                #[cfg(unix)]
                {
                    use std::os::unix::fs::OpenOptionsExt;
                    opts.mode(0o600);
                }
                let mut file = opts.open(&temp)?;
                file.write_all(&bytes)?;
                file.sync_all()?;
                std::fs::rename(&temp, &path)?;
                std::fs::File::open(dir)?.sync_all()
            })();
            if saved.is_err() {
                self.frozen.store(true, Ordering::SeqCst);
                return Err(fail());
            }
        }
        Ok(result)
    }
    fn digest(actor: &str, project: &str, id: &str) -> String {
        format!(
            "{:x}",
            Sha256::digest(serde_json::to_vec(&(actor, project, id)).unwrap())
        )
    }
    fn reserve(
        &self,
        actor: &str,
        project: &str,
        id: &str,
        payload: Value,
    ) -> Result<Intent, Error> {
        self.transaction(true, |d| {
            let k = Self::digest(actor, project, id);
            if let Some(old) = d.get(&k) {
                if old.payload != payload {
                    return Err(Error(StatusCode::CONFLICT, "research_operation_conflict"));
                }
                return Ok(old.clone());
            }
            if d.len() >= 10000 {
                return Err(Error(StatusCode::CONFLICT, "research_operation_capacity"));
            }
            let i = Intent {
                id: id.into(),
                actor: actor.into(),
                project: project.into(),
                payload,
                key: uuid().map_err(|_| unavailable())?,
                run_id: None,
                handoff: None,
            };
            d.insert(k, i.clone());
            Ok(i)
        })
    }
    fn get(&self, actor: &str, project: &str, id: &str) -> Result<Intent, Error> {
        self.transaction(false, |d| {
            d.get(&Self::digest(actor, project, id))
                .cloned()
                .ok_or(Error(StatusCode::NOT_FOUND, "research_operation_not_found"))
        })
    }
    fn accept(&self, i: &Intent, run: &str) -> Result<(), Error> {
        self.transaction(true, |d| {
            let old = d
                .get_mut(&Self::digest(&i.actor, &i.project, &i.id))
                .ok_or_else(unavailable)?;
            if old.run_id.as_ref().is_some_and(|id| id != run) {
                return Err(Error(StatusCode::CONFLICT, "research_operation_conflict"));
            }
            old.run_id = Some(run.into());
            Ok(())
        })
    }
    pub fn handoff(&self, actor: &str, run: &str, receipt: Value) -> Result<(), Error> {
        self.transaction(true, |d| {
            let i = d
                .values_mut()
                .find(|i| i.actor == actor && i.run_id.as_deref() == Some(run))
                .ok_or_else(unavailable)?;
            i.handoff = Some(receipt);
            Ok(())
        })
    }
    pub fn association(&self, actor: &str, run: &str) -> Result<Option<String>, Error> {
        self.transaction(false, |d| {
            Ok(d.values()
                .find(|i| i.actor == actor && i.run_id.as_deref() == Some(run))
                .map(|i| i.project.clone()))
        })
    }
    fn list(&self, actor: &str, project: &str) -> Result<Vec<Value>, Error> {
        self.transaction(false, |d| {
            Ok(d.values()
                .filter(|i| i.actor == actor && i.project == project)
                .map(view)
                .collect())
        })
    }
}
fn uuid() -> std::io::Result<String> {
    let mut b = [0u8; 16];
    getrandom::getrandom(&mut b).map_err(|_| std::io::Error::other("entropy unavailable"))?;
    b[6] = (b[6] & 15) | 64;
    b[8] = (b[8] & 63) | 128;
    Ok(format!("{:02x}{:02x}{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}{:02x}{:02x}{:02x}{:02x}",b[0],b[1],b[2],b[3],b[4],b[5],b[6],b[7],b[8],b[9],b[10],b[11],b[12],b[13],b[14],b[15]))
}
fn view(i: &Intent) -> Value {
    json!({"operation_id":i.id,"project":i.project,"run_id":i.run_id,"state":if i.run_id.is_some(){"accepted"}else{"acceptance_unknown"},"problem":i.payload["problem"],"handoff":i.handoff})
}
#[derive(Clone, Deserialize, Serialize)]
#[serde(default, deny_unknown_fields)]
pub(super) struct Options {
    papers_per_category: u32,
    since_years: u32,
    breadth: bool,
    breadth_results: u32,
    adhd: bool,
    aperture: u32,
}
impl Default for Options {
    fn default() -> Self {
        Self {
            papers_per_category: 4,
            since_years: 8,
            breadth: true,
            breadth_results: 8,
            adhd: true,
            aperture: 1,
        }
    }
}
// Complete compact accepted shape: two UTF-8 fields (32,768 bytes each),
// the 128-byte identifier, all ten field names, scalar maxima and punctuation.
// JSON can encode each decoded byte as six ASCII bytes (\u00XX), including keys.
// Whitespace and alternate numeric encodings must also fit this finite wire cap.
pub(in crate::browser) const CREATE_BODY_LIMIT: usize =
    r#"{"operation_id":"","problem":"","context":"","options":{"papers_per_category":10,"since_years":50,"breadth":false,"breadth_results":20,"adhd":false,"aperture":3}}"#.len()
    + 6 * (2 * 32768 + 128)
    + 5 * ("operation_idproblemcontextoptionspapers_per_categorysince_yearsbreadthbreadth_resultsadhdaperture".len());
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(in crate::browser) struct Create {
    operation_id: String,
    problem: String,
    #[serde(default)]
    context: String,
    #[serde(default)]
    options: Options,
}
fn payload(body: &Create, project: &str) -> Result<Value, Error> {
    if !identifier(&body.operation_id)
        || body.operation_id.len() < 8
        || body.problem.trim().is_empty()
        || body.problem.len() > 32768
        || body.context.len() > 32768
        || !(1..=10).contains(&body.options.papers_per_category)
        || body.options.since_years > 50
        || !(1..=20).contains(&body.options.breadth_results)
        || !(1..=3).contains(&body.options.aperture)
    {
        return Err(Error(StatusCode::BAD_REQUEST, "invalid_research_options"));
    }
    let mut options = serde_json::to_value(&body.options).unwrap();
    options["delegate"] = json!([]);
    options["shape_hint"] = json!(true);
    Ok(
        json!({"problem":body.problem.trim(),"context":if body.context.trim().is_empty(){None}else{Some(body.context.trim())},"project_id":project,"originator":{"reference":"browser-operation"},"options":options}),
    )
}
fn project(state: &AppState, c: &BrowserContext, p: &str) -> Result<(), Error> {
    if !c.actor.driver.starts_with("human:") {
        return Err(Error::forbidden("verified_human_required"));
    }
    if !crate::project::valid_name(p) || !state.registry.contains(p) {
        return Err(Error(StatusCode::NOT_FOUND, "unknown_project"));
    }
    Ok(())
}
async fn submit(s: &Services, c: &BrowserContext, i: &Intent) -> Result<Json<Value>, Error> {
    s.store.ensure_writable()?;
    let mut payload = i.payload.clone();
    payload["operation_key"] = json!(i.key);
    // An explicit recovery reuses the private key and canonical request with the CURRENT caller token.
    match s
        .call(
            Engine::Sde,
            Method::POST,
            "runs",
            &[],
            &c.access_token,
            Some(&payload),
        )
        .await
    {
        Ok(r) => {
            let id = r["run_id"]
                .as_str()
                .filter(|s| identifier(s))
                .ok_or_else(unavailable)?;
            s.store.accept(i, id)?;
            let mut v = view(i);
            v["run_id"] = json!(id);
            v["state"] = json!("accepted");
            Ok(Json(v))
        }
        Err(e) => Err(e),
    }
}
pub(in crate::browser) async fn create(
    c: BrowserContext,
    State(state): State<AppState>,
    Path(p): Path<String>,
    Json(body): Json<Create>,
) -> Result<Json<Value>, Error> {
    project(&state, &c, &p)?;
    let payload = payload(&body, &p)?;
    let s = state.browser.inner()?.research.clone();
    s.readiness(&c.access_token).await?;
    let i = s
        .store
        .reserve(&c.actor.driver, &p, &body.operation_id, payload)?;
    submit(&s, &c, &i).await
}
pub(in crate::browser) async fn recover(
    c: BrowserContext,
    State(state): State<AppState>,
    Path((p, id)): Path<(String, String)>,
) -> Result<Json<Value>, Error> {
    project(&state, &c, &p)?;
    checked_id(&id)?;
    let s = state.browser.inner()?.research.clone();
    let i = s.store.get(&c.actor.driver, &p, &id)?;
    // Recovery is deliberate and still requires a safe live persistence contract, never a catalog probe.
    s.readiness(&c.access_token).await?;
    submit(&s, &c, &i).await
}
pub(in crate::browser) async fn operations(
    c: BrowserContext,
    State(state): State<AppState>,
    Path(p): Path<String>,
) -> Result<Json<Value>, Error> {
    checked_id(&p)?;
    let s = state.browser.inner()?.research.clone();
    let mut rows = s.store.list(&c.actor.driver, &p)?;
    for row in &mut rows {
        for record in state.sessions.dump_records().values() {
            if record.actor.driver == c.actor.driver && record.project == p {
                if let Some(marker) = &record.handoff {
                    if marker["run_id"] == row["run_id"] {
                        row["handoff"] = json!({"phase":marker["phase"],"session_id":record.session_id,"generation":record.generation.to_string(),"receipt":marker["receipt"]});
                    }
                }
            }
        }
    }
    for row in &mut rows {
        if let Some(run) = row["run_id"].as_str() {
            if let Some(receipt) =
                crate::enrollment::research_receipt(&state, &p, run, &c.actor.driver)
            {
                row["handoff"] = receipt;
            }
        }
    }
    Ok(Json(json!({"operations":rows})))
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(in crate::browser) struct Mutation {
    action: String,
    expected_revision: String,
    #[serde(default)]
    output: Option<Value>,
    #[serde(default)]
    model: Option<String>,
}
pub(in crate::browser) async fn mutate(
    c: BrowserContext,
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(body): Json<Mutation>,
) -> Result<Json<Value>, Error> {
    checked_id(&id)?;
    let revision = revision(&body.expected_revision)?;
    let s = state.browser.inner()?.research.clone();
    s.store.ensure_writable()?;
    let project_id = s
        .store
        .association(&c.actor.driver, &id)?
        .ok_or(Error::forbidden("research_project_association_required"))?;
    project(&state, &c, &project_id)?;
    s.readiness(&c.access_token).await?;
    let run = s
        .call(
            Engine::Sde,
            Method::GET,
            &format!("runs/{id}"),
            &[],
            &c.access_token,
            None,
        )
        .await?;
    if run["revision"].as_u64() != Some(revision) {
        return Err(Error(StatusCode::CONFLICT, "research_revision_conflict"));
    }
    let (path, payload) = if body.action == "resume" {
        if run["status"] != "failed" || body.output.is_some() {
            return Err(Error(StatusCode::CONFLICT, "research_not_resumable"));
        }
        (format!("runs/{id}/resume"), json!({}))
    } else if matches!(body.action.as_str(), "categorize" | "pick" | "converge") {
        let awaiting =
            run["status"] == "awaiting_caller" && run["awaiting"]["stage"] == body.action;
        let converge = body.action == "converge"
            && run["status"] == "done"
            && run["artifacts"]["outcome"].is_null();
        if (!awaiting && !converge)
            || body.output.is_none()
            || body.model.as_ref().is_some_and(|m| m.len() > 128)
        {
            return Err(Error(StatusCode::CONFLICT, "research_not_awaiting"));
        }
        (
            format!("runs/{id}/stages/{}", body.action),
            json!({"output":body.output,"model":body.model,"expected_revision":revision}),
        )
    } else {
        return Err(Error(
            StatusCode::BAD_REQUEST,
            "unsupported_research_action",
        ));
    };
    let mut response = s
        .call(
            Engine::Sde,
            Method::POST,
            &path,
            &[],
            &c.access_token,
            Some(&payload),
        )
        .await?;
    scrub(&mut response, &c.access_token);
    project_revision(&mut response);
    Ok(Json(response))
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn durable_intent_survives_login_and_refuses_actor_or_payload_takeover() {
        let dir = std::env::temp_dir().join(uuid().unwrap());
        let s = Store::new(Some(dir.clone()));
        let p = json!({"problem":"x"});
        let i = s
            .reserve("human:first", "demo", "operation1", p.clone())
            .unwrap();
        assert_eq!(i.key.len(), 36);
        assert_eq!(&i.key[14..15], "4");
        assert!(!serde_json::to_string(&view(&i)).unwrap().contains(&i.key));
        assert!(s
            .reserve(
                "human:first",
                "demo",
                "operation1",
                json!({"problem":"changed"})
            )
            .is_err());
        let recovered = Store::new(Some(dir.clone()))
            .get("human:first", "demo", "operation1")
            .unwrap();
        assert_eq!(i.key, recovered.key);
        assert!(s.get("human:other", "demo", "operation1").is_err());
        s.accept(&i, "run1").unwrap();
        assert_eq!(
            s.association("human:first", "run1").unwrap().as_deref(),
            Some("demo")
        );
        assert!(s.association("human:other", "run1").unwrap().is_none());
        std::fs::remove_dir_all(dir).unwrap();
    }
    #[test]
    fn browser_options_never_delegate_and_are_bounded() {
        let b = Create {
            operation_id: "operation1".into(),
            problem: " problem ".into(),
            context: " ".into(),
            options: Options::default(),
        };
        let p = payload(&b, "demo").unwrap();
        assert_eq!(p["options"]["delegate"], json!([]));
        assert_eq!(p["problem"], "problem");
    }
}

#[cfg(test)]
mod acceptance_tests {
    use super::*;
    use std::future::IntoFuture;
    use std::sync::atomic::{AtomicUsize, Ordering};
    #[tokio::test]
    async fn ambiguous_acceptance_reuses_private_key_with_fresh_caller_token() {
        let accepted = Arc::new(Mutex::new(BTreeMap::<String, String>::new()));
        let count = Arc::new(AtomicUsize::new(0));
        let headers = Arc::new(Mutex::new(Vec::new()));
        let map = accepted.clone();
        let calls = count.clone();
        let seen = headers.clone();
        let app = Router::new().route(
            "/runs",
            post(move |h: HeaderMap, Json(body): Json<Value>| {
                let map = map.clone();
                let calls = calls.clone();
                let seen = seen.clone();
                async move {
                    assert_eq!(body["options"]["delegate"], json!([]));
                    seen.lock()
                        .unwrap()
                        .push(h["authorization"].to_str().unwrap().to_string());
                    let key = body["operation_key"].as_str().unwrap().to_owned();
                    let id = map
                        .lock()
                        .unwrap()
                        .entry(key)
                        .or_insert("run1".into())
                        .clone();
                    if calls.fetch_add(1, Ordering::SeqCst) == 0 {
                        (
                            StatusCode::SERVICE_UNAVAILABLE,
                            Json(json!({"private":"upstream log"})),
                        )
                    } else {
                        (
                            StatusCode::ACCEPTED,
                            Json(json!({"run_id":id,"replayed":true})),
                        )
                    }
                }
            }),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let server = tokio::spawn(axum::serve(listener, app).into_future());
        let mut s = Services::default();
        s.bases.insert("sde".into(), base);
        let dir = std::env::temp_dir().join(uuid().unwrap());
        s.store = Store::new(Some(dir.clone()));
        let mut c = BrowserContext {
            actor: Actor {
                driver: "human:verified".into(),
                role: auth::Role::Admin,
                approval: None,
            },
            issuer: "issuer".into(),
            subject: "subject".into(),
            display_name: None,
            access_token: "first-token".into(),
            session_id: "temporary-login-one".into(),
            csrf: "csrf".into(),
            absolute: u64::MAX,
            idle_expires: u64::MAX,
        };
        let p = payload(
            &Create {
                operation_id: "operation1".into(),
                problem: "problem".into(),
                context: String::new(),
                options: Options::default(),
            },
            "demo",
        )
        .unwrap();
        let intent = s
            .store
            .reserve(&c.actor.driver, "demo", "operation1", p)
            .unwrap();
        assert!(submit(&s, &c, &intent).await.is_err());
        assert_eq!(accepted.lock().unwrap().len(), 1);
        c.access_token = "fresh-token".into();
        c.session_id = "temporary-login-two".into();
        let recovered = Store::new(Some(dir.clone()))
            .get(&c.actor.driver, "demo", "operation1")
            .unwrap();
        let Json(r) = submit(&s, &c, &recovered).await.unwrap();
        assert_eq!(r["run_id"], "run1");
        assert_eq!(accepted.lock().unwrap().len(), 1);
        assert_eq!(
            *headers.lock().unwrap(),
            vec!["Bearer first-token", "Bearer fresh-token"]
        );
        assert!(!r.to_string().contains(&intent.key));
        server.abort();
        std::fs::remove_dir_all(dir).unwrap();
    }
}

fn durable_dir(dir: &std::path::Path) -> std::io::Result<()> {
    if dir.is_dir() {
        return Ok(());
    }
    let parent = dir
        .parent()
        .ok_or_else(|| std::io::Error::other("invalid directory"))?;
    durable_dir(parent)?;
    std::fs::create_dir(dir)?;
    std::fs::File::open(parent)?.sync_all()?;
    std::fs::File::open(dir)?.sync_all()
}
