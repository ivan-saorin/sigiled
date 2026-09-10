use std::{collections::HashSet, time::Instant};
pub struct Activity {
    last: Instant,
    commands: HashSet<String>,
    uncertain: bool,
    observation_required: bool,
    observer: Option<String>,
    sequence: u64,
    observed_at: Option<Instant>,
    unsupported: bool,
}
impl Default for Activity {
    fn default() -> Self {
        Self {
            last: Instant::now(),
            commands: HashSet::new(),
            uncertain: false,
            observation_required: false,
            observer: None,
            sequence: 0,
            observed_at: None,
            unsupported: false,
        }
    }
}
impl Activity {
    pub fn require_observation(&mut self) {
        self.observation_required = true;
        self.observer = None;
        self.observed_at = None;
        self.sequence = 0;
    }
    pub fn stopped(&mut self) {
        self.observation_required = false;
        self.commands.clear();
    }
    pub fn observation(&self) -> &'static str {
        if !self.observation_required {
            return "not_required";
        }
        let Some(at) = self.observed_at else {
            return "missing";
        };
        if at.elapsed().as_secs() >= 10 {
            return "stale";
        }
        if self.unsupported {
            return "unsupported";
        }
        "ready"
    }
    pub fn observe(&mut self, value: Observation) -> bool {
        fn id(s: &str) -> bool {
            !s.is_empty()
                && s.len() <= 128
                && s.bytes()
                    .all(|c| c.is_ascii_alphanumeric() || b"-:".contains(&c))
        }
        let Ok(sequence) = value.sequence.parse::<u64>() else {
            return false;
        };
        if sequence.to_string() != value.sequence
            || sequence == 0
            || !id(&value.observer)
            || value.terminals.len() > 256
            || value.executions.len() > 4096
            || value.terminals.iter().any(|t| !id(&t.id))
            || value.executions.iter().any(|s| !id(s))
            || !matches!(
                value.event.as_deref(),
                None | Some("command_start" | "command_end")
            )
        {
            return false;
        }
        if self.observer.as_ref().is_some_and(|o| o != &value.observer) || sequence <= self.sequence
        {
            return false;
        }
        if value
            .terminals
            .iter()
            .map(|t| &t.id)
            .collect::<HashSet<_>>()
            .len()
            != value.terminals.len()
        {
            return false;
        }
        self.observer = Some(value.observer);
        self.sequence = sequence;
        self.observed_at = Some(Instant::now());
        self.unsupported = value.terminals.iter().any(|t| !t.integrated);
        self.commands = value.executions.into_iter().collect();
        if value.event.is_some() {
            self.last = Instant::now();
        }
        true
    }
    pub fn report(&mut self, event: &str) -> bool {
        match event {
            "edit" | "save" | "selection" => self.last = Instant::now(),
            _ => return false,
        }
        true
    }
    pub fn report_execution(&mut self, event: &str, id: Option<&str>) -> bool {
        if !matches!(event, "command_start" | "command_end") {
            return self.report(event);
        }
        let Some(id) = id.filter(|id| {
            !id.is_empty()
                && id.len() <= 128
                && id
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b"-:".contains(&b))
        }) else {
            return false;
        };
        if event == "command_start" {
            self.last = Instant::now();
            if self.commands.len() >= 4096 {
                self.uncertain = true;
            } else {
                self.commands.insert(id.to_string());
            }
        } else if self.commands.remove(id) {
            self.last = Instant::now();
        }
        // Unknown/duplicate ends do not change another execution or its lease.
        // An end delivered before its start leaves the late start active: no
        // expiry guesses that a known command has ended after a lost event.
        true
    }
    pub fn idle_secs(&self) -> u64 {
        self.last.elapsed().as_secs()
    }
    pub fn busy(&self) -> bool {
        self.uncertain
            || !self.commands.is_empty()
            || !matches!(self.observation(), "ready" | "not_required")
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;
    #[test]
    fn execution_identity_preserves_known_commands_under_lost_and_reordered_events() {
        let mut a = Activity::default();
        a.report_execution("command_start", Some("instance:A"));
        a.report_execution("command_end", Some("instance:lost-start"));
        assert!(a.busy(), "unmatched end must not clear A");
        a.report_execution("command_start", Some("instance:B"));
        a.report_execution("command_start", Some("instance:A"));
        a.report_execution("command_end", Some("instance:A"));
        a.report_execution("command_end", Some("instance:A"));
        assert!(a.busy(), "duplicate end must not clear B");
        a.report_execution("command_end", Some("instance:B"));
        assert!(!a.busy());
        a.report_execution("command_end", Some("instance:reordered"));
        a.report_execution("command_start", Some("instance:reordered"));
        assert!(
            a.busy(),
            "late start retains uncertain execution conservatively"
        );
        assert!(!a.report_execution("command_end", None));
        assert!(a.busy());
    }
    #[test]
    fn background_events_do_not_renew_lease() {
        let mut a = Activity {
            last: Instant::now() - Duration::from_secs(100),
            ..Activity::default()
        };
        for e in ["health", "poll", "sync", "heartbeat", "websocket"] {
            assert!(!a.report(e));
        }
        assert!(a.idle_secs() >= 100);
        assert!(a.report_execution("command_start", Some("test:1")));
        assert!(a.busy());
        a.report_execution("command_end", Some("test:1"));
        assert!(!a.busy());
    }
}

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Observation {
    pub observer: String,
    pub sequence: String,
    pub terminals: Vec<Terminal>,
    pub executions: Vec<String>,
    pub event: Option<String>,
}
#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Terminal {
    pub id: String,
    pub integrated: bool,
}
#[cfg(test)]
mod observation_tests {
    use super::*;
    fn snapshot(n: u64, integrated: bool) -> Observation {
        Observation {
            observer: "instance-1".into(),
            sequence: n.to_string(),
            terminals: vec![Terminal {
                id: "terminal-1".into(),
                integrated,
            }],
            executions: vec![],
            event: None,
        }
    }
    #[test]
    fn missing_stale_unsupported_and_reordered_observations_never_prove_idle() {
        let mut a = Activity::default();
        a.require_observation();
        assert!(a.busy());
        assert_eq!(a.observation(), "missing");
        a.last = Instant::now() - std::time::Duration::from_secs(100);
        assert!(a.observe(snapshot(1, false)));
        assert!(a.busy());
        assert_eq!(a.observation(), "unsupported");
        assert!(a.idle_secs() >= 100);
        assert!(a.observe(snapshot(3, true)));
        assert!(!a.busy());
        assert!(a.idle_secs() >= 100);
        assert!(!a.observe(snapshot(2, false)));
        assert!(!a.busy());
        a.observed_at = Some(Instant::now() - std::time::Duration::from_secs(11));
        assert!(a.busy());
        assert_eq!(a.observation(), "stale");
        let mut replacement = snapshot(4, true);
        replacement.observer = "new-extension".into();
        assert!(!a.observe(replacement));
        assert!(a.busy());
        let mut closed = snapshot(4, false);
        closed.terminals.clear();
        assert!(a.observe(closed));
        assert!(!a.busy());
        assert!(a.idle_secs() >= 100);
        let mut running = snapshot(5, true);
        running.executions.push("command-1".into());
        running.event = Some("command_start".into());
        assert!(a.observe(running));
        assert!(a.busy());
        assert!(a.observe(snapshot(6, true)));
        assert!(
            !a.busy(),
            "complete same-instance later snapshot confirms command end"
        );
    }
}
