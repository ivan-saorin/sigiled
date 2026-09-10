use std::path::Path;
pub fn seed(root: &Path, live: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(live)?;
    if live.join(".seeded").exists() {
        return Ok(());
    }
    if let Ok(name) = std::fs::read_to_string(root.join("preferences-current")) {
        if !name.is_empty() && name.bytes().all(|c| c.is_ascii_alphanumeric() || c == b'-') {
            let snapshot = root.join("preferences").join(name);
            copy_tree(&snapshot.join("User"), &live.join("data/User"))?;
            copy_tree(&snapshot.join("extensions"), &live.join("extensions"))?;
        }
    }
    std::fs::write(live.join(".seeded"), [])
}
fn copy_tree(from: &Path, to: &Path) -> std::io::Result<()> {
    if !from.exists() {
        return Ok(());
    }
    std::fs::create_dir_all(to)?;
    for entry in std::fs::read_dir(from)? {
        let entry = entry?;
        let ty = entry.file_type()?;
        if ty.is_dir() {
            copy_tree(&entry.path(), &to.join(entry.file_name()))?
        } else if ty.is_file() {
            std::fs::copy(entry.path(), to.join(entry.file_name()))?;
        }
    }
    Ok(())
}
pub fn save(root: &Path, live: &Path) -> std::io::Result<()> {
    let name = format!(
        "snapshot-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos()
    );
    let destination = root.join("preferences").join(&name);
    std::fs::create_dir_all(&destination)?;
    copy_tree(&live.join("data/User"), &destination.join("User"))?;
    copy_tree(&live.join("extensions"), &destination.join("extensions"))?;
    let temporary = root.join(format!("pointer-{name}"));
    std::fs::write(&temporary, &name)?;
    std::fs::rename(temporary, root.join("preferences-current"))
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn profiles_seed_preferences_without_sharing_live_database() {
        let root = std::path::PathBuf::from(format!(
            "/workspace/target/ide-profile-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&root);
        let one = root.join("one");
        seed(&root, &one).unwrap();
        std::fs::create_dir_all(one.join("data/User")).unwrap();
        std::fs::write(one.join("data/User/settings.json"), "deliberate").unwrap();
        save(&root, &one).unwrap();
        let two = root.join("two");
        seed(&root, &two).unwrap();
        std::fs::write(two.join("data/User/settings.json"), "changed").unwrap();
        assert_eq!(
            std::fs::read_to_string(one.join("data/User/settings.json")).unwrap(),
            "deliberate"
        );
        seed(&root, &two).unwrap();
        assert_eq!(
            std::fs::read_to_string(two.join("data/User/settings.json")).unwrap(),
            "changed"
        );
    }
}
