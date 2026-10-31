//! Clearing a saved removal goes through rex-kiosk's real pending-delete state, which is
//! found through the environment. This binary holds a single test, so setting the
//! environment cannot race another test.

use std::{fs, path::PathBuf};

use kiosk_core::pending_delete::{
    PendingWorktreeDelete, load_pending_worktree_deletes, save_pending_worktree_deletes,
};
use rex_kiosk::{PRODUCT, rex::backend::forget_saved_removal};

#[test]
fn forgetting_a_removal_drops_only_its_record_and_refuses_a_broken_file() {
    let temp = tempfile::tempdir().unwrap();
    // SAFETY: this is the only test in its binary, so no other thread reads the
    // environment while it changes.
    unsafe { std::env::set_var("XDG_STATE_HOME", temp.path()) };
    let entry = |name: &str| {
        PendingWorktreeDelete::new(
            PathBuf::from("/repo"),
            name.into(),
            PathBuf::from(format!("/worktrees/{name}")),
        )
    };
    save_pending_worktree_deletes(&PRODUCT, &[entry("gone"), entry("kept")]).unwrap();

    forget_saved_removal(&PathBuf::from("/worktrees/gone")).unwrap();
    forget_saved_removal(&PathBuf::from("/worktrees/never-saved")).unwrap();

    let loaded = load_pending_worktree_deletes(&PRODUCT);
    assert!(loaded.warnings.is_empty(), "{:?}", loaded.warnings);
    let names: Vec<_> = loaded
        .entries
        .iter()
        .map(|entry| entry.branch_name.as_str())
        .collect();
    assert_eq!(names, ["kept"]);

    let file = temp.path().join("rex-kiosk/pending_deletes.toml");
    fs::write(&file, "not toml [").unwrap();
    let error = forget_saved_removal(&PathBuf::from("/worktrees/kept")).unwrap_err();
    assert!(error.contains("malformed"), "{error}");
    assert!(
        !file.exists(),
        "kiosk-core moves a broken file aside, and nothing replaces it"
    );
}
