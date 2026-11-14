use std::ffi::OsString;
use std::io;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::thread;
use std::time::Duration;

use ratatui::DefaultTerminal;
use ratatui::crossterm::event::{self, Event, KeyEventKind};

use storage_analyzer::app::{Action, App};
use storage_analyzer::format::display_path;
use storage_analyzer::scan::{self, Progress};
use storage_analyzer::tree::Tree;
use storage_analyzer::ui;

fn main() -> ExitCode {
    // 여기서 끝나는 오류는 터미널 화면을 바꾸기 전에 처리한다.
    // 경로를 확인하는 것도 디스크를 읽는 일이므로 읽기 전용 설정을 가장 먼저 적용한다.
    if let Err(err) = scan::set_read_only_io_policy() {
        eprintln!("읽기 전용 설정을 적용할 수 없습니다: {err}");
        return ExitCode::FAILURE;
    }
    let args: Vec<OsString> = std::env::args_os().skip(1).collect();
    if args.len() > 1 {
        eprintln!("사용법: storage-analyzer [경로]");
        return ExitCode::from(2);
    }
    let Some(home) = std::env::var_os("HOME").map(PathBuf::from) else {
        eprintln!("HOME 환경 변수가 없습니다");
        return ExitCode::FAILURE;
    };
    let home = home.canonicalize().unwrap_or(home);
    let requested = args.into_iter().next().map(PathBuf::from).unwrap_or_else(|| home.clone());
    let Ok(root) = requested.canonicalize() else {
        eprintln!("경로를 찾을 수 없습니다: {}", requested.display());
        return ExitCode::FAILURE;
    };
    if !root.is_dir() {
        eprintln!("폴더가 아닙니다: {}", requested.display());
        return ExitCode::FAILURE;
    }

    // `init`은 panic이 나도 터미널을 복구하는 훅을 함께 설치한다.
    let mut terminal = ratatui::init();
    let result = run(&mut terminal, &root, home);
    ratatui::restore();

    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(err) => {
            eprintln!("{err}");
            ExitCode::FAILURE
        }
    }
}

fn run(terminal: &mut DefaultTerminal, root: &Path, home: PathBuf) -> io::Result<()> {
    let Some(tree) = scan_with_progress(terminal, root, &home)? else {
        return Ok(());
    };
    let mut app = App::new(tree, home);
    while !app.should_quit {
        terminal.draw(|frame| ui::draw(frame, &app))?;
        if let Some(action) = read_action()? {
            app.handle(action);
        }
        if app.wants_rescan {
            let Some(tree) = scan_with_progress(terminal, root, &app.home)? else {
                return Ok(());
            };
            app.replace_tree(tree);
        }
    }
    Ok(())
}

/// 다른 스레드에서 스캔하면서 진행 화면을 그린다.
/// 사용자가 스캔 도중에 종료하면 `None`을 돌려준다.
fn scan_with_progress(
    terminal: &mut DefaultTerminal,
    root: &Path,
    home: &Path,
) -> io::Result<Option<Tree>> {
    let progress = Arc::new(Progress::default());
    let scanner = {
        let (root, home, progress) = (root.to_path_buf(), home.to_path_buf(), Arc::clone(&progress));
        thread::spawn(move || scan::scan(&root, &home, &progress))
    };

    let shown_root = display_path(root, home);
    while !scanner.is_finished() {
        let files = progress.files.load(Ordering::Relaxed);
        let bytes = progress.bytes.load(Ordering::Relaxed);
        terminal.draw(|frame| ui::draw_progress(frame, &shown_root, files, bytes))?;
        if event::poll(Duration::from_millis(100))? && read_action()? == Some(Action::Quit) {
            // 스캔 스레드는 프로세스가 끝날 때 함께 끝난다.
            return Ok(None);
        }
    }

    match scanner.join() {
        Ok(Ok(tree)) => Ok(Some(tree)),
        Ok(Err(err)) => Err(io::Error::new(err.kind(), format!("스캔할 수 없습니다: {err}"))),
        Err(_) => Err(io::Error::other("스캔할 수 없습니다: 스캔 중 내부 오류가 발생했습니다")),
    }
}

/// 이벤트 하나를 읽어서, 키를 누른 것이면 대응하는 동작을 돌려준다.
fn read_action() -> io::Result<Option<Action>> {
    match event::read()? {
        Event::Key(key) if key.kind == KeyEventKind::Press => Ok(ui::action_for(key)),
        _ => Ok(None),
    }
}
