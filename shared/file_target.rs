use serde::{Deserialize, Serialize};
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct FileTarget {
    pub path: String,
    pub line: Option<u32>,
    pub column: Option<u32>,
}
impl FileTarget {
    pub fn validate(&self) -> bool {
        !self.path.is_empty()
            && self.path.len() <= 1024
            && !self.path.starts_with('/')
            && !self.path.contains(['\\', ':', '%', '?', '#'])
            && !self.path.chars().any(char::is_control)
            && self
                .path
                .split('/')
                .all(|s| !s.is_empty() && s != "." && s != "..")
            && self.line.is_none_or(|n| (1..=10_000_000).contains(&n))
            && self
                .column
                .is_none_or(|n| self.line.is_some() && (1..=100_000).contains(&n))
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn project_relative_navigation_is_narrow() {
        let good = FileTarget {
            path: "src/main.rs".into(),
            line: Some(10),
            column: Some(2),
        };
        assert!(good.validate());
        for path in [
            "",
            "/etc/passwd",
            "../file",
            "a/../b",
            "https://evil",
            "a\\b",
            "a%2fb",
            "a#b",
            "a?b",
            "a\nfile",
            "a//b",
        ] {
            assert!(
                !FileTarget {
                    path: path.into(),
                    ..good.clone()
                }
                .validate(),
                "{path}"
            );
        }
        assert!(!FileTarget {
            line: Some(0),
            ..good.clone()
        }
        .validate());
        assert!(!FileTarget { line: None, ..good }.validate());
    }
}
