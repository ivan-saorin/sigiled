//! Deadline-controlled subprocesses for repository observation.
//! Each command gets a private Unix process group. Deadline/error cleanup kills
//! that group, confirms every process/thread has exited, then reaps the leader.
//! Uncertain termination quarantines the owning blocking worker and its resources;
//! the async refresh caller has a separate bounded acknowledgement grace period.
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
    output_until_observed(command, deadline, |_| Ok(true))
}
#[cfg(target_os = "linux")]
pub(crate) fn output_until_observed<F: FnMut(u32) -> Result<bool, Error>>(
    command: &mut Command,
    deadline: Instant,
    observer: F,
) -> Result<Output, Error> {
    use std::{
        io::{ErrorKind, Read},
        os::{fd::AsRawFd, unix::process::CommandExt},
        process::{Child, Stdio},
        time::Duration,
    };
    struct Group<F: FnMut(u32) -> Result<bool, Error>> {
        observer: F,
        child: Child,
        cleaned: bool,
    }
    impl<F: FnMut(u32) -> Result<bool, Error>> Group<F> {
        fn finish(&mut self) -> Result<std::process::ExitStatus, Error> {
            // Keep the leader unreaped until both waitid and complete /proc task
            // scans acknowledge exit. Its reserved PID also reserves the PGID.
            // No uncertainty may unwind into repository cleanup or guard release.
            let mut quiet_passes = 0;
            let mut warned = false;
            loop {
                let verified =
                    (|| {
                        let exited = self.exited()?;
                        // Never signal a stale identity after waitid loses ownership.
                        let result =
                            unsafe { libc::kill(-(self.child.id() as libc::pid_t), libc::SIGKILL) };
                        if result < 0
                            && std::io::Error::last_os_error().raw_os_error() != Some(libc::ESRCH)
                        {
                            return Err(Error::Io);
                        }
                        // The direct child normally remains in its initial group.
                        // Also terminate it if a command changed its own group.
                        let result =
                            unsafe { libc::kill(self.child.id() as libc::pid_t, libc::SIGKILL) };
                        if result < 0
                            && std::io::Error::last_os_error().raw_os_error() != Some(libc::ESRCH)
                        {
                            return Err(Error::Io);
                        }
                        Ok(exited
                            && group_quiet(self.child.id())?
                            && (self.observer)(self.child.id())?)
                    })();
                let mut retry = Duration::from_millis(10);
                match verified {
                    Ok(true) => {
                        quiet_passes += 1;
                        if quiet_passes >= 2 {
                            match self.child.wait() {
                                Ok(status) => {
                                    self.cleaned = true;
                                    return Ok(status);
                                }
                                Err(_) => quiet_passes = 0,
                            }
                        }
                    }
                    Ok(false) => quiet_passes = 0,
                    Err(_) => {
                        retry = Duration::from_millis(250);
                        quiet_passes = 0;
                        if !warned {
                            eprintln!("repository subprocess termination unconfirmed; retaining mirror ownership");
                            warned = true;
                        }
                    }
                }
                std::thread::sleep(retry);
            }
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
    impl<F: FnMut(u32) -> Result<bool, Error>> Drop for Group<F> {
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
        observer,
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
/// /proc is inspected while the caller still owns the unreaped group leader.
/// Zombies/dead tasks cannot perform further filesystem work; orphan zombies are
/// reaped by their parent/init, not by a process-global subreaper in this daemon.
#[cfg(target_os = "linux")]
fn group_quiet(pgid: u32) -> Result<bool, Error> {
    group_quiet_at(std::path::Path::new("/proc"), pgid, 65_536)
}
#[cfg(target_os = "linux")]
fn group_quiet_at(root: &std::path::Path, pgid: u32, max_entries: usize) -> Result<bool, Error> {
    use std::{
        fs,
        io::{ErrorKind, Read},
        path::Path,
        time::Duration,
    };
    struct Budget {
        left: usize,
        until: Instant,
    }
    impl Budget {
        fn take(&mut self) -> Result<(), Error> {
            if self.left == 0 || Instant::now() >= self.until {
                return Err(Error::Io);
            }
            self.left -= 1;
            Ok(())
        }
    }
    fn stat(path: &Path) -> Result<Option<(u32, u8, u32)>, Error> {
        let file = match fs::File::open(path) {
            Ok(file) => file,
            Err(e) if e.kind() == ErrorKind::NotFound => return Ok(None),
            Err(_) => return Err(Error::Io),
        };
        let mut bytes = Vec::new();
        file.take(4097)
            .read_to_end(&mut bytes)
            .map_err(|_| Error::Io)?;
        if bytes.len() > 4096 {
            return Err(Error::Io);
        }
        parse_stat(&bytes).map(Some)
    }
    fn terminal(state: u8) -> bool {
        matches!(state, b'Z' | b'X' | b'x')
    }
    let mut budget = Budget {
        left: max_entries,
        until: Instant::now() + Duration::from_millis(100),
    };
    let processes = fs::read_dir(root).map_err(|_| Error::Io)?;
    let mut saw_reserved_leader = false;
    for entry in processes {
        budget.take()?;
        let entry = entry.map_err(|_| Error::Io)?;
        let name = entry.file_name();
        let Some(pid) = name.to_str().and_then(|name| name.parse::<u32>().ok()) else {
            continue;
        };
        let Some((actual_pid, state, group)) = stat(&entry.path().join("stat"))? else {
            continue;
        };
        if actual_pid != pid {
            return Err(Error::Io);
        }
        if group != pgid {
            continue;
        }
        saw_reserved_leader |= pid == pgid;
        if !terminal(state) {
            return Ok(false);
        }
        // A zombie process leader alone is insufficient: sibling threads may
        // still be exiting. Inspect every task, including its dead leader.
        let tasks = match fs::read_dir(entry.path().join("task")) {
            Ok(tasks) => tasks,
            Err(e) if e.kind() == ErrorKind::NotFound => {
                // Missing task visibility is safe only after the process itself
                // vanished. A dead main thread may leave live sibling threads.
                if stat(&entry.path().join("stat"))?.is_some() {
                    return Err(Error::Io);
                }
                continue;
            }
            Err(_) => return Err(Error::Io),
        };
        let mut saw_leader = false;
        for task in tasks {
            budget.take()?;
            let task = task.map_err(|_| Error::Io)?;
            let tid = task
                .file_name()
                .to_str()
                .and_then(|name| name.parse::<u32>().ok())
                .ok_or(Error::Io)?;
            if let Some((actual_tid, state, group)) = stat(&task.path().join("stat"))? {
                if actual_tid != tid || group != pgid {
                    return Err(Error::Io);
                }
                saw_leader |= tid == pid;
                if !terminal(state) {
                    return Ok(false);
                }
            }
        }
        if !saw_leader && stat(&entry.path().join("stat"))?.is_some() {
            return Err(Error::Io);
        }
    }
    budget.take()?;
    // waitid still owns this unreaped leader: an empty/foreign proc mount is
    // missing visibility, not proof that the owned process group disappeared.
    if !saw_reserved_leader {
        return Err(Error::Io);
    }
    Ok(true)
}
#[cfg(target_os = "linux")]
fn parse_stat(bytes: &[u8]) -> Result<(u32, u8, u32), Error> {
    let text = std::str::from_utf8(bytes).map_err(|_| Error::Io)?;
    let (pid, rest) = text.split_once(" (").ok_or(Error::Io)?;
    let (_, fields) = rest.rsplit_once(") ").ok_or(Error::Io)?;
    let mut fields = fields.split_whitespace();
    let state = fields.next().ok_or(Error::Io)?.as_bytes();
    if state.len() != 1 || !b"RSDZTWtXxKPI".contains(&state[0]) {
        return Err(Error::Io);
    }
    fields
        .next()
        .ok_or(Error::Io)?
        .parse::<u32>()
        .map_err(|_| Error::Io)?;
    let group = fields
        .next()
        .ok_or(Error::Io)?
        .parse()
        .map_err(|_| Error::Io)?;
    Ok((pid.parse().map_err(|_| Error::Io)?, state[0], group))
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

#[cfg(all(test, target_os = "linux"))]
mod inspection_tests {
    use super::*;
    fn fixture() -> std::path::PathBuf {
        crate::AppState::test_without_runtime()
            .sessions
            .repos_dir
            .unwrap()
    }
    fn write_stat(root: &std::path::Path, path: &str, pid: u32, state: &str, group: u32) {
        let path = root.join(path);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(
            path,
            format!("{pid} (a tricky ) process) {state} 1 {group} 0 0 0\n"),
        )
        .unwrap();
    }
    #[test]
    fn proc_stat_parses_parentheses_and_rejects_malformed_identity_or_state() {
        assert_eq!(
            parse_stat(b"31 (name ) and ( name) Z 1 31 0"),
            Ok((31, b'Z', 31))
        );
        for text in [
            "",
            "31 name Z 1 31",
            "abc (name) Z 1 31",
            "31 (name) ZZ 1 31",
            "31 (name) ? 1 31",
            "31 (name) Z nope 31",
            "31 (name) Z 1 nope",
        ] {
            assert_eq!(parse_stat(text.as_bytes()), Err(Error::Io), "{text}");
        }
    }
    #[test]
    fn zombie_leader_is_not_quiet_while_any_thread_is_live() {
        let root = fixture();
        write_stat(&root, "31/stat", 31, "Z", 31);
        write_stat(&root, "31/task/31/stat", 31, "Z", 31);
        write_stat(&root, "31/task/32/stat", 32, "D", 31);
        assert_eq!(group_quiet_at(&root, 31, 100), Ok(false));
        write_stat(&root, "31/task/32/stat", 32, "X", 31);
        assert_eq!(group_quiet_at(&root, 31, 100), Ok(true));
    }
    #[test]
    fn proc_enumeration_and_malformed_or_oversized_reads_fail_closed() {
        let root = fixture();
        assert_eq!(group_quiet_at(&root, 31, 100), Err(Error::Io));
        assert_eq!(
            group_quiet_at(&root.join("missing"), 31, 100),
            Err(Error::Io)
        );
        write_stat(&root, "31/stat", 31, "Z", 31);
        assert_eq!(group_quiet_at(&root, 31, 100), Err(Error::Io));
        std::fs::create_dir(root.join("31/task")).unwrap();
        assert_eq!(group_quiet_at(&root, 31, 100), Err(Error::Io));
        std::fs::remove_dir(root.join("31/task")).unwrap();
        std::fs::write(root.join("31/task"), "not a directory").unwrap();
        assert_eq!(group_quiet_at(&root, 31, 100), Err(Error::Io));
        std::fs::remove_file(root.join("31/task")).unwrap();
        write_stat(&root, "31/task/31/stat", 31, "Z", 31);
        std::fs::write(root.join("31/stat"), "malformed").unwrap();
        assert_eq!(group_quiet_at(&root, 31, 100), Err(Error::Io));
        std::fs::write(root.join("31/stat"), "x".repeat(4097)).unwrap();
        assert_eq!(group_quiet_at(&root, 31, 100), Err(Error::Io));
        write_stat(&root, "31/stat", 32, "Z", 31);
        assert_eq!(group_quiet_at(&root, 31, 100), Err(Error::Io));
    }
    #[test]
    fn proc_scan_budget_exhaustion_is_uncertainty_never_quiescence() {
        let root = fixture();
        write_stat(&root, "31/stat", 31, "Z", 31);
        write_stat(&root, "31/task/31/stat", 31, "Z", 31);
        assert_eq!(group_quiet_at(&root, 31, 1), Err(Error::Io));
        assert_eq!(group_quiet_at(&root, 31, 100), Ok(true));
    }
}
