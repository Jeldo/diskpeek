//! 실행 파일의 인자 오류 처리. 모두 터미널 화면을 띄우기 전에 끝나야 한다.

use std::process::{Command, Output};

fn run(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_diskpeek")).args(args).output().unwrap()
}

fn stderr(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).into_owned()
}

#[test]
fn missing_path_exits_1() {
    let output = run(&["/no/such/dir"]);
    assert_eq!(output.status.code(), Some(1));
    assert!(stderr(&output).contains("경로를 찾을 수 없습니다: /no/such/dir"), "{}", stderr(&output));
    assert!(output.stdout.is_empty());
}

#[test]
fn file_path_exits_1() {
    let file = tempfile::NamedTempFile::new().unwrap();
    let path = file.path().to_str().unwrap();
    let output = run(&[path]);
    assert_eq!(output.status.code(), Some(1));
    assert!(stderr(&output).contains("폴더가 아닙니다"), "{}", stderr(&output));
    assert!(output.stdout.is_empty());
}

#[test]
fn too_many_args_exits_2() {
    let output = run(&["/tmp", "/tmp"]);
    assert_eq!(output.status.code(), Some(2));
    assert!(stderr(&output).contains("사용법: diskpeek [경로]"), "{}", stderr(&output));
    assert!(output.stdout.is_empty());
}

#[test]
fn missing_home_exits_1() {
    let output = Command::new(env!("CARGO_BIN_EXE_diskpeek")).env_remove("HOME").output().unwrap();
    assert_eq!(output.status.code(), Some(1));
    assert!(stderr(&output).contains("HOME 환경 변수가 없습니다"), "{}", stderr(&output));
    assert!(output.stdout.is_empty());
}
