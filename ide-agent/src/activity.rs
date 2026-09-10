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
    conflict: bool,
    incomplete: bool,
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
            conflict: false,
            incomplete: false,
        }
    }
}
impl Activity {
    pub fn require_observation(&mut self) {
        self.observation_required = true;
        self.observer = None;
        self.observed_at = None;
        self.sequence = 0;
        self.conflict = false;
        self.incomplete = false;
    }
    pub fn stopped(&mut self) {
        self.observation_required = false;
        self.commands.clear();
    }
    pub fn observation(&self) -> &'static str {
        if !self.observation_required {
            return "not_required";
        }
        if self.conflict {
            return "conflict";
        }
        if self.incomplete {
            return "incomplete";
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
        if sequence.to_string() != value.sequence || sequence == 0 || !id(&value.observer) {
            return false;
        }
        // A distinct authenticated observer is evidence of writers outside this
        // custody. Neither fresh A telemetry nor time can discharge that conflict.
        if self.observer.as_ref().is_some_and(|o| o != &value.observer) {
            self.conflict = true;
            return false;
        }
        if self.conflict || sequence <= self.sequence {
            return false;
        }
        self.observer = Some(value.observer);
        // Retain the watermark even on incomplete newer evidence, so a delayed
        // complete packet cannot undo the newly observed uncertainty.
        self.sequence = sequence;
        // Version rejection is not proof that this observer has no writers.
        // Retain its single bounded custody identity before rejecting telemetry;
        // another observer cannot discharge it with an empty snapshot.
        if !self.observer.as_ref().unwrap().starts_with("v3:") {
            self.incomplete = true;
            return false;
        }
        if value.terminals.len() > 256
            || value.executions.len() > 4096
            || value.terminals.iter().any(|t| !id(&t.id))
            || value.executions.iter().any(|s| !id(s))
            || !matches!(
                value.event.as_deref(),
                None | Some("command_start" | "command_end")
            )
            || value
                .terminals
                .iter()
                .map(|t| &t.id)
                .collect::<HashSet<_>>()
                .len()
                != value.terminals.len()
            || value.executions.iter().collect::<HashSet<_>>().len() != value.executions.len()
        {
            self.incomplete = true;
            return false;
        }
        // This reserved unsupported terminal uses the v2 wire shape too: an old
        // helper sees busy, instead of rejecting an unknown overflow field while
        // retaining its preceding idle observation. Do not discard known commands.
        if value
            .terminals
            .iter()
            .any(|t| t.id == "observation-overflow")
        {
            self.incomplete = true;
            return true;
        }
        self.incomplete = false;
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
            observer: "v3:instance-1".into(),
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
    fn old_first_active_observer_retains_custody_against_new_and_delayed_traffic() {
        let mut a = Activity::default();
        a.require_observation();
        a.last = Instant::now() - std::time::Duration::from_secs(100);
        let mut old = snapshot(1, true);
        old.observer = "old-extension".into();
        old.terminals[0].id = "old-terminal".into();
        old.executions.push("old-running-command".into());
        old.event = Some("command_start".into());
        assert!(!a.observe(old));
        assert!(a.busy());
        assert_eq!(a.observation(), "incomplete");
        let mut new = snapshot(1, true);
        new.observer = "v3:new-extension".into();
        new.terminals.clear();
        a.observe(new);
        assert!(a.busy(), "empty B must not release old A's running command");
        assert_eq!(a.observation(), "conflict");
        for n in 2..5 {
            let mut delayed = snapshot(n, true);
            delayed.observer = "old-extension".into();
            delayed.terminals.clear();
            a.observe(delayed);
            let mut repeated = snapshot(n, true);
            repeated.observer = "v3:new-extension".into();
            repeated.terminals.clear();
            a.observe(repeated);
            assert!(a.busy());
            assert_eq!(a.observation(), "conflict");
        }
        a.observed_at = Some(Instant::now() - std::time::Duration::from_secs(100));
        assert!(a.busy());
        assert!(
            a.idle_secs() >= 100,
            "rejected telemetry cannot renew activity"
        );
    }
    #[test]
    fn old_extension_cannot_establish_new_contract_and_duplicate_capacity_is_benign() {
        let mut a = Activity::default();
        a.require_observation();
        let mut old = snapshot(1, true);
        old.observer = "old-extension".into();
        assert!(!a.observe(old));
        assert!(a.busy());
        assert_eq!(a.observation(), "incomplete");
        assert!(!a.observe(snapshot(3, true)));
        assert!(a.busy());
        assert_eq!(
            a.observation(),
            "conflict",
            "even empty old telemetry retains custody"
        );
        // Ordering is tested on a separate freshly custodied observer, not an
        // automatic old-to-new upgrade or a reset of the preceding conflict.
        let mut a = Activity::default();
        a.require_observation();
        assert!(a.observe(snapshot(3, true)));
        assert!(!a.busy());
        let mut delayed = snapshot(2, true);
        delayed.executions = (0..4097).map(|n| format!("c-{n}")).collect();
        assert!(!a.observe(delayed));
        assert!(
            !a.busy(),
            "same-observer delayed packet is not new evidence"
        );
        let mut invalid = snapshot(4, true);
        invalid.terminals[0].id = "invalid space".into();
        assert!(!a.observe(invalid));
        assert!(a.busy());
        assert!(!a.observe(snapshot(3, true)));
        assert!(a.busy());
        assert!(a.observe(snapshot(5, true)));
        assert!(!a.busy());
    }
    #[test]
    fn overflow_wire_is_legacy_unsupported_and_preserves_known_commands() {
        let mut a = Activity::default();
        a.require_observation();
        let mut first = snapshot(1, true);
        first.executions.push("known-command".into());
        assert!(a.observe(first));
        a.last = Instant::now() - std::time::Duration::from_secs(100);
        let notice:Observation=serde_json::from_value(serde_json::json!({"observer":"v3:instance-1","sequence":"2","terminals":[{"id":"observation-overflow","integrated":false}],"executions":[]})).unwrap();
        assert!(
            !notice.terminals[0].integrated,
            "v2 sees unsupported terminal in its unchanged wire shape"
        );
        assert!(a.observe(notice));
        assert!(a.busy());
        assert_eq!(a.observation(), "incomplete");
        assert!(a.commands.contains("known-command"));
        assert!(a.idle_secs() >= 100);
        assert!(!a.observe(snapshot(2, true)));
        assert!(a.busy());
        assert!(a.observe(snapshot(3, true)));
        assert!(!a.busy());
        assert!(a.idle_secs() >= 100);
    }
    #[test]
    fn fresh_observer_conflict_revokes_idle_permanently() {
        let mut a = Activity::default();
        a.require_observation();
        a.last = Instant::now() - std::time::Duration::from_secs(100);
        assert!(a.observe(snapshot(1, true)));
        assert!(!a.busy());
        let mut b = snapshot(1, true);
        b.observer = "v3:instance-2".into();
        assert!(!a.observe(b));
        assert!(
            a.busy(),
            "fresh A cannot authorize cleanup after evidence of B"
        );
        assert_eq!(a.observation(), "conflict");
        a.observe(snapshot(2, true));
        assert!(a.busy(), "later empty A cannot discharge B custody");
        assert!(a.idle_secs() >= 100);
    }
    #[test]
    fn fresh_capacity_rejection_preserves_commands_and_recovers_only_complete_snapshot() {
        let mut a = Activity::default();
        a.require_observation();
        a.last = Instant::now() - std::time::Duration::from_secs(100);
        assert!(a.observe(snapshot(1, true)));
        let mut large = snapshot(10, true);
        large.terminals = (0..257)
            .map(|n| Terminal {
                id: format!("t-{n}"),
                integrated: true,
            })
            .collect();
        assert!(!a.observe(large));
        assert!(
            a.busy(),
            "new over-limit evidence immediately revokes fresh idle"
        );
        assert_eq!(a.observation(), "incomplete");
        assert!(!a.observe(snapshot(9, true)));
        assert!(a.busy());
        assert!(a.observe(snapshot(11, true)));
        assert!(!a.busy());
        let mut running = snapshot(12, true);
        running.executions.push("known-command".into());
        assert!(a.observe(running));
        let mut too_many = snapshot(13, true);
        too_many.executions = (0..4097).map(|n| format!("c-{n}")).collect();
        assert!(!a.observe(too_many));
        assert!(a.commands.contains("known-command"));
        assert!(a.idle_secs() >= 100);
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
