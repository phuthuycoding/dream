//! Persistence contract tests for the session registry.
//!
//! Behaviour covered (all through the public `Registry` surface):
//!   - create + save + load round-trips every session field, with
//!     `session.json` written inside the per-session state dir
//!   - the on-disk schema carries the documented fields and no stray temp
//!     files survive an atomic write
//!   - a corrupt `session.json` is quarantined (moved aside or removed)
//!     while every other session still loads; a state dir with no
//!     `session.json` is skipped; startup never panics
//!   - `mark_dead` flips the status to `Dead`, keeps the session listed,
//!     and persists across reload
//!   - `update` persists rename / cwd / setup_script changes across reload
//!   - `delete` removes the whole per-session state dir and drops the entry

#![cfg(test)]

use std::fs;
use std::path::PathBuf;

use tempfile::TempDir;

use super::registry::Registry;
use crate::model::Status;

fn state_dir(data_dir: &std::path::Path, id: &str) -> PathBuf {
    data_dir.join("sessions").join(id)
}

#[test]
fn create_save_and_reload_preserves_all_session_fields() {
    let data = TempDir::new().expect("tempdir");
    let cwd = TempDir::new().expect("cwd tempdir");

    let mut reg = Registry::load(data.path()).expect("loading an empty data dir must succeed");
    assert!(reg.list().is_empty(), "a fresh data dir holds no sessions");
    assert!(
        reg.get("no-such-id").is_none(),
        "unknown ids return nothing"
    );

    let script = "export X=1; echo SETUP-DONE".to_string();
    let created = reg
        .create("alpha", "zsh -l", cwd.path(), Some(script.clone()))
        .expect("create session");
    let plain = reg
        .create("beta", "bash", cwd.path(), None)
        .expect("create session without a setup script");
    let id = created.id.to_string();
    let plain_id = plain.id.to_string();

    assert_eq!(reg.list().len(), 2);
    assert!(
        state_dir(data.path(), &id).join("session.json").exists(),
        "session.json must be written into the per-session state dir"
    );

    let reloaded = Registry::load(data.path()).expect("reload must succeed");
    assert_eq!(reloaded.list().len(), 2);

    let s = reloaded
        .get(&id)
        .expect("the created session must survive a reload");
    assert_eq!(s.id.to_string(), id);
    assert_eq!(s.name, "alpha");
    assert_eq!(s.cmd, "zsh -l");
    assert_eq!(
        PathBuf::from(&s.cwd)
            .canonicalize()
            .expect("canonicalize stored cwd"),
        cwd.path().canonicalize().expect("canonicalize cwd"),
        "cwd must round-trip"
    );
    assert_eq!(s.setup_script.as_deref(), Some(script.as_str()));
    assert_eq!(s.status, Status::Live, "a fresh session starts live");
    assert_eq!(
        s.created_at, created.created_at,
        "created_at must round-trip"
    );
    assert_eq!(
        PathBuf::from(&s.log_path),
        state_dir(data.path(), &id).join("replay.log"),
        "log path points at the session's replay.log"
    );
    assert!(
        !s.resurrected,
        "a freshly created session is not resurrected"
    );

    let s2 = reloaded
        .get(&plain_id)
        .expect("second session must survive a reload");
    assert_eq!(s2.name, "beta");
    assert!(
        s2.setup_script.is_none(),
        "absent setup script stays absent"
    );
}

#[test]
fn session_json_on_disk_uses_the_documented_schema_without_temp_leftovers() {
    let data = TempDir::new().expect("tempdir");
    let mut reg = Registry::load(data.path()).expect("load");
    let created = reg
        .create("schema-check", "zsh", data.path(), Some("true".to_string()))
        .expect("create");
    let dir = state_dir(data.path(), &created.id.to_string());

    let raw = fs::read_to_string(dir.join("session.json")).expect("session.json must be readable");
    let doc: serde_json::Value =
        serde_json::from_str(&raw).expect("session.json must be valid JSON");
    for key in [
        "id",
        "name",
        "cmd",
        "cwd",
        "setup_script",
        "status",
        "created_at",
        "log_path",
        "resurrected",
    ] {
        assert!(
            doc.get(key).is_some(),
            "session.json is missing field `{key}`"
        );
    }
    assert_eq!(
        doc["id"].as_str().expect("id is a string"),
        created.id.to_string()
    );
    assert_eq!(doc["name"].as_str(), Some("schema-check"));

    let entries: Vec<String> = fs::read_dir(&dir)
        .expect("state dir readable")
        .map(|e| {
            e.expect("dir entry")
                .file_name()
                .to_string_lossy()
                .into_owned()
        })
        .collect();
    assert!(
        entries
            .iter()
            .all(|n| !n.contains("tmp") && !n.starts_with('.')),
        "atomic writes must not leave temp files behind: {entries:?}"
    );
}

#[test]
fn corrupt_session_json_is_quarantined_and_valid_sessions_still_load() {
    let data = TempDir::new().expect("tempdir");
    let mut reg = Registry::load(data.path()).expect("load");
    let good = reg
        .create("good", "zsh", data.path(), None)
        .expect("create");
    let other = reg
        .create("also-good", "bash", data.path(), None)
        .expect("create");

    let corrupt = state_dir(data.path(), "corrupt-session");
    fs::create_dir_all(&corrupt).expect("seed corrupt dir");
    fs::write(corrupt.join("session.json"), b"{ this is not json !!!").expect("seed corrupt file");

    let malformed = state_dir(data.path(), "malformed-session");
    fs::create_dir_all(&malformed).expect("seed malformed dir");
    fs::write(malformed.join("session.json"), br#"{"unexpected": true}"#)
        .expect("seed malformed file");

    let reloaded = Registry::load(data.path())
        .expect("corrupt session.json files must never crash startup load");
    let ids: Vec<String> = reloaded.list().iter().map(|s| s.id.to_string()).collect();
    assert_eq!(reloaded.list().len(), 2, "only the valid sessions load");
    assert!(ids.contains(&good.id.to_string()));
    assert!(ids.contains(&other.id.to_string()));
    assert!(reloaded.get("corrupt-session").is_none());
    assert!(reloaded.get("malformed-session").is_none());

    for dir in [&corrupt, &malformed] {
        let has_live_file = dir.join("session.json").exists();
        let has_quarantined_copy = fs::read_dir(dir)
            .expect("state dir readable")
            .filter_map(|e| e.ok())
            .any(|e| {
                let name = e.file_name().to_string_lossy().into_owned();
                name != "session.json" && name.starts_with("session.json")
            });
        assert!(
            !has_live_file || has_quarantined_copy,
            "a corrupt session.json must be quarantined — renamed aside or removed: {}",
            dir.display()
        );
    }

    let rereg = Registry::load(data.path()).expect("repeated loads stay healthy");
    assert_eq!(rereg.list().len(), 2);
}

#[test]
fn state_dir_without_session_json_is_skipped_on_load() {
    let data = TempDir::new().expect("tempdir");
    let mut reg = Registry::load(data.path()).expect("load");
    reg.create("kept", "zsh", data.path(), None)
        .expect("create");

    let orphan = state_dir(data.path(), "orphan-dir");
    fs::create_dir_all(&orphan).expect("seed orphan dir");
    fs::write(orphan.join("replay.log"), b"").expect("seed stray log");

    let reloaded =
        Registry::load(data.path()).expect("a state dir without session.json must be skipped");
    assert_eq!(reloaded.list().len(), 1);
    assert_eq!(reloaded.list()[0].name, "kept");
}

#[test]
fn mark_dead_flips_status_to_dead_and_keeps_the_session_listed() {
    let data = TempDir::new().expect("tempdir");
    let mut reg = Registry::load(data.path()).expect("load");
    let s = reg
        .create("worker", "sleep 0.1", data.path(), None)
        .expect("create");
    let id = s.id.to_string();

    reg.mark_dead(&id).expect("mark_dead");

    let dead = reg.get(&id).expect("a dead session stays in the registry");
    assert_eq!(dead.status, Status::Dead);
    assert_eq!(reg.list().len(), 1, "dead sessions remain listed");

    let reloaded = Registry::load(data.path()).expect("reload");
    let persisted = reloaded
        .get(&id)
        .expect("dead status must persist across reload");
    assert_eq!(persisted.status, Status::Dead);
    assert_eq!(reloaded.list().len(), 1);
}

#[test]
fn update_persists_name_cwd_and_setup_script_across_reload() {
    let data = TempDir::new().expect("tempdir");
    let old_cwd = TempDir::new().expect("old cwd");
    let new_cwd = TempDir::new().expect("new cwd");
    let mut reg = Registry::load(data.path()).expect("load");
    let s = reg
        .create("old-name", "bash", old_cwd.path(), None)
        .expect("create");
    let id = s.id.to_string();

    reg.update(&id, Some("renamed".to_string()), None, None)
        .expect("rename-only update");
    {
        let cur = reg.get(&id).expect("session after rename");
        assert_eq!(cur.name, "renamed");
        assert_eq!(
            PathBuf::from(&cur.cwd)
                .canonicalize()
                .expect("canonicalize stored cwd"),
            old_cwd.path().canonicalize().expect("canonicalize cwd"),
            "fields not updated must be left alone"
        );
        assert!(cur.setup_script.is_none());
    }

    reg.update(
        &id,
        Some("renamed-again".to_string()),
        Some(new_cwd.path().to_path_buf()),
        Some("echo edited".to_string()),
    )
    .expect("full update");

    let reloaded = Registry::load(data.path()).expect("reload");
    let s = reloaded.get(&id).expect("session after update + reload");
    assert_eq!(s.name, "renamed-again");
    assert_eq!(
        PathBuf::from(&s.cwd)
            .canonicalize()
            .expect("canonicalize stored cwd"),
        new_cwd.path().canonicalize().expect("canonicalize cwd"),
    );
    assert_eq!(s.setup_script.as_deref(), Some("echo edited"));
    assert_eq!(
        s.cmd, "bash",
        "cmd is not part of update and must be untouched"
    );
}

#[test]
fn delete_removes_the_whole_state_dir_and_drops_the_entry() {
    let data = TempDir::new().expect("tempdir");
    let mut reg = Registry::load(data.path()).expect("load");
    let keep = reg
        .create("keep", "zsh", data.path(), None)
        .expect("create");
    let doomed = reg
        .create("doomed", "bash", data.path(), None)
        .expect("create");
    let doomed_id = doomed.id.to_string();
    let dir = state_dir(data.path(), &doomed_id);
    fs::write(dir.join("replay.log"), b"recorded output").expect("seed log");

    reg.delete(&doomed_id).expect("delete");

    assert!(
        !dir.exists(),
        "delete must remove the per-session state dir, including the replay log"
    );
    assert!(reg.get(&doomed_id).is_none());
    assert_eq!(reg.list().len(), 1);
    assert_eq!(reg.list()[0].id.to_string(), keep.id.to_string());

    let reloaded = Registry::load(data.path()).expect("reload");
    assert_eq!(reloaded.list().len(), 1, "a deleted session stays deleted");
    assert!(reloaded.get(&doomed_id).is_none());
}
