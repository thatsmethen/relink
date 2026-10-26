use relink_core::Session;
use serde::{Deserialize, Serialize};
use std::{
    fs::{self, OpenOptions},
    io::{Read, Write},
    path::{Path, PathBuf},
};

const MAX_SESSION_BYTES: u64 = 32 * 1024 * 1024;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StudioDocument {
    pub studio_schema_version: u32,
    pub session: Session,
    pub output_routing: crate::routing::RoutingDraft,
}
impl StudioDocument {
    pub fn new(session: Session, output_routing: crate::routing::RoutingDraft) -> Self {
        Self {
            studio_schema_version: 1,
            session,
            output_routing,
        }
    }
}

pub fn default_path() -> Result<PathBuf, String> {
    // Explicit developer/test isolation without changing the user's home or
    // normal studio. No override is inferred from a missing/corrupt file.
    if let Some(path) = std::env::var_os("RELINK_STUDIO_PATH") {
        let path = PathBuf::from(path);
        if !path.is_absolute() || path.file_name().is_none() {
            return Err("RELINK_STUDIO_PATH must name an absolute studio file.".into());
        }
        return Ok(path);
    }
    #[cfg(target_os = "macos")]
    let base =
        std::env::var_os("HOME").map(|p| PathBuf::from(p).join("Library/Application Support"));
    #[cfg(target_os = "windows")]
    let base = std::env::var_os("APPDATA").map(PathBuf::from);
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    let base = std::env::var_os("XDG_DATA_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|p| PathBuf::from(p).join(".local/share")));
    base.map(|p| p.join("RaveLinkRust/session.json"))
        .ok_or_else(|| "Cannot locate your application-data folder.".into())
}

pub fn load(path: &Path) -> Result<Option<StudioDocument>, String> {
    let file = match fs::File::open(path) {
        Ok(file) => file,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(format!("Cannot open {}: {e}", path.display())),
    };
    let mut json = String::new();
    file.take(MAX_SESSION_BYTES + 1)
        .read_to_string(&mut json)
        .map_err(|e| format!("Cannot read studio: {e}"))?;
    if json.len() as u64 > MAX_SESSION_BYTES {
        return Err("Studio exceeds the 32 MiB file safety budget.".into());
    }
    let value: serde_json::Value =
        serde_json::from_str(&json).map_err(|e| format!("Cannot load studio: {e}"))?;
    if let Some(version) = value.get("studio_schema_version") {
        if version.as_u64() != Some(1) {
            return Err("Cannot load this studio document version.".into());
        }
        let document: StudioDocument =
            serde_json::from_value(value).map_err(|e| format!("Cannot load studio: {e}"))?;
        document
            .session
            .validate()
            .map_err(|e| format!("Cannot load studio: {e}"))?;
        Ok(Some(document))
    } else {
        // Legacy files contain a bare core session. Loading is read-only;
        // the complete document is written atomically only when saved.
        Session::from_json(&json)
            .map(|session| Some(StudioDocument::new(session, Default::default())))
            .map_err(|e| format!("Cannot load studio: {e}"))
    }
}

/// Serialize and validate before touching the destination; replace with a
/// fully written sibling file. A failed write leaves the previous studio intact.
pub fn save(path: &Path, document: &StudioDocument) -> Result<(), String> {
    let json = serialize(document)?;
    replace(path, &json)
}

/// Preserve an unreadable or unsupported original before the first recovery
/// save replaces the canonical path. A failed backup never permits replacement.
pub fn save_recovering(path: &Path, document: &StudioDocument) -> Result<PathBuf, String> {
    let name = path.file_name().ok_or("Studio path has no filename")?;
    let backup = path.with_file_name(format!(
        "{}.preserved-{}.bak",
        name.to_string_lossy(),
        uuid::Uuid::new_v4()
    ));
    save_recovering_with_backup(path, document, &backup)?;
    Ok(backup)
}

fn save_recovering_with_backup(
    path: &Path,
    document: &StudioDocument,
    backup: &Path,
) -> Result<(), String> {
    let json = serialize(document)?;
    preserve_original(path, backup)?;
    replace(path, &json)
}

fn preserve_original(path: &Path, backup: &Path) -> Result<(), String> {
    let failed = |e| format!("Cannot preserve the original studio; saving is blocked: {e}");
    let mut original = fs::File::open(path).map_err(failed)?;
    if !original.metadata().map_err(failed)?.is_file() {
        return Err(
            "Cannot preserve the original studio; saving is blocked: not a regular file.".into(),
        );
    }
    // create_new makes even an unexpected filename collision non-destructive.
    let mut copy = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(backup)
        .map_err(failed)?;
    let result = (|| -> std::io::Result<()> {
        std::io::copy(&mut original, &mut copy)?;
        copy.sync_all()?;
        #[cfg(unix)]
        fs::File::open(backup.parent().expect("backup is a sibling file"))?.sync_all()?;
        Ok(())
    })();
    drop(copy);
    if result.is_err() {
        let _ = fs::remove_file(backup);
    }
    result.map_err(failed)
}

fn serialize(document: &StudioDocument) -> Result<String, String> {
    if document.studio_schema_version != 1 {
        return Err("Cannot save this studio document version.".into());
    }
    document.session.validate().map_err(|e| e.to_string())?;
    let json = serde_json::to_string_pretty(document).map_err(|e| e.to_string())?;
    if json.len() as u64 > MAX_SESSION_BYTES {
        return Err("Studio exceeds the 32 MiB file safety budget.".into());
    }
    Ok(json)
}

fn replace(path: &Path, json: &str) -> Result<(), String> {
    let parent = path.parent().ok_or("Studio path has no parent folder")?;
    fs::create_dir_all(parent).map_err(|e| format!("Cannot create studio folder: {e}"))?;
    let temporary = parent.join(format!(".relink-{}.tmp", uuid::Uuid::new_v4()));
    let result = (|| -> std::io::Result<()> {
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temporary)?;
        file.write_all(json.as_bytes())?;
        file.sync_all()?;
        drop(file);
        fs::rename(&temporary, path)?;
        #[cfg(unix)]
        fs::File::open(parent)?.sync_all()?;
        Ok(())
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    result.map_err(|e| format!("Cannot save studio: {e}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    struct TestDir(PathBuf);
    impl TestDir {
        fn new() -> Self {
            let path = std::env::temp_dir().join(format!("relink-store-{}", uuid::Uuid::new_v4()));
            fs::create_dir(&path).unwrap();
            Self(path)
        }
    }
    impl Drop for TestDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }
    #[test]
    fn replace_roundtrips_and_invalid_state_preserves_original() {
        let dir = TestDir::new();
        let path = dir.0.join("studio.json");
        assert_eq!(load(&path).unwrap(), None);
        let mut document = StudioDocument::new(Session::new("Studio", 48000), Default::default());
        save(&path, &document).unwrap();
        document.session.name = "Revised studio".into();
        document
            .output_routing
            .outputs
            .push(crate::routing::OutputDraft {
                device_id: Some("unavailable-exact-id".into()),
                bus_id: None,
            });
        save(&path, &document).unwrap();
        assert_eq!(load(&path).unwrap().unwrap(), document);
        let original = fs::read(&path).unwrap();
        document.session.sample_rate = 0;
        assert!(save(&path, &document).is_err());
        assert_eq!(fs::read(&path).unwrap(), original);
        assert_eq!(fs::read_dir(&dir.0).unwrap().count(), 1);
    }
    #[test]
    fn corrupt_and_future_files_are_never_replaced_on_read() {
        let dir = TestDir::new();
        let path = dir.0.join("studio.json");
        for bytes in [
            b"{bad".as_slice(),
            b"{\"schema_version\":999}",
            b"{\"studio_schema_version\":999}",
        ] {
            fs::write(&path, bytes).unwrap();
            assert!(load(&path).is_err());
            assert_eq!(fs::read(&path).unwrap(), bytes);
            assert_eq!(fs::read_dir(&dir.0).unwrap().count(), 1);
        }
    }

    #[test]
    fn recovery_preserves_exact_original_and_reopens_saved_work_at_canonical_path() {
        for original in [
            b"{bad".as_slice(),
            b"{\"studio_schema_version\":999,\"future_data\":\"preserve me\"}",
            b"\xff\xfe not UTF-8",
        ] {
            let dir = TestDir::new();
            let path = dir.0.join("session.json");
            fs::write(&path, original).unwrap();
            assert!(load(&path).is_err());
            assert_eq!(fs::read(&path).unwrap(), original);
            assert_eq!(fs::read_dir(&dir.0).unwrap().count(), 1);
            let mut document =
                StudioDocument::new(Session::new("Recovered", 48_000), Default::default());
            document
                .output_routing
                .outputs
                .push(crate::routing::OutputDraft {
                    device_id: Some("saved-exact-device-id".into()),
                    bus_id: None,
                });
            let backup = save_recovering(&path, &document).unwrap();
            assert_eq!(backup.parent(), path.parent());
            assert_ne!(backup, path);
            assert_eq!(fs::read(&backup).unwrap(), original);
            assert_eq!(load(&path).unwrap().unwrap(), document);
            document.session.name = "Further recovered edits".into();
            save(&path, &document).unwrap();
            assert_eq!(load(&path).unwrap().unwrap(), document);
            assert_eq!(fs::read(&backup).unwrap(), original);
            assert_eq!(fs::read_dir(&dir.0).unwrap().count(), 2);
        }
    }

    #[test]
    fn failed_recovery_backup_never_replaces_original_or_existing_backup() {
        let dir = TestDir::new();
        let path = dir.0.join("session.json");
        let backup = dir.0.join("already-preserved.bak");
        fs::write(&path, b"{original corrupt bytes").unwrap();
        fs::write(&backup, b"previous backup must survive").unwrap();
        let document = StudioDocument::new(Session::new("Recovered", 48_000), Default::default());
        let error = save_recovering_with_backup(&path, &document, &backup).unwrap_err();
        assert!(error.contains("saving is blocked"));
        assert_eq!(fs::read(&path).unwrap(), b"{original corrupt bytes");
        assert_eq!(fs::read(&backup).unwrap(), b"previous backup must survive");
        assert_eq!(fs::read_dir(&dir.0).unwrap().count(), 2);
    }

    #[test]
    fn invalid_recovery_document_does_not_create_backup_or_touch_original() {
        let dir = TestDir::new();
        let path = dir.0.join("session.json");
        fs::write(&path, b"{original corrupt bytes").unwrap();
        let mut document =
            StudioDocument::new(Session::new("Recovered", 48_000), Default::default());
        document.session.sample_rate = 0;
        assert!(save_recovering(&path, &document).is_err());
        assert_eq!(fs::read(&path).unwrap(), b"{original corrupt bytes");
        assert_eq!(fs::read_dir(&dir.0).unwrap().count(), 1);
    }

    #[test]
    fn legacy_read_preserves_bytes_and_atomic_save_migrates_with_routes() {
        let dir = TestDir::new();
        let path = dir.0.join("studio.json");
        let session = Session::new("Legacy", 48_000);
        let original = session.to_json().unwrap();
        fs::write(&path, &original).unwrap();
        let mut document = load(&path).unwrap().unwrap();
        assert_eq!(document.session, session);
        assert_eq!(fs::read(&path).unwrap(), original.as_bytes());
        document.output_routing.outputs.push(Default::default());
        save(&path, &document).unwrap();
        assert_eq!(load(&path).unwrap().unwrap(), document);
        assert_eq!(fs::read_dir(&dir.0).unwrap().count(), 1);
    }
}
