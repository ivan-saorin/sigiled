//! Versioned accepted snapshot DTO. Identity excludes commit/content provenance.
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
pub const MAX_BODY: usize = 1_100_000;
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Document {
    pub path: String,
    pub text: String,
    pub sha: String,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Snapshot {
    pub project: String,
    pub repository: String,
    pub owner: String,
    pub revision: String,
    pub commit: String,
    pub documents: Vec<Document>,
    pub shared: bool,
}
pub fn digest(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}
impl Snapshot {
    pub fn digest(&self) -> String {
        digest(&serde_json::to_vec(self).unwrap())
    }
    pub fn validate(&self) -> bool {
        let relative = |p: &str| {
            !p.is_empty()
                && p.len() <= 4096
                && !p.contains('\\')
                && p.split('/').all(|s| {
                    !s.is_empty()
                        && s != "."
                        && s != ".."
                        && !s.starts_with('.')
                        && !s.chars().any(char::is_control)
                })
        };
        let hex = |s: &str, n| {
            s.len() == n
                && s.bytes()
                    .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
        };
        self.project.len() >= 2
            && self.project.len() <= 39
            && self.project.as_bytes()[0].is_ascii_lowercase()
            && self
                .project
                .bytes()
                .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
            && self.project != "mem0"
            && self.repository.len() <= 2048
            && self.repository.split('/').count() == 2
            && self.repository.split('/').all(|s| {
                !s.is_empty()
                    && s.bytes()
                        .all(|b| b.is_ascii_alphanumeric() || b"-_.".contains(&b))
            })
            && self.owner.len() == 36
            && self.owner.bytes().enumerate().all(|(i, b)| {
                if [8, 13, 18, 23].contains(&i) {
                    b == b'-'
                } else {
                    b.is_ascii_digit() || (b'a'..=b'f').contains(&b)
                }
            })
            && self
                .revision
                .parse::<i64>()
                .is_ok_and(|n| n > 0 && n.to_string() == self.revision)
            && hex(&self.commit, 40)
            && self.documents.len() <= 64
            && self.documents.windows(2).all(|w| w[0].path < w[1].path)
            && self.documents.iter().all(|d| {
                relative(&d.path)
                    && d.text.len() <= 524288
                    && !d.text.contains('\0')
                    && d.sha == digest(d.text.as_bytes())
            })
            && serde_json::to_vec(self).is_ok_and(|v| v.len() <= MAX_BODY)
    }
}
