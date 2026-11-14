use std::fs;
use std::path::{Path, PathBuf};

use storage_manager::classify::{Category, classify};

fn mkdir(home: &Path, rel: &str) -> PathBuf {
    let dir = home.join(rel);
    fs::create_dir_all(&dir).unwrap();
    dir
}

fn touch(home: &Path, rel: &str) {
    let file = home.join(rel);
    fs::create_dir_all(file.parent().unwrap()).unwrap();
    fs::write(file, b"x").unwrap();
}

#[test]
fn fixed_cache_paths() {
    let tmp = tempfile::tempdir().unwrap();
    let home = tmp.path();
    for rel in [
        "Library/Caches",
        ".cache",
        ".npm",
        ".yarn/cache",
        "Library/pnpm/store",
        ".cargo/registry",
        ".cargo/git",
        ".gradle/caches",
        ".m2/repository",
        "go/pkg/mod",
    ] {
        assert_eq!(classify(&mkdir(home, rel), home), Some(Category::Cache), "{rel}");
    }
}

#[test]
fn fixed_build_and_temp_paths() {
    let tmp = tempfile::tempdir().unwrap();
    let home = tmp.path();
    let derived = mkdir(home, "Library/Developer/Xcode/DerivedData");
    assert_eq!(classify(&derived, home), Some(Category::Build));
    assert_eq!(classify(&mkdir(home, "Library/Logs"), home), Some(Category::Temp));
    assert_eq!(classify(&mkdir(home, ".Trash"), home), Some(Category::Temp));
}

#[test]
fn marker_rules() {
    for (dir, marker) in [
        ("node_modules", "package.json"),
        (".next", "package.json"),
        ("target", "Cargo.toml"),
        ("target", "pom.xml"),
        ("build", "build.gradle"),
        ("build", "build.gradle.kts"),
        ("Pods", "Podfile"),
    ] {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path();
        let candidate = mkdir(home, &format!("proj/{dir}"));
        assert_eq!(classify(&candidate, home), None, "{dir} without {marker}");
        touch(home, &format!("proj/{marker}"));
        assert_eq!(classify(&candidate, home), Some(Category::Build), "{dir} with {marker}");
    }
}

#[test]
fn venv_needs_pyvenv_cfg_inside() {
    for name in [".venv", "venv"] {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path();
        let venv = mkdir(home, &format!("proj/{name}"));
        assert_eq!(classify(&venv, home), None, "{name} without pyvenv.cfg");
        touch(home, &format!("proj/{name}/pyvenv.cfg"));
        assert_eq!(classify(&venv, home), Some(Category::Build), "{name} with pyvenv.cfg");
    }
}

#[test]
fn pycache_needs_no_marker() {
    let tmp = tempfile::tempdir().unwrap();
    let home = tmp.path();
    assert_eq!(classify(&mkdir(home, "proj/__pycache__"), home), Some(Category::Build));
}

#[test]
fn never_classified() {
    let tmp = tempfile::tempdir().unwrap();
    let home = tmp.path();
    touch(home, "proj/package.json");
    for name in ["dist", "out", "bin", "src"] {
        assert_eq!(classify(&mkdir(home, &format!("proj/{name}")), home), None, "{name}");
    }
    // 고정 경로는 홈 바로 아래의 정확한 경로만 해당한다.
    assert_eq!(classify(&mkdir(home, "proj/.npm"), home), None);
    // 후보의 하위 폴더 자체는 후보가 아니다.
    assert_eq!(classify(&mkdir(home, "Library/Caches/app"), home), None);
    // 표지는 파일이어야 한다.
    mkdir(home, "proj2/package.json");
    assert_eq!(classify(&mkdir(home, "proj2/node_modules"), home), None);
}

#[test]
fn labels() {
    assert_eq!(Category::Cache.label(), "캐시");
    assert_eq!(Category::Build.label(), "빌드 산출물");
    assert_eq!(Category::Temp.label(), "임시 데이터");
    assert!(Category::Cache < Category::Build && Category::Build < Category::Temp);
}
