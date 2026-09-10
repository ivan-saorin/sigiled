use std::{
    process::{Child, Command, Stdio},
    time::{Duration, Instant},
};
/// Owns a child process group. Never discovers/kills a process by name or PID file.
pub struct Process {
    child: Child,
    reaped: bool,
}
impl Process {
    pub fn start(command: &mut Command) -> Result<Self, &'static str> {
        use std::os::unix::process::CommandExt;
        command
            .process_group(0)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        Ok(Self {
            child: command.spawn().map_err(|_| "editor_start_failed")?,
            reaped: false,
        })
    }
    pub fn running(&mut self) -> bool {
        !self.reaped && self.exited().is_ok_and(|exited| !exited)
    }
    fn exited(&self) -> Result<bool, &'static str> {
        let mut info = unsafe { std::mem::zeroed::<libc::siginfo_t>() };
        if unsafe {
            libc::waitid(
                libc::P_PID,
                self.child.id(),
                &mut info,
                libc::WEXITED | libc::WNOHANG | libc::WNOWAIT,
            )
        } != 0
        {
            return Err("editor_ownership_unknown");
        }
        Ok(unsafe { info.si_pid() } != 0)
    }
    pub fn stop(&mut self) -> Result<(), &'static str> {
        if self.reaped {
            return Ok(());
        }
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            let exited = self.exited()?;
            // The unreaped child reserves this PGID throughout cleanup.
            unsafe {
                libc::kill(-(self.child.id() as i32), libc::SIGKILL);
                libc::kill(self.child.id() as i32, libc::SIGKILL);
            }
            if exited && crate::bounded_process::group_quiet(self.child.id()).unwrap_or(false) {
                self.child.wait().map_err(|_| "editor_stop_unconfirmed")?;
                self.reaped = true;
                return Ok(());
            }
            if Instant::now() > deadline {
                return Err("editor_stop_unconfirmed");
            }
            std::thread::sleep(Duration::from_millis(20));
        }
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn fake_editor_stop_restart_does_not_kill_independent_process() {
        let mut other = Process::start(Command::new("sh").args(["-c", "sleep 30"])).unwrap();
        let mut editor =
            Process::start(Command::new("sh").args(["-c", "sleep 30 & wait"])).unwrap();
        assert!(editor.running());
        editor.stop().unwrap();
        assert!(other.running());
        let mut again = Process::start(Command::new("sh").args(["-c", "sleep 30"])).unwrap();
        again.stop().unwrap();
        other.stop().unwrap();
    }
}
#[cfg(test)]
mod ownership_tests {
    use super::*;
    #[test]
    fn completed_stop_is_idempotent_and_lost_child_identity_is_not_signaled() {
        let mut p = Process::start(Command::new("sh").args(["-c", "sleep 30"])).unwrap();
        p.stop().unwrap();
        assert!(p.stop().is_ok());
        let mut lost = Process::start(Command::new("sh").args(["-c", "exit 0"])).unwrap();
        lost.child.wait().unwrap();
        let before = Instant::now();
        assert_eq!(lost.stop(), Err("editor_ownership_unknown"));
        assert!(before.elapsed() < Duration::from_secs(1));
    }
}
