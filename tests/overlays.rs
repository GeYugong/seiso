use std::collections::BTreeMap;
use std::path::Path;

use seiso::analysis;
use seiso::workspace::{self, LoadOptions, LoadScope};

fn load(root: &Path, options: &LoadOptions, buffers: &[(&str, &str)]) -> workspace::Snapshot {
    let overlays = buffers
        .iter()
        .map(|(name, text)| (root.join(name), (*text).to_owned()))
        .collect();
    workspace::load_with_overlays(root, options, LoadScope::Check, &overlays).unwrap()
}

#[test]
fn multiple_overlays_match_the_same_workspace_saved_to_disk() {
    let root = tempfile::tempdir().unwrap();
    std::fs::write(
        root.path().join("seiso.toml"),
        "preview = true\n[lint]\nselect = ['LNK001','LNK002']\n",
    )
    .unwrap();
    let buffers = [
        ("source.md", "[Target](new/target.md#missing)\n"),
        ("new/target.md", "# Heading\n"),
    ];
    let options = LoadOptions {
        no_cache: true,
        ..LoadOptions::default()
    };
    let overlay =
        analysis::check(load(root.path(), &options, &buffers), &options.overrides).unwrap();
    for (name, text) in buffers {
        let path = root.path().join(name);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, text).unwrap();
    }
    let disk = analysis::check(
        workspace::load(root.path(), &options, LoadScope::Check).unwrap(),
        &options.overrides,
    )
    .unwrap();
    assert_eq!(overlay.diagnostics, disk.diagnostics);
    assert_eq!(overlay.snapshot.errors, disk.snapshot.errors);
    assert_eq!(overlay.diagnostics.len(), 1);
    assert_eq!(overlay.diagnostics[0].code, "LNK002");
}

#[test]
fn overlays_are_dependencies_without_implicitly_selecting_them() {
    let root = tempfile::tempdir().unwrap();
    std::fs::write(
        root.path().join("seiso.toml"),
        "preview = true\n[lint]\nselect = ['KND001','LNK002']\n",
    )
    .unwrap();
    let options = LoadOptions {
        paths: vec!["source.md".into()],
        no_cache: true,
        ..LoadOptions::default()
    };
    let snapshot = load(
        root.path(),
        &options,
        &[
            (
                "source.md",
                "---\nkind: howto\n---\n[Target](target.md#heading)\n",
            ),
            ("target.md", "# Heading"),
        ],
    );
    assert_eq!(
        snapshot.selected.into_iter().collect::<Vec<_>>(),
        ["source.md"]
    );
    assert_eq!(snapshot.index.files.len(), 2);
    assert!(snapshot.errors.is_empty());
}

#[test]
fn stdin_remains_strict_and_takes_precedence_over_editor_buffers() {
    let root = tempfile::tempdir().unwrap();
    std::fs::write(
        root.path().join("seiso.toml"),
        "exclude = ['excluded.md']\n",
    )
    .unwrap();
    let options = LoadOptions {
        stdin: Some(("source.md".into(), "# Stdin".into())),
        no_cache: true,
        ..LoadOptions::default()
    };
    let snapshot = load(root.path(), &options, &[("source.md", "# Editor")]);
    assert_eq!(
        snapshot.index.file("source.md").unwrap().document.source,
        "# Stdin"
    );
    let excluded = LoadOptions {
        stdin: Some(("excluded.md".into(), "# Stdin".into())),
        ..LoadOptions::default()
    };
    let snapshot = load(root.path(), &excluded, &[("excluded.md", "# Editor")]);
    assert!(
        snapshot
            .errors
            .iter()
            .any(|error| error.message.contains("stdin filename is excluded"))
    );
}

#[test]
fn outside_workspace_overlays_are_rejected() {
    let root = tempfile::tempdir().unwrap();
    let outside = tempfile::tempdir().unwrap();
    std::fs::write(root.path().join("seiso.toml"), "").unwrap();
    let overlays = BTreeMap::from([(outside.path().join("outside.md"), "# Outside".into())]);
    assert!(
        workspace::load_with_overlays(
            root.path(),
            &LoadOptions::default(),
            LoadScope::Check,
            &overlays
        )
        .err()
        .unwrap()
        .contains("outside workspace")
    );
}

#[test]
fn a_new_directory_can_select_its_unsaved_descendants() {
    let root = tempfile::tempdir().unwrap();
    std::fs::write(
        root.path().join("seiso.toml"),
        "[lint]\nselect = ['KND001']\n",
    )
    .unwrap();
    let options = LoadOptions {
        paths: vec!["new".into()],
        no_cache: true,
        ..LoadOptions::default()
    };
    let snapshot = load(
        root.path(),
        &options,
        &[("new/doc.md", "# New"), ("other.md", "# Other")],
    );
    assert!(snapshot.errors.is_empty());
    assert_eq!(
        snapshot.selected.into_iter().collect::<Vec<_>>(),
        ["new/doc.md"]
    );
    assert_eq!(snapshot.index.files.len(), 1);
    assert!(!root.path().join("new").exists());
}
