use std::{collections::HashSet, time::Instant};
pub struct Activity {
    last: Instant,
    commands: HashSet<String>,
    uncertain: bool,
}
impl Default for Activity {
    fn default() -> Self {
        Self {
            last: Instant::now(),
            commands: HashSet::new(),
            uncertain: false,
        }
    }
}
impl Activity {
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
        self.uncertain || !self.commands.is_empty()
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
