//! 읽기 전용 제약: 제품 코드(`src/`)에는 디스크를 바꾸거나 외부 프로세스를
//! 실행하는 호출이 없어야 한다.

use std::fs;
use std::path::{Path, PathBuf};

const FORBIDDEN: &[&str] = &[
    "fs::write",
    "File::create",
    "File::options",
    "OpenOptions",
    "remove_file",
    "remove_dir",
    "fs::rename",
    "create_dir",
    "set_permissions",
    "fs::copy",
    "hard_link",
    "symlink(",
    "set_len",
    "process::Command",
    "Command::new",
];

fn rust_files(dir: &Path, found: &mut Vec<PathBuf>) {
    for entry in fs::read_dir(dir).unwrap() {
        let path = entry.unwrap().path();
        if path.is_dir() {
            rust_files(&path, found);
        } else if path.extension().is_some_and(|ext| ext == "rs") {
            found.push(path);
        }
    }
}

#[test]
fn product_code_never_modifies_the_disk() {
    let mut files = Vec::new();
    rust_files(&Path::new(env!("CARGO_MANIFEST_DIR")).join("src"), &mut files);
    assert!(files.len() >= 7, "expected to find the source files, found {files:?}");

    let mut violations = Vec::new();
    for file in &files {
        let source = fs::read_to_string(file).unwrap();
        for word in FORBIDDEN {
            if source.contains(word) {
                violations.push(format!("{}: {word}", file.display()));
            }
        }
    }
    assert!(violations.is_empty(), "disk-modifying calls in product code:\n{}", violations.join("\n"));
}
