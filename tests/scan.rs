use std::fs;
use std::os::unix::fs::{MetadataExt, PermissionsExt, symlink};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::Ordering;

use storage_analyzer::classify::Category;
use storage_analyzer::scan::{Progress, is_dataless, scan};
use storage_analyzer::tree::{Node, NodeKind, Tree};

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
fn hard_link_is_credited_to_the_same_path_every_time() {
    let (_tmp, root) = temp_root();
    // 폴더 단위로 깊이 우선이므로 `a/z`가 `a/b/c`보다 먼저다.
    write(&root, "a/z", MIB);
    fs::create_dir(root.join("a/b")).unwrap();
    fs::hard_link(root.join("a/z"), root.join("a/b/c")).unwrap();
    // 여러 폴더에 흩어진 링크는 이름순으로 첫 폴더에 계산된다.
    write(&root, "d00/f", MIB);
    for i in 1..20 {
        fs::create_dir(root.join(format!("d{i:02}"))).unwrap();
        fs::hard_link(root.join("d00/f"), root.join(format!("d{i:02}/f"))).unwrap();
    }

    for _ in 0..10 {
        let t = scan_ok(&root, &root);
        assert!(find(&t, "a/z").size >= MIB as u64);
        assert_eq!(find(&t, "a/b/c").size, 0);
        assert!(find(&t, "d00/f").size >= MIB as u64);
        for i in 1..20 {
            assert_eq!(find(&t, &format!("d{i:02}/f")).size, 0);
        }
        assert_eq!(t.node(Tree::ROOT).size, du_bytes(&root));
    }
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

fn timestamps(path: &Path) -> (i64, i64, i64) {
    let meta = fs::symlink_metadata(path).unwrap();
    (meta.atime(), meta.mtime(), meta.ctime())
}

/// 폴더 목록을 읽기만 해도 macOS는 접근 시각을 디스크에 기록한다. 스캔은 그것도 하지 않아야 한다.
#[test]
fn scan_leaves_timestamps_untouched() {
    let (_tmp, root) = temp_root();
    write(&root, "a", 4 * KIB);
    write(&root, "sub/deep/file", 4 * KIB);
    let paths = [
        root.clone(),
        root.join("a"),
        root.join("sub"),
        root.join("sub/deep"),
        root.join("sub/deep/file"),
    ];
    let touched = Command::new("touch").args(["-a", "-t", "202001010000"]).args(&paths).status().unwrap();
    assert!(touched.success());
    let before: Vec<_> = paths.iter().map(|path| timestamps(path)).collect();
    assert!(before.iter().all(|&(atime, _, _)| atime < 1_600_000_000), "touch did not set atime");

    scan_ok(&root, &root);

    let after: Vec<_> = paths.iter().map(|path| timestamps(path)).collect();
    assert_eq!(before, after);
}

/// 스캔은 클라우드 항목 내려받기와 접근 시각 기록을 프로세스 단위로 꺼야 한다.
#[test]
fn scan_turns_off_writes_caused_by_reading() {
    unsafe extern "C" {
        fn getiopolicy_np(iotype: i32, scope: i32) -> i32;
    }
    const PROCESS: i32 = 0;
    const ATIME_UPDATES: i32 = 2;
    const MATERIALIZE_DATALESS_FILES: i32 = 3;
    const OFF: i32 = 1;
    let (_tmp, root) = temp_root();

    scan_ok(&root, &root);

    // SAFETY: 정수 인자만 받고 현재 설정값을 돌려주는 libSystem 함수다.
    unsafe {
        assert_eq!(getiopolicy_np(ATIME_UPDATES, PROCESS), OFF);
        assert_eq!(getiopolicy_np(MATERIALIZE_DATALESS_FILES, PROCESS), OFF);
    }
}

/// 목록은 읽히지만 항목을 조회할 수 없는 폴더(읽기만 가능, 진입 불가)도 건너뛴 것으로 센다.
#[test]
fn unsearchable_dir_is_counted_as_skipped() {
    let (_tmp, root) = temp_root();
    write(&root, "listed/secret", MIB);
    let listed = root.join("listed");
    fs::set_permissions(&listed, fs::Permissions::from_mode(0o444)).unwrap();

    let result = scan(&root, &root, &Progress::default());
    fs::set_permissions(&listed, fs::Permissions::from_mode(0o755)).unwrap();

    assert_eq!(result.unwrap().skipped, 1);
}
