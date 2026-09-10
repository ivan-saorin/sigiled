include!("../../shared/bounded_process.rs");
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
