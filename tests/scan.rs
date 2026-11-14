use std::fs;
use std::os::unix::fs::{MetadataExt, PermissionsExt, symlink};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::Ordering;

use storage_manager::classify::Category;
use storage_manager::scan::{Progress, is_dataless, scan};
use storage_manager::tree::{Node, NodeKind, Tree};

const KIB: usize = 1024;
const MIB: usize = 1024 * 1024;

/// 임시 폴더와 그 정규화된 경로(`/var`는 `/private/var`의 링크다).
fn temp_root() -> (tempfile::TempDir, PathBuf) {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().canonicalize().unwrap();
    (tmp, root)
}

fn write(root: &Path, rel: &str, len: usize) {
    let file = root.join(rel);
    fs::create_dir_all(file.parent().unwrap()).unwrap();
    fs::write(file, vec![1u8; len]).unwrap();
}

fn scan_ok(root: &Path, home: &Path) -> Tree {
    scan(root, home, &Progress::default()).unwrap()
}

/// `du`가 계산한 실제 디스크 점유량(바이트).
fn du_bytes(dir: &Path) -> u64 {
    let out = Command::new("du").arg("-skx").arg(dir).output().unwrap();
    let text = String::from_utf8(out.stdout).unwrap();
    text.split_whitespace().next().unwrap().parse::<u64>().unwrap() * 1024
}

fn find<'a>(t: &'a Tree, rel: &str) -> &'a Node {
    let id = rel.split('/').fold(Tree::ROOT, |dir, name| {
        t.children_by_size(dir)
            .into_iter()
            .find(|&id| &*t.node(id).name == name)
            .unwrap_or_else(|| panic!("{name} not found in {rel}"))
    });
    t.node(id)
}

#[test]
fn total_matches_du() {
    let (_tmp, root) = temp_root();
    write(&root, "a", MIB);
    write(&root, "sub/b", 2 * MIB);
    write(&root, "sub/deep/c", 4 * KIB);

    let t = scan_ok(&root, &root);

    assert_eq!(t.node(Tree::ROOT).size, du_bytes(&root));
    assert_eq!(find(&t, "a").size, fs::metadata(root.join("a")).unwrap().blocks() * 512);
    assert_eq!(find(&t, "sub").size, du_bytes(&root.join("sub")));
    assert_eq!(t.root_path, root);
    assert_eq!(t.skipped, 0);
}

#[test]
fn kinds_and_names() {
    let (_tmp, root) = temp_root();
    write(&root, "a", 4 * KIB);
    write(&root, "sub/b", 4 * KIB);
    write(&root, ".hidden", 4 * KIB);

    let t = scan_ok(&root, &root);

    let mut names: Vec<String> =
        t.children_by_size(Tree::ROOT).iter().map(|&id| t.node(id).name.to_string()).collect();
    names.sort();
    assert_eq!(names, [".hidden", "a", "sub"]);
    assert_eq!(find(&t, "a").kind, NodeKind::File);
    assert_eq!(find(&t, "sub").kind, NodeKind::Dir);
    assert_eq!(find(&t, "sub/b").kind, NodeKind::File);
}

#[test]
fn symlinks_not_followed() {
    let (_tmp, root) = temp_root();
    write(&root, "a", MIB);
    fs::create_dir(root.join("sub")).unwrap();
    symlink("..", root.join("sub/loop")).unwrap();
    symlink("../a", root.join("sub/link")).unwrap();

    let t = scan_ok(&root, &root);

    assert_eq!(find(&t, "sub/loop").kind, NodeKind::Other);
    assert_eq!(find(&t, "sub/link").kind, NodeKind::Other);
    assert!(t.children_by_size(Tree::ROOT).len() == 2);
    assert_eq!(t.node(Tree::ROOT).size, du_bytes(&root));
}

#[test]
fn hard_link_counted_once() {
    let (_tmp, root) = temp_root();
    write(&root, "x/f", MIB);
    fs::create_dir(root.join("y")).unwrap();
    fs::hard_link(root.join("x/f"), root.join("y/g")).unwrap();

    let t = scan_ok(&root, &root);

    assert!(find(&t, "x/f").size >= MIB as u64);
    assert_eq!(find(&t, "y/g").size, 0);
    assert_eq!(t.node(Tree::ROOT).size, du_bytes(&root));
}

#[test]
fn unreadable_dir_is_skipped() {
    let (_tmp, root) = temp_root();
    write(&root, "locked/secret", 4 * KIB);
    write(&root, "ok/file", MIB);
    let locked = root.join("locked");
    fs::set_permissions(&locked, fs::Permissions::from_mode(0o000)).unwrap();

    let result = scan(&root, &root, &Progress::default());
    fs::set_permissions(&locked, fs::Permissions::from_mode(0o755)).unwrap();

    let t = result.unwrap();
    assert_eq!(t.skipped, 1);
    assert_eq!(find(&t, "locked").kind, NodeKind::Dir);
    assert!(t.children_by_size(Tree::ROOT).len() == 2);
    assert!(find(&t, "ok/file").size >= MIB as u64);
}

#[test]
fn candidates_found_once() {
    let (_tmp, root) = temp_root();
    write(&root, "proj/package.json", 1);
    write(&root, "proj/node_modules/pkg/package.json", 1);
    write(&root, "proj/node_modules/pkg/node_modules/x", 4 * KIB);
    write(&root, "Library/Caches/app/blob", 4 * KIB);

    let t = scan_ok(&root, &root);

    let found: Vec<(PathBuf, Option<Category>)> =
        t.candidates().iter().map(|&id| (t.path(id), t.node(id).category)).collect();
    assert_eq!(
        found,
        [
            (root.join("Library/Caches"), Some(Category::Cache)),
            (root.join("proj/node_modules"), Some(Category::Build)),
        ]
    );
}

#[test]
fn root_is_never_classified() {
    let (_tmp, home) = temp_root();
    write(&home, "proj/Cargo.toml", 1);
    write(&home, "proj/target/out", 4 * KIB);

    let t = scan_ok(&home.join("proj/target"), &home);

    assert_eq!(t.node(Tree::ROOT).category, None);
    assert!(t.candidates().is_empty());
}

#[test]
fn empty_root() {
    let (_tmp, root) = temp_root();

    let t = scan_ok(&root, &root);

    assert!(t.children_by_size(Tree::ROOT).is_empty());
    assert_eq!(t.node(Tree::ROOT).size, du_bytes(&root));
}

#[test]
fn progress_is_reported() {
    let (_tmp, root) = temp_root();
    write(&root, "a", 4 * KIB);
    write(&root, "sub/b", 4 * KIB);
    let progress = Progress::default();

    scan(&root, &root, &progress).unwrap();

    assert_eq!(progress.files.load(Ordering::Relaxed), 3);
    assert!(progress.bytes.load(Ordering::Relaxed) >= 8 * KIB as u64);
}

#[test]
fn missing_root_is_err() {
    let (_tmp, root) = temp_root();
    assert!(scan(&root.join("nope"), &root, &Progress::default()).is_err());
}

#[test]
fn dataless_flag() {
    assert!(is_dataless(0x4000_0000));
    assert!(is_dataless(0x4000_0020));
    assert!(!is_dataless(0));
    assert!(!is_dataless(0x20));
}

/// 실제 폴더로 `du`와 비교한다. 클라우드 폴더가 없는 개발용 폴더로 실행한다.
/// `SM_REAL_DIR=$HOME/Github cargo test --release --test scan real_dir -- --ignored --nocapture`
#[test]
#[ignore = "needs SM_REAL_DIR"]
fn real_dir_matches_du() {
    let dir = PathBuf::from(std::env::var_os("SM_REAL_DIR").expect("SM_REAL_DIR is not set"))
        .canonicalize()
        .unwrap();
    let home = PathBuf::from(std::env::var_os("HOME").unwrap());

    let started = std::time::Instant::now();
    let t = scan_ok(&dir, &home);
    let elapsed = started.elapsed();

    let ours = t.node(Tree::ROOT).size;
    let du = du_bytes(&dir);
    println!(
        "nodes: {}, skipped: {}, ours: {ours}, du: {du}, candidates: {}, elapsed: {elapsed:?}",
        t.node_count(),
        t.skipped,
        t.candidates().len(),
    );
    assert!(ours.abs_diff(du) <= du / 100, "ours {ours} vs du {du}");
}
