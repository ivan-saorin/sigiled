use std::time::Instant;
pub struct Activity {
    last: Instant,
    commands: u32,
}
impl Default for Activity {
    fn default() -> Self {
        Self {
            last: Instant::now(),
            commands: 0,
        }
    }
}
impl Activity {
    pub fn report(&mut self, event: &str) -> bool {
        match event {
            "edit" | "save" | "selection" => self.last = Instant::now(),
            "command_start" => {
                self.last = Instant::now();
                self.commands = self.commands.saturating_add(1);
            }
            "command_end" => {
                self.last = Instant::now();
                self.commands = self.commands.saturating_sub(1);
            }
            _ => return false,
        }
        true
    }
    pub fn idle_secs(&self) -> u64 {
        self.last.elapsed().as_secs()
    }
    pub fn busy(&self) -> bool {
        self.commands > 0
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;
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
        assert!(a.report("command_start"));
        assert!(a.busy());
        a.report("command_end");
        assert!(!a.busy());
    }
}
