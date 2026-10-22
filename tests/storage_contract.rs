use clipvault::{
    config::{Config, Paths},
    model::Capture,
    storage::{Store, StoredFile},
};

fn setup() -> (tempfile::TempDir, Store) {
    let d = tempfile::tempdir().unwrap();
    let s = Store::open(Paths::open(d.path().join("vault")).unwrap()).unwrap();
    (d, s)
}
#[test]
fn managed_files_survive_source_deletion_with_original_names() {
    let (d, mut s) = setup();
    let original = d.path().join("important note.txt");
    std::fs::write(&original, "unique searchable content").unwrap();
    let id = s
        .insert(
            Capture::Files(vec![original.clone()]),
            "test",
            &Config::default(),
        )
        .unwrap();
    std::fs::remove_file(original).unwrap();
    let e = s.get(id).unwrap().unwrap();
    let manifest: Vec<StoredFile> = serde_json::from_slice(
        &std::fs::read(s.blob_path(e.blob.as_deref().unwrap()).unwrap()).unwrap(),
    )
    .unwrap();
    assert_eq!(manifest[0].name, "important note.txt");
    let path = s.blob_path(&manifest[0].stored).unwrap();
    assert_eq!(path.file_name().unwrap(), "important note.txt");
    assert_eq!(
        std::fs::read_to_string(path).unwrap(),
        "unique searchable content"
    );
    assert_eq!(s.keyword("searchable", 10).unwrap().len(), 1);
}
#[test]
fn deleted_ids_cannot_receive_inflight_vectors_for_new_items() {
    let (_d, mut s) = setup();
    let c = Config::default();
    let old = s
        .insert(Capture::Text("old private content".into()), "test", &c)
        .unwrap();
    s.clear().unwrap();
    let new = s
        .insert(Capture::Text("new public content".into()), "test", &c)
        .unwrap();
    assert_ne!(old, new);
    s.save_vector(old, "model", &[1.0, 0.0]).unwrap();
    assert!(s.semantic(&[1.0, 0.0], "model", 10).unwrap().is_empty());
}
#[test]
fn pins_survive_limits_and_unpin_allows_cleanup() {
    let (_d, mut s) = setup();
    let c = Config {
        max_entries: 1,
        ..Config::default()
    };
    let a = s
        .insert(Capture::Text("pin me".into()), "test", &c)
        .unwrap();
    s.pin(a, true).unwrap();
    let b = s
        .insert(Capture::Text("discard me".into()), "test", &c)
        .unwrap();
    s.cleanup(&c).unwrap();
    assert!(s.get(a).unwrap().is_some());
    assert!(s.get(b).unwrap().is_some());
    let latest = s
        .insert(Capture::Text("latest".into()), "test", &c)
        .unwrap();
    s.cleanup(&c).unwrap();
    assert!(s.get(a).unwrap().is_some());
    assert!(s.get(b).unwrap().is_none());
    assert!(s.get(latest).unwrap().is_some());
}
#[test]
fn model_changes_do_not_mix_vector_spaces() {
    let (_d, mut s) = setup();
    let id = s
        .insert(Capture::Text("indexed".into()), "test", &Config::default())
        .unwrap();
    s.save_vector(id, "old:model", &[1.0, 0.0]).unwrap();
    s.reindex_mismatched("new:model").unwrap();
    assert_eq!(s.next_pending().unwrap().unwrap().id, id);
    assert!(s.semantic(&[1.0, 0.0], "new:model", 10).unwrap().is_empty());
    s.save_vector(id, "new:model", &[0.0, 1.0]).unwrap();
    assert!(s.semantic(&[1.0, 0.0], "old:model", 10).unwrap().is_empty());
}
#[test]
fn future_schema_and_oversized_items_are_rejected() {
    let (_d, mut s) = setup();
    let c = Config {
        max_item_mb: 1,
        ..Config::default()
    };
    assert!(
        s.insert(Capture::Text("x".repeat(1024 * 1024 + 1)), "test", &c)
            .is_err()
    );
    let paths = s.paths.clone();
    s.conn.execute_batch("PRAGMA user_version=999").unwrap();
    drop(s);
    assert!(Store::open(paths).is_err());
}
#[test]
fn hostile_fts_inputs_are_literals() {
    let (_d, mut s) = setup();
    s.insert(
        Capture::Text("https://example.org/error_code 中文 🚀".into()),
        "test",
        &Config::default(),
    )
    .unwrap();
    for query in [
        "\"",
        "OR NOT NEAR",
        "*",
        "a:b",
        "(((",
        "中文",
        "error_code",
        "%",
        "https://example.org",
    ] {
        assert!(s.keyword(query, 10).is_ok(), "query: {query}");
    }
    assert_eq!(s.keyword("中文", 10).unwrap().len(), 1);
    assert_eq!(s.keyword("error_code", 10).unwrap().len(), 1);
}
#[cfg(unix)]
#[test]
fn symlink_files_are_rejected() {
    let (d, mut s) = setup();
    let file = d.path().join("secret");
    std::fs::write(&file, "secret").unwrap();
    let link = d.path().join("link");
    std::os::unix::fs::symlink(file, &link).unwrap();
    assert!(
        s.insert(Capture::Files(vec![link]), "test", &Config::default())
            .is_err()
    );
}
