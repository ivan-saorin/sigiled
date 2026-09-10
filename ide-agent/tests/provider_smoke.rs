/// Release qualification only: download/checksum evidence lives in the runbook.
/// Normal CI does not download or silently substitute a provider.
#[tokio::test]
#[ignore = "requires explicitly checksum-verified pinned standalone provider"]
async fn pinned_provider_non_root_health_and_owned_cleanup() {
    use sigil_ide_agent::process::Process;
    use std::{path::PathBuf, process::Command, time::Duration};
    let provider = std::env::var("SIGIL_TEST_PROVIDER").expect("explicit verified provider path");
    let root = PathBuf::from(format!(
        "/workspace/target/ide-provider-health-{}",
        std::process::id()
    ));
    std::fs::create_dir_all(&root).unwrap();
    assert_ne!(
        unsafe { libc::geteuid() },
        0,
        "qualification must exercise a non-root project user"
    );
    let mut command = Command::new(provider);
    command
        .args([
            "--config",
            "/dev/null",
            "--auth",
            "none",
            "--disable-proxy",
            "--bind-addr",
            "127.0.0.1:18091",
            "--user-data-dir",
        ])
        .arg(root.join("data"))
        .arg("--extensions-dir")
        .arg(root.join("extensions"))
        .arg(&root)
        .env("XDG_CONFIG_HOME", root.join("config"))
        .env("XDG_DATA_HOME", root.join("xdg-data"));
    let mut child = Process::start(&mut command).unwrap();
    let http = reqwest::Client::builder()
        .timeout(Duration::from_secs(1))
        .build()
        .unwrap();
    let mut ready = false;
    for _ in 0..40 {
        if !child.running() {
            break;
        }
        if http
            .get("http://127.0.0.1:18091/healthz")
            .send()
            .await
            .is_ok_and(|r| r.status().is_success())
        {
            ready = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(500)).await;
    }
    child.stop().unwrap();
    assert!(ready, "provider failed bounded health probe");
    assert!(tokio::net::TcpStream::connect("127.0.0.1:18091")
        .await
        .is_err());
}
