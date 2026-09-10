//! Deadline-controlled subprocesses for repository observation.
//! Each command gets a private Unix process group. Deadline/error cleanup kills
//! that group and reaps its leader before the caller can release its mirror lock.
use std::{
    process::{Command, Output},
    time::Instant,
};
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Error {
    Deadline,
    Io,
    Exit,
    OutputTooLarge,
    LockBusy,
    #[cfg(not(target_os = "linux"))]
    UnsupportedPlatform,
}
#[cfg(target_os = "linux")]
pub fn output_until(command: &mut Command, deadline: Instant) -> Result<Output, Error> {
    use std::{
        io::{ErrorKind, Read},
        os::{fd::AsRawFd, unix::process::CommandExt},
        process::{Child, Stdio},
        time::Duration,
    };
    struct Group {
        child: Child,
        cleaned: bool,
    }
    impl Group {
        fn finish(&mut self) -> Result<std::process::ExitStatus, Error> {
            self.cleaned = true;
            // waitid(WNOWAIT) keeps the leader unreaped, reserving its PID/PGID
            // until cleanup. Never signal a potentially reused process-group ID.
            unsafe {
                libc::kill(-(self.child.id() as libc::pid_t), libc::SIGKILL);
            }
            let _ = self.child.kill();
            self.child.wait().map_err(|_| Error::Io)
        }
        fn exited(&self) -> Result<bool, Error> {
            let mut info = unsafe { std::mem::zeroed::<libc::siginfo_t>() };
            let result = unsafe {
                libc::waitid(
                    libc::P_PID,
                    self.child.id(),
                    &mut info,
                    libc::WEXITED | libc::WNOHANG | libc::WNOWAIT,
                )
            };
            if result < 0 {
                if std::io::Error::last_os_error().kind() == ErrorKind::Interrupted {
                    return Ok(false);
                }
                return Err(Error::Io);
            }
            Ok(unsafe { info.si_pid() } != 0)
        }
    }
    impl Drop for Group {
        fn drop(&mut self) {
            if !self.cleaned {
                let _ = self.finish();
            }
        }
    }
    fn nonblocking(pipe: &impl AsRawFd) -> Result<(), Error> {
        let fd = pipe.as_raw_fd();
        let flags = unsafe { libc::fcntl(fd, libc::F_GETFL) };
        if flags < 0 || unsafe { libc::fcntl(fd, libc::F_SETFL, flags | libc::O_NONBLOCK) } < 0 {
            return Err(Error::Io);
        }
        Ok(())
    }
    fn drain(pipe: &mut impl Read, bytes: &mut Vec<u8>, eof: &mut bool) -> Result<(), Error> {
        let mut buffer = [0u8; 8192];
        // Bound each drain so a noisy child cannot prevent deadline checks.
        for _ in 0..16 {
            match pipe.read(&mut buffer) {
                Ok(0) => {
                    *eof = true;
                    return Ok(());
                }
                Ok(n) => {
                    if bytes.len() + n > 4 * 1024 * 1024 {
                        return Err(Error::OutputTooLarge);
                    }
                    bytes.extend_from_slice(&buffer[..n]);
                }
                Err(e) if e.kind() == ErrorKind::WouldBlock => return Ok(()),
                Err(e) if e.kind() == ErrorKind::Interrupted => continue,
                Err(_) => return Err(Error::Io),
            }
        }
        Ok(())
    }
    if Instant::now() >= deadline {
        return Err(Error::Deadline);
    }
    let mut group = Group {
        child: command
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .process_group(0)
            .spawn()
            .map_err(|_| Error::Io)?,
        cleaned: false,
    };
    let mut stdout = group.child.stdout.take().ok_or(Error::Io)?;
    let mut stderr = group.child.stderr.take().ok_or(Error::Io)?;
    nonblocking(&stdout)?;
    nonblocking(&stderr)?;
    let (mut out, mut err) = (Vec::new(), Vec::new());
    let (mut out_eof, mut err_eof) = (false, false);
    loop {
        if Instant::now() >= deadline {
            return Err(Error::Deadline);
        }
        drain(&mut stdout, &mut out, &mut out_eof)?;
        drain(&mut stderr, &mut err, &mut err_eof)?;
        if out_eof && err_eof && group.exited()? {
            let status = group.finish()?;
            return Ok(Output {
                status,
                stdout: out,
                stderr: err,
            });
        }
        std::thread::sleep(
            Duration::from_millis(5).min(deadline.saturating_duration_since(Instant::now())),
        );
    }
}
#[cfg(not(target_os = "linux"))]
pub fn output_until(_command: &mut Command, _deadline: Instant) -> Result<Output, Error> {
    // The production runtime is Linux; fail closed without its cleanup guarantees.
    Err(Error::UnsupportedPlatform)
}
/// Observation commands never launch automatic/detached maintenance or hooks.
/// Command-local overrides leave all lifecycle/runtime Git callers unchanged.
pub fn git_command() -> Command {
    let mut command = Command::new("git");
    command.args([
        "-c",
        "maintenance.auto=false",
        "-c",
        "gc.auto=0",
        "-c",
        "gc.autoDetach=false",
        "-c",
        "core.hooksPath=/dev/null",
    ]);
    command
}
pub fn git(repo: &std::path::Path, args: &[&str], deadline: Instant) -> Result<String, Error> {
    let output = output_until(git_command().arg("-C").arg(repo).args(args), deadline)?;
    if !output.status.success() {
        return Err(Error::Exit);
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}
#[cfg(all(test, target_os = "linux"))]
mod tests {
    use super::*;
    use std::time::Duration;
    #[test]
    fn deadline_terminates_stalled_parent_and_child() {
        let start = Instant::now();
        let result = output_until(
            Command::new("sh").args(["-c", "trap '' TERM; sleep 0.6 & wait"]),
            start + Duration::from_millis(60),
        );
        assert_eq!(result.unwrap_err(), Error::Deadline);
        assert!(
            start.elapsed() < Duration::from_millis(400),
            "deadline left the process running"
        );
    }
    #[test]
    fn deadline_terminates_descendant_holding_pipes_after_parent_exits() {
        let start = Instant::now();
        let result = output_until(
            Command::new("sh").args(["-c", "sleep 0.6 & exit 0"]),
            start + Duration::from_millis(60),
        );
        assert_eq!(result.unwrap_err(), Error::Deadline);
        assert!(
            start.elapsed() < Duration::from_millis(400),
            "inherited output pipe escaped deadline"
        );
    }
}

#[cfg(all(test, target_os = "linux"))]
mod cleanup_tests {
    use super::*;
    use std::time::Duration;
    #[test]
    fn timeout_reaps_its_leader_and_terminates_its_descendant() {
        let root = crate::AppState::test_without_runtime()
            .sessions
            .repos_dir
            .unwrap();
        let deadline = Instant::now() + Duration::from_millis(120);
        let result = output_until(
            Command::new("sh")
                .args([
                    "-c",
                    "echo $$ > \"$1/parent\"; sleep 5 & echo $! > \"$1/child\"; wait",
                    "fixture",
                ])
                .arg(&root),
            deadline,
        );
        assert_eq!(result.unwrap_err(), Error::Deadline);
        let parent = std::fs::read_to_string(root.join("parent")).unwrap();
        let child = std::fs::read_to_string(root.join("child")).unwrap();
        assert!(
            !std::path::Path::new(&format!("/proc/{}", parent.trim())).exists(),
            "leader was not reaped"
        );
        // Descendant reaping belongs to its new parent/init after group kill;
        // it must be gone or terminated, never running with inherited pipes.
        if let Ok(stat) = std::fs::read_to_string(format!("/proc/{}/stat", child.trim())) {
            let status = stat.rsplit_once(") ").unwrap().1.chars().next().unwrap();
            assert!(
                ['Z', 'X'].contains(&status),
                "descendant is still running: {status}"
            );
        }
    }
    #[test]
    fn output_cap_stops_continuous_writer_and_cleans_group() {
        let started = Instant::now();
        let result = output_until(
            Command::new("sh").args(["-c", "yes bounded-output"]),
            started + Duration::from_secs(2),
        );
        assert_eq!(result.unwrap_err(), Error::OutputTooLarge);
        assert!(started.elapsed() < Duration::from_secs(2));
    }
    #[test]
    fn normal_command_preserves_exit_and_both_streams() {
        let output = output_until(
            Command::new("sh").args(["-c", "printf stdout; printf stderr >&2; exit 7"]),
            Instant::now() + Duration::from_secs(1),
        )
        .unwrap();
        assert_eq!(output.status.code(), Some(7));
        assert_eq!(output.stdout, b"stdout");
        assert_eq!(output.stderr, b"stderr");
    }
}

#[cfg(all(test, target_os = "linux"))]
mod git_policy_tests {
    #[test]
    fn refresh_git_overrides_automatic_maintenance_and_hooks_locally() {
        let repo = crate::merge::tests::mk_repo("registry-git-policy");
        crate::merge::tests::sh(&repo, &["config", "maintenance.auto", "true"]);
        crate::merge::tests::sh(&repo, &["config", "gc.auto", "1"]);
        for (key, expected) in [
            ("maintenance.auto", "false"),
            ("gc.auto", "0"),
            ("gc.autoDetach", "false"),
            ("core.hooksPath", "/dev/null"),
        ] {
            assert_eq!(
                super::git(
                    &repo,
                    &["config", "--get", key],
                    std::time::Instant::now() + std::time::Duration::from_secs(1)
                )
                .unwrap(),
                expected
            );
        }
        // The repository config itself is unchanged by the scoped overrides.
        assert_eq!(
            crate::merge::git(&repo, &["config", "--get", "maintenance.auto"])
                .unwrap()
                .trim(),
            "true"
        );
    }
}
