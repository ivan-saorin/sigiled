//! Immutable Git tree import. Never checks out or executes template contents.
use crate::{bounded_process, manifest::Manifest};
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeMap,
    path::Path,
    process::Command,
    time::{Duration, Instant},
};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Snapshot {
    pub commit: String,
    pub tree: String,
    pub source_tree: String,
    pub foundation: Option<String>,
    pub workspace: Option<String>,
}

#[derive(Deserialize, Default)]
#[serde(deny_unknown_fields)]
struct Preparation {
    #[serde(default)]
    identity: Vec<Identity>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Identity {
    file: String,
    path: Vec<String>,
    value: String,
}

pub fn sha(value: &str) -> bool {
    value.len() == 40
        && value
            .bytes()
            .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
}

struct Git<'a> {
    root: &'a Path,
    pat: &'a str,
    deadline: Instant,
    created_at: u64,
}
impl Git<'_> {
    fn run(&self, args: &[&str]) -> Result<Vec<u8>, String> {
        use base64::Engine;
        let mut command = Command::new("git");
        command
            .current_dir(self.root)
            .args([
                "-c",
                "core.hooksPath=/dev/null",
                "-c",
                "maintenance.auto=false",
                "-c",
                "gc.auto=0",
                "-c",
                "gc.autoDetach=false",
                "-c",
                "credential.helper=",
                "-c",
                "http.followRedirects=false",
                "-c",
                if cfg!(test) {
                    "protocol.file.allow=always"
                } else {
                    "protocol.file.allow=never"
                },
            ])
            .args(args)
            .env("GIT_AUTHOR_DATE", format!("{} +0000", self.created_at))
            .env("GIT_COMMITTER_DATE", format!("{} +0000", self.created_at))
            .env("GIT_TERMINAL_PROMPT", "0")
            .env("GIT_ASKPASS", "/bin/false")
            .env("SSH_ASKPASS", "/bin/false")
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env_remove("GIT_TRACE")
            .env_remove("GIT_TRACE_CURL")
            .env_remove("GIT_CURL_VERBOSE")
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_COUNT", "1")
            .env("GIT_CONFIG_KEY_0", "http.https://github.com/.extraheader")
            .env(
                "GIT_CONFIG_VALUE_0",
                format!(
                    "Authorization: Basic {}",
                    base64::engine::general_purpose::STANDARD
                        .encode(format!("x-access-token:{}", self.pat))
                ),
            );
        let output = bounded_process::output_until(&mut command, self.deadline)
            .map_err(|_| "template_git_unavailable: retry the same name after checking Git connectivity and process capacity".to_string())?;
        if !output.status.success() {
            #[cfg(test)]
            eprintln!(
                "template fixture Git {args:?}: {}",
                String::from_utf8_lossy(&output.stderr)
            );
            return Err("template_git_failed: source commit or destination branch could not be verified; retry the same name".into());
        }
        Ok(output.stdout)
    }
    fn text(&self, args: &[&str]) -> Result<String, String> {
        String::from_utf8(self.run(args)?)
            .map(|s| s.trim().to_string())
            .map_err(|_| "template_invalid_utf8".into())
    }
    fn blob(&self, commit: &str, path: &str) -> Result<String, String> {
        let bytes = self.run(&["show", &format!("{commit}:{path}")])?;
        if bytes.len() > 1024 * 1024 {
            return Err("template_manifest_too_large".into());
        }
        String::from_utf8(bytes).map_err(|_| "template_invalid_utf8".into())
    }
    fn replace(&self, path: &str, content: &[u8]) -> Result<(), String> {
        let temporary = self.root.join("sigil-input");
        std::fs::write(&temporary, content).map_err(|_| "template_staging_unavailable")?;
        let hash = self.text(&["hash-object", "-w", "--no-filters", "sigil-input"])?;
        self.run(&[
            "update-index",
            "--add",
            "--cacheinfo",
            "100644",
            &hash,
            path,
        ])?;
        Ok(())
    }
}

fn safe_file(path: &str) -> bool {
    !path.is_empty()
        && path.len() <= 300
        && !path.starts_with('/')
        && !path.contains(['\\', ':', '\0'])
        && path.split('/').all(|part| {
            !part.is_empty() && part != "." && part != ".." && !part.eq_ignore_ascii_case(".git")
        })
}

pub fn prepare(
    root: &Path,
    source_url: &str,
    pat: &str,
    intent: &crate::templates::Intent,
) -> Result<Snapshot, String> {
    // This directory is private to one attempt of an operation. Retain it on
    // failure; no template checkout, hooks, filters, submodules or LFS execution.
    std::fs::create_dir_all(root).map_err(|_| "template_staging_unavailable")?;
    let git = Git {
        root,
        pat,
        deadline: Instant::now() + Duration::from_secs(180),
        created_at: intent.created_at,
    };
    git.run(&["init", "--bare", "--quiet"])?;
    git.run(&[
        "fetch",
        "--no-tags",
        "--depth=1",
        "--",
        source_url,
        &intent.source_commit,
    ])?;
    if git.text(&["rev-parse", "FETCH_HEAD"])? != intent.source_commit {
        return Err("template_revision_mismatch".into());
    }
    let source_tree = git.text(&["rev-parse", &format!("{}^{{tree}}", intent.source_commit)])?;
    let mut files = BTreeMap::new();
    for entry in git
        .run(&["ls-tree", "-rz", &intent.source_commit])?
        .split(|b| *b == 0)
        .filter(|b| !b.is_empty())
    {
        let entry = std::str::from_utf8(entry).map_err(|_| "template_path_invalid")?;
        let (mode, path) = entry.split_once('\t').ok_or("template_tree_invalid")?;
        if !safe_file(path) {
            return Err("template_path_invalid".into());
        }
        if mode.starts_with("160000") {
            return Err(
                "template_submodule_unsupported: vendor dependencies before designation".into(),
            );
        }
        files.insert(path.to_string(), mode[..6].to_string());
    }
    let manifest_path = if files.contains_key("sigiled.toml") {
        "sigiled.toml"
    } else if files.contains_key("mgr.toml") {
        "mgr.toml"
    } else {
        return Err("template_manifest_missing: add sigiled.toml before designation".into());
    };
    let regular = |path: &str| {
        files
            .get(path)
            .is_some_and(|mode| mode == "100644" || mode == "100755")
    };
    if !regular(manifest_path) {
        return Err("template_manifest_not_regular".into());
    }
    for path in files
        .keys()
        .filter(|p| p.rsplit('/').next() == Some(".gitattributes"))
    {
        if !regular(path)
            || git
                .blob(&intent.source_commit, path)?
                .lines()
                .filter(|line| !line.trim_start().starts_with('#'))
                .any(|line| line.split_whitespace().any(|word| word == "filter=lfs"))
        {
            return Err("template_lfs_unsupported: store required assets as ordinary Git blobs before designation".into());
        }
    }
    let raw = git.blob(&intent.source_commit, manifest_path)?;
    let manifest = Manifest::parse(&raw).map_err(|_| "template_manifest_invalid")?;
    let mut value: toml::Value = toml::from_str(&raw).map_err(|_| "template_manifest_invalid")?;
    // v1 eligibility is for reusable workspace templates. Definitions that
    // enroll live workloads, mount existing state or reference stack secrets
    // require deliberate preparation; never silently reuse source authority.
    for key in ["app", "jobs", "service", "compose", "volumes", "secrets"] {
        if value
            .get(key)
            .is_some_and(|v| v.as_table().is_none_or(|t| !t.is_empty()))
        {
            return Err(format!(
                "template_runtime_binding: remove [{key}] from the reusable source"
            ));
        }
    }
    let workspace = manifest.workspace_dockerfile.clone();
    if workspace.as_ref().is_some_and(|p| !regular(p)) {
        return Err(
            "template_workspace_missing: declared Dockerfile must be a regular source file".into(),
        );
    }
    // Metadata is declarative identity, not a global text substitution.
    let table = value.as_table_mut().ok_or("template_manifest_invalid")?;
    let project = table
        .entry("project")
        .or_insert_with(|| toml::Value::Table(Default::default()));
    project
        .as_table_mut()
        .ok_or("template_manifest_invalid")?
        .insert(
            "display_name".into(),
            toml::Value::String(intent.name.clone()),
        );
    git.run(&["read-tree", &intent.source_commit])?;
    let mut edits = BTreeMap::from([(manifest_path.to_string(), value)]);
    if files.contains_key("sigiled-template.toml") {
        if !regular("sigiled-template.toml") {
            return Err("template_identity_invalid".into());
        }
        let preparation: Preparation =
            toml::from_str(&git.blob(&intent.source_commit, "sigiled-template.toml")?)
                .map_err(|_| "template_identity_invalid")?;
        if preparation.identity.len() > 32 {
            return Err("template_identity_too_large".into());
        }
        for binding in preparation.identity {
            if !safe_file(&binding.file)
                || !regular(&binding.file)
                || binding.path.is_empty()
                || binding.path.len() > 8
                || binding.file == "sigiled-template.toml"
            {
                return Err("template_identity_invalid".into());
            }
            if binding.file == manifest_path && binding.path != ["project", "display_name"] {
                return Err("template_identity_invalid: manifest bindings may only adapt project.display_name".into());
            }
            let replacement = match binding.value.as_str() {
                "project_name" => intent.name.clone(),
                "repository" => intent.repository.clone(),
                "project_id" => intent.operation.clone(),
                _ => return Err("template_identity_invalid".into()),
            };
            if !edits.contains_key(&binding.file) {
                edits.insert(
                    binding.file.clone(),
                    toml::from_str(&git.blob(&intent.source_commit, &binding.file)?)
                        .map_err(|_| "template_identity_invalid")?,
                );
            }
            let mut field = edits.get_mut(&binding.file).unwrap();
            for part in &binding.path {
                field = field
                    .get_mut(part)
                    .ok_or("template_identity_field_missing")?;
            }
            if !field.is_str() {
                return Err("template_identity_field_not_string".into());
            }
            *field = toml::Value::String(replacement);
        }
    }
    for (path, value) in edits {
        let content = toml::to_string_pretty(&value).map_err(|_| "template_identity_invalid")?;
        if path == manifest_path {
            Manifest::parse(&content).map_err(|_| "template_identity_invalid_manifest")?;
        }
        git.replace(&path, content.as_bytes())?;
    }
    let foundation = manifest
        .template
        .map(|pin| format!("{}@{}", pin.name, pin.version));
    let provenance = serde_json::json!({"format":1,"project_id":intent.operation,"project":intent.name,
        "repository":intent.repository,"template":{"repository":intent.source_repository,"repository_id":intent.source_repository_id,
        "requested_ref":intent.requested_ref,"commit":intent.source_commit,"tree":source_tree},"foundation":foundation,
        "approval_namespace":intent.operation,"inherited_approvals":"source_data_only"});
    git.replace(
        ".sigil/project.json",
        &serde_json::to_vec_pretty(&provenance).unwrap(),
    )?;
    let tree = git.text(&["write-tree"])?;
    // Identical retry yields the identical root commit; no source parents.
    let date = format!("{} +0000", intent.created_at);
    let commit = git.text(&[
        "-c",
        "user.name=SIGILED",
        "-c",
        "user.email=sigiled@localhost",
        "-c",
        "commit.gpgSign=false",
        "commit-tree",
        &tree,
        "-m",
        &format!(
            "Initialize {} from {}@{}\n\nSIGILED operation {} at {}",
            intent.name, intent.source_repository, intent.source_commit, intent.operation, date
        ),
    ])?;
    Ok(Snapshot {
        commit,
        tree,
        source_tree,
        foundation,
        workspace,
    })
}

pub fn publish(root: &Path, destination_url: &str, pat: &str, commit: &str) -> Result<(), String> {
    let git = Git {
        root,
        pat,
        deadline: Instant::now() + Duration::from_secs(180),
        created_at: 1,
    };
    let refs = git.text(&["ls-remote", "--heads", "--tags", "--", destination_url])?;
    let expected = format!("{commit}\trefs/heads/master");
    if refs == expected {
        return Ok(());
    }
    if !refs.is_empty() {
        return Err("destination_changed: repository is not the empty operation-owned destination; no files overwritten".into());
    }
    git.run(&[
        "push",
        "--force-with-lease=refs/heads/master:",
        "--",
        destination_url,
        &format!("{commit}:refs/heads/master"),
    ])?;
    if git.text(&["ls-remote", "--heads", "--tags", "--", destination_url])? != expected {
        return Err("destination_verification_failed".into());
    }
    Ok(())
}
