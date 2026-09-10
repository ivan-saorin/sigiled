//! Exact D1 DTOs. Unknown metadata stays absent; source strings never grant edit authority.
use super::*;
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub(super) enum Selector {
    Manual {
        id: String,
    },
    Document {
        source: String,
        #[serde(rename = "ref")]
        refid: String,
        path: String,
    },
    Source {
        source: String,
        #[serde(rename = "ref")]
        refid: String,
    },
}
impl Selector {
    pub fn validate(&self, scope: bool) -> Result<(), Error> {
        match self {
            Self::Manual { id } => {
                if !manual_id(id) {
                    return Err(invalid());
                }
            }
            Self::Document {
                source,
                refid,
                path,
            } => {
                field(source, 128, false)?;
                field(refid, 2048, false)?;
                field(path, 4096, true)?;
            }
            Self::Source { source, refid } => {
                if !scope || source == "manual" {
                    return Err(invalid());
                }
                field(source, 128, false)?;
                field(refid, 2048, false)?;
            }
        }
        Ok(())
    }
}
#[derive(Clone, Deserialize, Serialize)]
pub(super) struct Actor {
    key: String,
    principal_kind: String,
    issuer: Option<String>,
    subject: Option<String>,
}
#[derive(Clone, Deserialize, Serialize)]
pub(super) struct Context {
    chunk_id: String,
    span: String,
    sha: String,
}
#[derive(Clone, Deserialize, Serialize)]
pub(super) struct Annotation {
    text: String,
    actor: Actor,
    at: i64,
    context: Option<Context>,
}
#[derive(Clone, Deserialize, Serialize)]
pub(super) struct Curation {
    pub revision: String,
    pub pinned: bool,
    pub archived: bool,
    pub annotations: Vec<Annotation>,
    updated_by: Option<Actor>,
    updated_at: Option<i64>,
}
#[derive(Clone, Deserialize, Serialize)]
pub(super) struct Chunk {
    pub id: String,
    #[serde(default)]
    pub idx: Option<String>,
    pub text: String,
    pub source: String,
    #[serde(rename = "ref")]
    pub refid: String,
    pub path: String,
    #[serde(default)]
    pub span: Option<String>,
    #[serde(default)]
    pub ts: Option<i64>,
    #[serde(default)]
    pub sha: Option<String>,
    #[serde(default)]
    pub tags: Option<Vec<String>>,
    #[serde(default)]
    pub target: Option<Selector>,
    #[serde(default)]
    pub curation: Option<Curation>,
}
#[derive(Deserialize, Serialize)]
pub(super) struct Manual {
    pub id: String,
    pub revision: String,
    text: String,
    tags: Vec<String>,
    actor: Actor,
    created_by: Actor,
    updated_at: i64,
    deleted: bool,
    projection: String,
}
impl Manual {
    pub fn validate(&self) -> Result<(), Error> {
        if !manual_id(&self.id)
            || revision(&self.revision).is_err()
            || !matches!(self.projection.as_str(), "indexed" | "pending" | "failed")
        {
            Err(unavailable())
        } else {
            Ok(())
        }
    }
}
#[derive(Deserialize, Serialize)]
pub(super) struct Detail {
    pub memory: Manual,
    pub target: Selector,
    pub curation: Curation,
}
#[derive(Deserialize, Serialize)]
pub(super) struct Curated {
    pub target: Selector,
    pub curation: Curation,
}
#[derive(Deserialize, Serialize)]
pub(super) struct Page {
    pub items: Vec<Chunk>,
    pub next_cursor: Option<String>,
    pub scanned: usize,
    pub consistency: String,
}
#[derive(Deserialize, Serialize)]
pub(super) struct History {
    pub items: Vec<Manual>,
    pub next_after: Option<String>,
}
#[derive(Deserialize, Serialize)]
pub(super) struct Preview {
    pub selector: Selector,
    pub affected: u64,
    pub sample: Vec<Sample>,
    pub confirmation: String,
    pub index_version: String,
    pub curation_version: String,
}
#[derive(Deserialize, Serialize)]
pub(super) struct Sample {
    pub id: String,
    #[serde(default)]
    source: Option<String>,
    #[serde(default, rename = "ref")]
    refid: Option<String>,
    #[serde(default)]
    path: Option<String>,
}
#[derive(Deserialize, Serialize)]
pub(super) struct Projection {
    #[serde(default)]
    durability: Option<String>,
    #[serde(default)]
    pending: Option<u64>,
    #[serde(default)]
    failed: Option<u64>,
    #[serde(default)]
    index_deletions_pending: Option<u64>,
}
#[derive(Deserialize, Serialize)]
pub(super) struct Forgotten {
    suppressed: bool,
    selector: Selector,
    affected: u64,
    projection: Projection,
}
#[derive(Deserialize, Serialize)]
pub(super) struct Index {
    pub name: String,
    #[serde(default)]
    rows: Option<u64>,
    #[serde(default)]
    last_ingest: Option<String>,
    #[serde(default)]
    model: Option<String>,
}
#[derive(Deserialize)]
pub(super) struct Search {
    pub hits: Vec<SearchHit>,
}
#[derive(Deserialize)]
pub(super) struct SearchHit {
    pub id: String,
    #[serde(default)]
    pub idx: Option<String>,
    pub text: String,
    pub source: String,
    #[serde(rename = "ref")]
    pub refid: String,
    pub path: String,
    #[serde(default)]
    pub span: Option<String>,
    #[serde(default)]
    pub ts_epoch: Option<i64>,
    #[serde(default)]
    pub sha: Option<String>,
    #[serde(default)]
    pub tags: Option<Vec<String>>,
}
impl From<SearchHit> for Chunk {
    fn from(h: SearchHit) -> Self {
        Self {
            id: h.id,
            idx: h.idx,
            text: h.text,
            source: h.source,
            refid: h.refid,
            path: h.path,
            span: h.span,
            ts: h.ts_epoch,
            sha: h.sha,
            tags: h.tags,
            target: None,
            curation: None,
        }
    }
}
#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Create {
    pub create_id: String,
    pub text: String,
    #[serde(default)]
    pub tags: Vec<String>,
}
#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Edit {
    pub expected_revision: String,
    pub text: String,
    #[serde(default)]
    pub tags: Vec<String>,
}
#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Curate {
    pub target: Selector,
    pub expected_revision: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pinned: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub archived: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub annotation: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub context_chunk_id: Option<String>,
}
#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Forget {
    pub selector: Selector,
    pub confirmation: String,
}
#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(super) struct PreviewInput {
    pub selector: Selector,
}

impl Chunk {
    pub(super) fn validate(&self, index: &str) -> Result<(), Error> {
        if self.idx.as_deref().is_some_and(|n| n != index) {
            return Err(unavailable());
        }
        if let Some(c) = &self.curation {
            revision(&c.revision).map_err(|_| unavailable())?;
        }
        match &self.target {
            Some(Selector::Manual { id })
                if id == &self.id
                    && self.source == "manual"
                    && self.refid == format!("memory:{id}")
                    && self.path.is_empty() =>
            {
                if !manual_id(id) {
                    return Err(unavailable());
                }
            }
            Some(Selector::Document {
                source,
                refid,
                path,
            }) if source == &self.source && refid == &self.refid && path == &self.path => {}
            None => {}
            _ => return Err(unavailable()),
        }
        Ok(())
    }
}
