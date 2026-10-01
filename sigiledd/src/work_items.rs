//! Small durable work-item transactions. No cache can get ahead of the disk.
use axum::{
    http::StatusCode,
    response::{IntoResponse, Response},
    Json,
};
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
};

#[derive(Clone, Default)]
pub struct Store {
    path: Option<PathBuf>,
    lock: Arc<Mutex<()>>,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum State {
    Open,
    Blocked,
    Done,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Fields {
    pub title: String,
    #[serde(default)]
    pub description: String,
    pub state: State,
    #[serde(default)]
    pub owner: String,
    #[serde(default)]
    pub source_link: String,
}
#[derive(Clone, Serialize, Deserialize)]
pub struct Edit {
    pub revision: u64,
    pub at: u64,
    pub actor: String,
    pub fields: Fields,
}
#[derive(Clone, Serialize, Deserialize)]
pub struct Item {
    pub id: String,
    pub project: String,
    #[serde(flatten)]
    pub fields: Fields,
    pub created_at: u64,
    pub updated_at: u64,
    pub revision: u64,
    pub creator: String,
    pub editor: String,
    pub audit: Vec<Edit>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub request: Option<crate::requests::Question>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Create {
    pub id: String,
    pub fields: Fields,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Update {
    pub expected_revision: u64,
    pub fields: Fields,
}
#[derive(Debug)]
pub struct Error(pub StatusCode, pub &'static str);
impl IntoResponse for Error {
    fn into_response(self) -> Response {
        (self.0, Json(json!({"error":self.1}))).into_response()
    }
}
type Data = BTreeMap<String, Item>;
impl Store {
    pub fn from_env() -> Self {
        std::env::var("SIGILED_STATE_DIR")
            .ok()
            .map(|d| Self::at_dir(Path::new(&d)))
            .unwrap_or_default()
    }
    pub fn at_dir(dir: &Path) -> Self {
        Self {
            path: Some(dir.join("work-items.json")),
            lock: Arc::default(),
        }
    }
    pub fn available(&self) -> bool {
        self.path.is_some()
    }
    fn transaction<T>(
        &self,
        write: bool,
        f: impl FnOnce(&mut Data) -> Result<T, Error>,
    ) -> Result<T, Error> {
        let _lock = self.lock.lock().unwrap();
        let path = self.path.as_ref().ok_or(Error(
            StatusCode::SERVICE_UNAVAILABLE,
            "work_item_storage_not_configured",
        ))?;
        let mut data = match std::fs::read(path) {
            Ok(bytes) if bytes.len() <= 64 * 1024 * 1024 => serde_json::from_slice::<Data>(&bytes)
                .map_err(|_| Error(StatusCode::SERVICE_UNAVAILABLE, "work_item_store_invalid"))?,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Data::new(),
            _ => {
                return Err(Error(
                    StatusCode::SERVICE_UNAVAILABLE,
                    "work_item_store_unavailable",
                ))
            }
        };
        let result = f(&mut data)?;
        if write {
            self.save(path, &data)?;
        }
        Ok(result)
    }
    fn save(&self, path: &Path, data: &Data) -> Result<(), Error> {
        use std::io::Write;
        let fail = || Error(StatusCode::SERVICE_UNAVAILABLE, "work_item_save_uncertain");
        let dir = path.parent().ok_or_else(fail)?;
        std::fs::create_dir_all(dir).map_err(|_| fail())?;
        let bytes = serde_json::to_vec(data).map_err(|_| fail())?;
        if bytes.len() > 64 * 1024 * 1024 {
            return Err(Error(StatusCode::CONFLICT, "work_item_capacity"));
        }
        let tmp = path.with_extension("json.tmp");
        let mut options = std::fs::OpenOptions::new();
        options.write(true).create(true).truncate(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let mut file = options.open(&tmp).map_err(|_| fail())?;
        file.write_all(&bytes)
            .and_then(|_| file.sync_all())
            .map_err(|_| fail())?;
        std::fs::rename(&tmp, path).map_err(|_| fail())?;
        std::fs::File::open(dir)
            .and_then(|f| f.sync_all())
            .map_err(|_| fail())?;
        Ok(())
    }
    pub fn list(
        &self,
        project: &str,
        offset: usize,
        limit: usize,
    ) -> Result<serde_json::Value, Error> {
        self.transaction(false,|data| {
            let mut rows:Vec<_>=data.values().filter(|i|i.project==project && i.request.is_none()).collect();
            rows.sort_by(|a,b|b.updated_at.cmp(&a.updated_at).then(a.id.cmp(&b.id)));
            let total=rows.len();
            // Audit is a detail-only projection, bounded independently of list pages.
            let items:Vec<_>=rows.into_iter().skip(offset).take(limit).map(|i| {let mut v=serde_json::to_value(i).unwrap();v.as_object_mut().unwrap().remove("audit");v}).collect();
            Ok(json!({"items":items,"total":total,"offset":offset,"limit":limit,"next_offset":if offset+limit<total {Some(offset+limit)}else{None}}))
        })
    }
    pub fn get(&self, project: &str, id: &str) -> Result<Item, Error> {
        self.transaction(false, |d| Self::find(d, project, id).cloned())
    }
    fn find<'a>(data: &'a Data, project: &str, id: &str) -> Result<&'a Item, Error> {
        data.get(id)
            .filter(|i| i.project == project)
            .ok_or(Error(StatusCode::NOT_FOUND, "work_item_not_found"))
    }
    pub fn create(&self, project: &str, actor: &str, body: Create) -> Result<Item, Error> {
        validate(&body.fields)?;
        if body.id.len() != 36
            || !body.id.bytes().enumerate().all(|(i, c)| {
                if [8, 13, 18, 23].contains(&i) {
                    c == b'-'
                } else {
                    c.is_ascii_hexdigit()
                }
            })
        {
            return Err(Error(
                StatusCode::UNPROCESSABLE_ENTITY,
                "invalid_work_item_id",
            ));
        }
        self.transaction(true, |data| {
            if let Some(old) = data.get(&body.id) {
                if old.project == project
                    && old.creator == actor
                    && old.audit[0].fields == body.fields
                {
                    return Ok(old.clone());
                }
                return Err(Error(StatusCode::CONFLICT, "work_item_id_conflict"));
            }
            if data.values().filter(|i| i.project == project).count() >= 10000 {
                return Err(Error(StatusCode::CONFLICT, "work_item_capacity"));
            }
            let now = crate::auth::now_epoch();
            let item = Item {
                id: body.id,
                project: project.into(),
                fields: body.fields.clone(),
                created_at: now,
                updated_at: now,
                revision: 1,
                creator: actor.into(),
                editor: actor.into(),
                audit: vec![Edit {
                    revision: 1,
                    at: now,
                    actor: actor.into(),
                    fields: body.fields,
                }],
                request: None,
            };
            data.insert(item.id.clone(), item.clone());
            Ok(item)
        })
    }
    pub fn update(
        &self,
        project: &str,
        id: &str,
        actor: &str,
        body: Update,
    ) -> Result<Item, Error> {
        validate(&body.fields)?;
        self.transaction(true, |data| {
            let mut item = Self::find(data, project, id)?.clone();
            if item.request.is_some() {
                return Err(Error(StatusCode::CONFLICT, "request_requires_answer_route"));
            }
            if item.revision != body.expected_revision {
                return Err(Error(StatusCode::CONFLICT, "work_item_revision_conflict"));
            }
            if item.audit.len() >= 1000 {
                return Err(Error(StatusCode::CONFLICT, "work_item_audit_capacity"));
            }
            item.revision += 1;
            item.updated_at = crate::auth::now_epoch();
            item.editor = actor.into();
            item.fields = body.fields;
            item.audit.push(Edit {
                revision: item.revision,
                at: item.updated_at,
                actor: actor.into(),
                fields: item.fields.clone(),
            });
            data.insert(id.into(), item.clone());
            Ok(item)
        })
    }
}
impl Store {
    fn request_store(&self) -> Self {
        Self {
            path: self
                .path
                .as_ref()
                .map(|p| p.with_file_name("agent-requests.json")),
            lock: self.lock.clone(),
        }
    }
    pub fn get_request(&self, project: &str, id: &str) -> Result<Item, Error> {
        self.request_store().get(project, id)
    }
    pub fn list_requests(
        &self,
        project: &str,
        offset: usize,
        limit: usize,
    ) -> Result<serde_json::Value, Error> {
        self.request_store().transaction(false, |data| {
            let mut rows: Vec<_> = data.values().filter(|i| i.project == project && i.request.is_some()).collect();
            rows.sort_by(|a,b| a.request.as_ref().unwrap().answer.is_some().cmp(&b.request.as_ref().unwrap().answer.is_some()).then(b.updated_at.cmp(&a.updated_at)).then(a.id.cmp(&b.id)));
            let total = rows.len();
            let waiting = rows.iter().filter(|i| i.request.as_ref().unwrap().answer.is_none()).count();
            let items: Vec<_> = rows.into_iter().skip(offset).take(limit).map(|i| {
                let mut v = serde_json::to_value(i).unwrap();
                v.as_object_mut().unwrap().remove("audit");
                v
            }).collect();
            Ok(json!({"items":items,"total":total,"waiting":waiting,"offset":offset,"limit":limit,"next_offset":if offset+limit<total {Some(offset+limit)}else{None}}))
        })
    }

    pub fn create_request(
        &self,
        project: &str,
        actor: &str,
        body: crate::requests::Create,
    ) -> Result<Item, Error> {
        let fields = Fields {
            title: body.title,
            description: body.context.clone(),
            state: State::Blocked,
            owner: actor.into(),
            source_link: body.source_link,
        };
        validate(&fields)?;
        if body.question.trim().is_empty()
            || body.question.len() > 8000
            || body.id.len() != 36
            || !body.id.bytes().enumerate().all(|(i, c)| {
                if [8, 13, 18, 23].contains(&i) {
                    c == b'-'
                } else {
                    c.is_ascii_hexdigit()
                }
            })
        {
            return Err(Error(StatusCode::UNPROCESSABLE_ENTITY, "invalid_request"));
        }
        self.request_store().transaction(true, |data| {
            if let Some(old) = data.get(&body.id) {
                if old.project == project
                    && old.creator == actor
                    && old.fields.title == fields.title
                    && old.fields.source_link == fields.source_link
                    && old
                        .request
                        .as_ref()
                        .is_some_and(|q| q.question == body.question && q.context == body.context)
                {
                    return Ok(old.clone());
                }
                return Err(Error(StatusCode::CONFLICT, "request_id_conflict"));
            }
            if data.values().filter(|i| i.project == project).count() >= 10000 {
                return Err(Error(StatusCode::CONFLICT, "work_item_capacity"));
            }
            let now = crate::auth::now_epoch();
            let item = Item {
                id: body.id,
                project: project.into(),
                fields: fields.clone(),
                created_at: now,
                updated_at: now,
                revision: 1,
                creator: actor.into(),
                editor: actor.into(),
                audit: vec![Edit {
                    revision: 1,
                    at: now,
                    actor: actor.into(),
                    fields,
                }],
                request: Some(crate::requests::Question {
                    question: body.question,
                    context: body.context,
                    answer: None,
                }),
            };
            data.insert(item.id.clone(), item.clone());
            Ok(item)
        })
    }

    pub fn answer_request(
        &self,
        project: &str,
        id: &str,
        actor: &str,
        body: crate::requests::Answer,
    ) -> Result<Item, Error> {
        if body.text.trim().is_empty() || body.text.len() > 8000 {
            return Err(Error(
                StatusCode::UNPROCESSABLE_ENTITY,
                "invalid_request_answer",
            ));
        }
        self.request_store().transaction(true, |data| {
            let mut item = Self::find(data, project, id)?.clone();
            let question = item
                .request
                .as_mut()
                .ok_or(Error(StatusCode::NOT_FOUND, "request_not_found"))?;
            if let Some(reply) = &question.answer {
                // Exact retry recovers an answer whose response or durability receipt was lost.
                if body.expected_revision == 1
                    && item.revision == 2
                    && reply.actor == actor
                    && reply.text == body.text
                {
                    return Ok(item);
                }
                return Err(Error(StatusCode::CONFLICT, "request_already_answered"));
            }
            if item.revision != body.expected_revision {
                return Err(Error(StatusCode::CONFLICT, "request_revision_conflict"));
            }
            let now = crate::auth::now_epoch();
            question.answer = Some(crate::requests::Reply {
                text: body.text,
                actor: actor.into(),
                at: now,
            });
            item.revision += 1;
            item.updated_at = now;
            item.editor = actor.into();
            item.fields.state = State::Done;
            item.audit.push(Edit {
                revision: item.revision,
                at: now,
                actor: actor.into(),
                fields: item.fields.clone(),
            });
            data.insert(id.into(), item.clone());
            Ok(item)
        })
    }
}

fn validate(f: &Fields) -> Result<(), Error> {
    if f.title.trim().is_empty()
        || f.title.len() > 200
        || f.description.len() > 8000
        || f.owner.len() > 200
        || f.source_link.len() > 2048
        || [&f.title, &f.owner, &f.source_link]
            .iter()
            .any(|s| s.chars().any(char::is_control))
        || !safe_link(&f.source_link)
    {
        return Err(Error(
            StatusCode::UNPROCESSABLE_ENTITY,
            "invalid_work_item_fields",
        ));
    }
    Ok(())
}
fn safe_link(link: &str) -> bool {
    if link.is_empty() {
        return true;
    }
    if link.contains('\\') || link.chars().any(char::is_control) {
        return false;
    }
    if link.starts_with('/') {
        return link.starts_with("/ui/")
            && !link.contains('%')
            && !link.split('/').any(|p| p == ".." || p == ".");
    }
    reqwest::Url::parse(link).is_ok_and(|u| {
        u.scheme() == "https"
            && u.host_str().is_some()
            && u.username().is_empty()
            && u.password().is_none()
    })
}
