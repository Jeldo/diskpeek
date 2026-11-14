//! `App`을 화면에 그리고, 키 입력을 `Action`으로 바꾼다.

use ratatui::Frame;
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{List, ListState, Paragraph};

use crate::app::{Action, App, Row, View};
use crate::classify::Category;
use crate::format::{bar, display_path, format_size};
use crate::tree::{NodeId, NodeKind};

const TITLE: &str = " storage-analyzer  ";

/// 화면 아래에 보여주는 (키, 설명) 안내. 가장 넓은 것이 80칸에 들어가야 한다.
type Keys = &'static [(&'static str, &'static str)];
const BROWSE_KEYS: Keys = &[
    ("↑↓", "이동"),
    ("Enter", "들어가기"),
    ("Esc", "상위"),
    ("c", "정리 후보"),
    ("f", "큰 파일"),
    ("r", "다시 스캔"),
    ("q", "종료"),
];
const COLLECTED_KEYS: Keys =
    &[("↑↓", "이동"), ("Enter", "해당 위치로 이동"), ("Esc", "돌아가기"), ("r", "다시 스캔"), ("q", "종료")];
const PROGRESS_KEYS: Keys = &[("q", "종료")];

/// 크기 칸의 폭. "999.9 GB"보다 넉넉하게 잡는다.
const SIZE_WIDTH: usize = 10;

pub fn action_for(key: KeyEvent) -> Option<Action> {
    // Ctrl+C를 먼저 검사해야 `c`(정리 후보)로 처리되지 않는다.
    if key.modifiers.contains(KeyModifiers::CONTROL) {
        return (key.code == KeyCode::Char('c')).then_some(Action::Quit);
    }
    // 글자 키는 대문자(Caps Lock, Shift)와, 한글 입력 상태에서 같은 자리의
    // 키를 눌렀을 때 들어오는 두벌식 자모도 함께 받는다.
    match key.code {
        KeyCode::Up | KeyCode::Char('k' | 'K' | 'ㅏ') => Some(Action::Up),
        KeyCode::Down | KeyCode::Char('j' | 'J' | 'ㅓ') => Some(Action::Down),
        KeyCode::Enter | KeyCode::Right => Some(Action::Enter),
        KeyCode::Esc | KeyCode::Left | KeyCode::Backspace => Some(Action::Back),
        KeyCode::Char('c' | 'C' | 'ㅊ') => Some(Action::ToggleCandidates),
        KeyCode::Char('f' | 'F' | 'ㄹ') => Some(Action::ToggleLargeFiles),
        KeyCode::Char('r' | 'R' | 'ㄱ') => Some(Action::Rescan),
        KeyCode::Char('q' | 'Q' | 'ㅂ') => Some(Action::Quit),
        _ => None,
    }
}

pub fn draw(frame: &mut Frame, app: &App) {
    let [header, top_rule, body, bottom_rule, footer] = Layout::vertical([
        Constraint::Length(1),
        Constraint::Length(1),
        Constraint::Fill(1),
        Constraint::Length(1),
        Constraint::Length(1),
    ])
    .areas(frame.area());

    let keys = match app.view {
        View::Browse => {
            let total = app.tree.node(app.cwd).size;
            let mut summary = format!(
                "합계 {} · 정리 후보 {}",
                format_size(total),
                format_size(app.tree.candidate_total())
            );
            if app.tree.skipped > 0 {
                summary.push_str(&format!(" · 건너뜀 {}개", app.tree.skipped));
            }
            let path = display_path(&app.tree.path(app.cwd), &app.home);
            draw_header(frame, header, &path, &summary);
            draw_browse(frame, body, app);
            BROWSE_KEYS
        }
        View::Candidates => {
            let count = app.candidate_rows.iter().filter(|row| matches!(row, Row::Item(_))).count();
            let summary = format!("합계 {} · {count}개", format_size(app.tree.candidate_total()));
            draw_header(frame, header, "정리 후보", &summary);
            draw_candidates(frame, body, app);
            COLLECTED_KEYS
        }
        View::LargeFiles => {
            let summary = format!("상위 {}개", app.large_files.len());
            draw_header(frame, header, "큰 파일", &summary);
            draw_large_files(frame, body, app);
            COLLECTED_KEYS
        }
    };

    draw_rule(frame, top_rule);
    draw_rule(frame, bottom_rule);
    frame.render_widget(key_line(keys), footer);
}

pub fn draw_progress(frame: &mut Frame, root: &str, files: u64, bytes: u64) {
    let lines = vec![
        Line::from(Span::styled(TITLE, bold())),
        Line::default(),
        Line::from(format!("  스캔 중… {root}")),
        Line::from(format!("  파일 {files}개 · {}", format_size(bytes))),
        Line::default(),
        key_line(PROGRESS_KEYS),
    ];
    frame.render_widget(Paragraph::new(lines), frame.area());
}

/// 왼쪽에 제목과 `context`(경로나 화면 이름), 오른쪽에 `summary`를 그린다.
fn draw_header(frame: &mut Frame, area: Rect, context: &str, summary: &str) {
    let summary = Line::from(format!("{summary} ")).style(dim());
    let [left, right] =
        Layout::horizontal([Constraint::Fill(1), Constraint::Length(summary.width() as u16)])
            .areas(area);
    let context_width = (left.width as usize).saturating_sub(Span::raw(TITLE).width() + 1);
    let title = Line::from(vec![Span::styled(TITLE, bold()), Span::raw(fit_end(context, context_width))]);
    frame.render_widget(title, left);
    frame.render_widget(summary, right);
}

/// 키 안내 한 줄. 어떤 터미널 테마에서도 읽히도록 키는 반전시키고 설명은 기본 글자색으로 쓴다.
fn key_line(keys: Keys) -> Line<'static> {
    let mut spans = Vec::new();
    for &(key, label) in keys {
        let gap = if spans.is_empty() { " " } else { "  " };
        spans.push(Span::raw(gap));
        spans.push(Span::styled(key, Style::new().add_modifier(Modifier::REVERSED)));
        spans.push(Span::raw(format!(" {label}")));
    }
    Line::from(spans)
}

fn draw_rule(frame: &mut Frame, area: Rect) {
    frame.render_widget(Line::from("─".repeat(area.width as usize)).style(dim()), area);
}

fn draw_browse(frame: &mut Frame, area: Rect, app: &App) {
    if app.items.is_empty() {
        return draw_message(frame, area, "비어 있음");
    }
    let total = app.tree.node(app.cwd).size;
    let bar_width = (area.width as usize / 4).min(30);
    let lines = app.items.iter().map(|&id| {
        let node = app.tree.node(id);
        // 합계가 0이면 비율이 NaN이 되고, `bar`는 그것을 빈 막대로 그린다.
        let ratio = node.size as f64 / total as f64;
        let mut spans = vec![
            Span::raw(size_column(node.size)),
            Span::styled(bar(ratio, bar_width), Style::new().fg(Color::Blue)),
            Span::raw("  "),
            Span::raw(&*node.name),
        ];
        if node.kind == NodeKind::Dir {
            spans.push(Span::raw("/"));
        }
        if let Some(category) = node.category {
            spans.push(Span::styled(format!("  [{}]", category.label()), category_style(category)));
        }
        Line::from(spans)
    });
    draw_list(frame, area, lines, app.selected);
}

fn draw_candidates(frame: &mut Frame, area: Rect, app: &App) {
    if app.candidate_rows.is_empty() {
        return draw_message(frame, area, "정리 후보가 없습니다");
    }
    let width = area.width as usize;
    let lines = app.candidate_rows.iter().map(|row| match *row {
        Row::Header { category, total } => {
            let label = Span::styled(format!(" {}", category.label()), category_style(category).bold());
            let total = Span::raw(format!("{} ", format_size(total)));
            let gap = width.saturating_sub(label.width() + total.width());
            Line::from(vec![label, Span::raw(" ".repeat(gap)), total])
        }
        Row::Item(id) => path_line(app, id, "  ", width),
    });
    draw_list(frame, area, lines, app.candidate_selected);
}

fn draw_large_files(frame: &mut Frame, area: Rect, app: &App) {
    if app.large_files.is_empty() {
        return draw_message(frame, area, "파일이 없습니다");
    }
    let width = area.width as usize;
    let lines = app.large_files.iter().map(|&id| path_line(app, id, "", width));
    draw_list(frame, area, lines, app.large_selected);
}

/// `{크기}  {경로}` 한 줄. 경로가 길면 끝부분(파일 이름)이 보이도록 앞을 줄인다.
fn path_line<'a>(app: &App, id: NodeId, indent: &'a str, width: usize) -> Line<'a> {
    let size = size_column(app.tree.node(id).size);
    let path = display_path(&app.tree.path(id), &app.home);
    let path_width = width.saturating_sub(indent.len() + size.len() + 1);
    Line::from(vec![Span::raw(indent), Span::raw(size), Span::raw(fit_end(&path, path_width))])
}

fn draw_message(frame: &mut Frame, area: Rect, message: &str) {
    frame.render_widget(Paragraph::new(format!("  {message}")).style(dim()), area);
}

/// 선택한 줄을 강조하고, 그 줄이 화면 가운데쯤에 오도록 스크롤해서 그린다.
fn draw_list<'a>(frame: &mut Frame, area: Rect, lines: impl ExactSizeIterator<Item = Line<'a>>, selected: usize) {
    let height = area.height as usize;
    let offset = selected.saturating_sub(height / 2).min(lines.len().saturating_sub(height));
    let mut state = ListState::default().with_offset(offset).with_selected(Some(selected));
    let list = List::new(lines).highlight_style(Style::new().add_modifier(Modifier::REVERSED));
    frame.render_stateful_widget(list, area, &mut state);
}

fn size_column(size: u64) -> String {
    format!(" {:>SIZE_WIDTH$}  ", format_size(size))
}

fn category_style(category: Category) -> Style {
    Style::new().fg(match category {
        Category::Cache => Color::Yellow,
        Category::Build => Color::Green,
        Category::Temp => Color::Magenta,
    })
}

fn bold() -> Style {
    Style::new().add_modifier(Modifier::BOLD)
}

fn dim() -> Style {
    Style::new().fg(Color::DarkGray)
}

/// `text`가 `max_width`칸보다 넓으면 앞을 `…`로 줄여서 끝부분을 남긴다.
fn fit_end(text: &str, max_width: usize) -> String {
    if Span::raw(text).width() <= max_width {
        return text.to_string();
    }
    if max_width == 0 {
        return String::new();
    }
    let mut width = 1; // "…"
    let mut start = text.len();
    for (index, ch) in text.char_indices().rev() {
        let mut buffer = [0; 4];
        let char_width = Span::raw(&*ch.encode_utf8(&mut buffer)).width();
        if width + char_width > max_width {
            break;
        }
        width += char_width;
        start = index;
    }
    format!("…{}", &text[start..])
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use ratatui::Terminal;
    use ratatui::backend::TestBackend;
    use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

    use super::*;
    use crate::app::Action::*;
    use crate::tree::{NewNode, NodeKind, Tree, sample};

    /// 화면에 그려진 글자를 모두 이어 붙이고 공백을 제거한다.
    /// 한글은 두 칸을 차지하므로 공백을 제거하고 비교해야 한다.
    fn rendered(w: u16, h: u16, render: impl FnOnce(&mut Frame)) -> String {
        let mut terminal = Terminal::new(TestBackend::new(w, h)).unwrap();
        terminal.draw(render).unwrap();
        let buffer = terminal.backend().buffer();
        buffer.content().iter().map(|cell| cell.symbol()).collect::<String>().replace(' ', "")
    }

    fn screen(app: &App, w: u16, h: u16) -> String {
        rendered(w, h, |frame| draw(frame, app))
    }

    fn app() -> App {
        App::new(sample(), PathBuf::from("/home/u"))
    }

    fn app_with_files(files: Vec<(String, u64)>) -> App {
        let children = files
            .into_iter()
            .map(|(name, size)| NewNode { name: name.into(), kind: NodeKind::File, size, category: None })
            .collect();
        let mut tree = Tree::new(PathBuf::from("/home/u"), 0);
        tree.add_children(Tree::ROOT, children);
        tree.finalize();
        App::new(tree, PathBuf::from("/home/u"))
    }

    fn press(app: &mut App, actions: &[Action]) {
        for &action in actions {
            app.handle(action);
        }
    }

    #[track_caller]
    fn assert_contains(screen: &str, expected: &[&str]) {
        for text in expected {
            assert!(screen.contains(text), "{text:?} not in screen:\n{screen}");
        }
    }

    #[test]
    fn browse_screen() {
        let mut app = app();
        let s = screen(&app, 100, 20);
        assert_contains(
            &s,
            &["storage-analyzer", "~", "movie.mov", "proj/", "9.0MB", "합계17.0MB", "정리후보8.0MB", "q종료"],
        );
        assert!(!s.contains("건너뜀"));
        assert!(!s.contains("movie.mov/"));

        press(&mut app, &[Down, Enter]);
        let s = screen(&app, 100, 20);
        assert_contains(&s, &["~/proj", "합계5.0MB", "node_modules/", "[빌드산출물]", "main.rs"]);
    }

    #[test]
    fn footers_name_esc_as_the_back_key() {
        let mut app = app();
        assert_contains(&screen(&app, 100, 20), &["Esc상위"]);
        press(&mut app, &[ToggleCandidates]);
        assert_contains(&screen(&app, 100, 20), &["Esc돌아가기"]);
        press(&mut app, &[ToggleLargeFiles]);
        assert_contains(&screen(&app, 100, 20), &["Esc돌아가기"]);
    }

    #[test]
    fn footers_fit_in_80_columns() {
        let mut app = app();
        assert_contains(&screen(&app, 80, 20), &["↑↓이동", "Enter들어가기", "c정리후보", "f큰파일", "r다시스캔", "q종료"]);
        press(&mut app, &[ToggleCandidates]);
        assert_contains(&screen(&app, 80, 20), &["Enter해당위치로이동", "r다시스캔", "q종료"]);
        press(&mut app, &[ToggleLargeFiles]);
        assert_contains(&screen(&app, 80, 20), &["Enter해당위치로이동", "r다시스캔", "q종료"]);
    }

    /// 마지막 줄에서 `symbol`이 그려진 첫 칸의 스타일.
    fn footer_style(w: u16, h: u16, render: impl FnOnce(&mut Frame), symbol: &str) -> Style {
        let mut terminal = Terminal::new(TestBackend::new(w, h)).unwrap();
        terminal.draw(render).unwrap();
        let buffer = terminal.backend().buffer();
        let cell = (0..w).map(|x| &buffer[(x, h - 1)]).find(|cell| cell.symbol() == symbol);
        cell.unwrap_or_else(|| panic!("{symbol:?} not in the last line")).style()
    }

    #[test]
    fn footer_keys_are_highlighted_and_labels_use_the_normal_text_color() {
        let app = app();
        for key in ["↑", "E", "c", "f", "r", "q"] {
            let style = footer_style(100, 20, |frame| draw(frame, &app), key);
            assert!(style.add_modifier.contains(Modifier::REVERSED), "{key} is not highlighted");
        }
        for label in ["이", "들", "정", "큰", "다", "종"] {
            let style = footer_style(100, 20, |frame| draw(frame, &app), label);
            assert!(!style.add_modifier.contains(Modifier::REVERSED), "{label} is highlighted");
            assert_eq!(style.fg, Some(Color::Reset), "{label} is not in the normal text color");
        }

        // 스캔 중 화면의 안내도 같은 모양이다. 안내는 6번째 줄에 있다.
        let progress = |frame: &mut Frame| draw_progress(frame, "~/x", 1, 1);
        assert!(footer_style(80, 6, progress, "q").add_modifier.contains(Modifier::REVERSED));
        assert_eq!(footer_style(80, 6, progress, "종").fg, Some(Color::Reset));
    }

    #[test]
    fn skipped_count_shown() {
        let mut app = app();
        app.tree.skipped = 3;
        assert_contains(&screen(&app, 100, 20), &["건너뜀3개"]);
    }

    #[test]
    fn empty_folder_message() {
        let mut app = app();
        press(&mut app, &[Down, Down, Down, Down, Enter]);
        assert_contains(&screen(&app, 100, 20), &["~/empty", "비어있음"]);
    }

    #[test]
    fn candidates_screen() {
        let mut app = app();
        press(&mut app, &[ToggleCandidates]);
        let s = screen(&app, 100, 20);
        assert_contains(
            &s,
            &[
                "정리후보",
                "합계8.0MB·2개",
                "캐시",
                "3.0MB~/Library/Caches",
                "빌드산출물",
                "5.0MB~/proj/node_modules",
                "Esc돌아가기",
            ],
        );
    }

    #[test]
    fn no_candidates_message() {
        let mut app = app_with_files(vec![("f".into(), 5)]);
        press(&mut app, &[ToggleCandidates]);
        assert_contains(&screen(&app, 100, 20), &["정리후보가없습니다"]);
    }

    #[test]
    fn large_files_screen() {
        let mut app = app();
        press(&mut app, &[ToggleLargeFiles]);
        let s = screen(&app, 100, 20);
        assert_contains(&s, &["큰파일", "상위4개", "9.0MB~/movie.mov", "~/proj/main.rs", "Esc돌아가기"]);
        assert!(!s.contains("big.bin"));
    }

    #[test]
    fn no_files_message() {
        let mut app = app_with_files(vec![]);
        press(&mut app, &[ToggleLargeFiles]);
        assert_contains(&screen(&app, 100, 20), &["파일이없습니다"]);
    }

    #[test]
    fn tiny_terminal_does_not_panic() {
        for (w, h) in [(1, 1), (10, 3), (20, 5)] {
            let mut app = app();
            screen(&app, w, h);
            press(&mut app, &[ToggleCandidates]);
            screen(&app, w, h);
            press(&mut app, &[ToggleLargeFiles]);
            screen(&app, w, h);
            rendered(w, h, |frame| draw_progress(frame, "~/x", 1234, 5_000_000));
        }
    }

    #[test]
    fn long_wide_names_do_not_panic() {
        let name = "한글이름이아주긴파일".repeat(10);
        for w in [40, 21, 3] {
            let mut app = app_with_files(vec![(name.clone(), 5_000)]);
            assert_contains(&screen(&app, 40, 10), &["한글이름"]);
            screen(&app, w, 10);
            press(&mut app, &[ToggleLargeFiles]);
            screen(&app, w, 10);
        }
    }

    #[test]
    fn long_paths_keep_their_end() {
        let mut app = app_with_files(vec![(format!("{}-end.mov", "x".repeat(200)), 5_000)]);
        press(&mut app, &[ToggleLargeFiles]);
        assert_contains(&screen(&app, 60, 10), &["…", "-end.mov"]);
    }

    #[test]
    fn selection_scrolls_into_view() {
        let files = (0..100).map(|i| (format!("f{i:03}"), 1_000_000 - i)).collect();
        let mut app = app_with_files(files);
        assert_contains(&screen(&app, 80, 12), &["f000"]);
        press(&mut app, &[Down; 60]);
        let s = screen(&app, 80, 12);
        assert_contains(&s, &["f060"]);
        assert!(!s.contains("f000"));
        press(&mut app, &[Down; 60]);
        assert_contains(&screen(&app, 80, 12), &["f099"]);
    }

    #[test]
    fn zero_size_folder() {
        let app = app_with_files(vec![("zero".into(), 0)]);
        assert_contains(&screen(&app, 80, 10), &["zero", "합계0B"]);
    }

    #[test]
    fn progress_screen() {
        let s = rendered(80, 10, |frame| draw_progress(frame, "~/x", 1234, 5_000_000));
        assert_contains(&s, &["storage-analyzer", "스캔중", "~/x", "파일1234개", "5.0MB", "q종료"]);
    }

    #[test]
    fn fit_end_keeps_the_tail() {
        assert_eq!(fit_end("~/a/b/c", 10), "~/a/b/c");
        assert_eq!(fit_end("~/a/b/c", 7), "~/a/b/c");
        assert_eq!(fit_end("~/a/b/c", 5), "…/b/c");
        assert_eq!(fit_end("~/a/b/c", 1), "…");
        assert_eq!(fit_end("~/a/b/c", 0), "");
        // 한글은 두 칸이다: "…" 1칸 + "나다" 4칸.
        assert_eq!(fit_end("가나다", 5), "…나다");
        assert_eq!(fit_end("가나다", 4), "…다");
    }

    #[test]
    fn key_mapping() {
        let key = |code| action_for(KeyEvent::new(code, KeyModifiers::NONE));
        assert_eq!(key(KeyCode::Up), Some(Up));
        assert_eq!(key(KeyCode::Char('k')), Some(Up));
        assert_eq!(key(KeyCode::Down), Some(Down));
        assert_eq!(key(KeyCode::Char('j')), Some(Down));
        assert_eq!(key(KeyCode::Enter), Some(Enter));
        assert_eq!(key(KeyCode::Right), Some(Enter));
        assert_eq!(key(KeyCode::Left), Some(Back));
        assert_eq!(key(KeyCode::Backspace), Some(Back));
        assert_eq!(key(KeyCode::Esc), Some(Back));
        assert_eq!(key(KeyCode::Char('c')), Some(ToggleCandidates));
        assert_eq!(key(KeyCode::Char('f')), Some(ToggleLargeFiles));
        assert_eq!(key(KeyCode::Char('q')), Some(Quit));
        assert_eq!(key(KeyCode::Char('r')), Some(Rescan));
        assert_eq!(key(KeyCode::Char('ㄱ')), Some(Rescan));
        assert_eq!(key(KeyCode::Char('x')), None);
        // 한글 입력 상태에서 같은 자리의 키를 눌렀을 때 (두벌식)
        assert_eq!(key(KeyCode::Char('ㅊ')), Some(ToggleCandidates));
        assert_eq!(key(KeyCode::Char('ㄹ')), Some(ToggleLargeFiles));
        assert_eq!(key(KeyCode::Char('ㅂ')), Some(Quit));
        assert_eq!(key(KeyCode::Char('ㅓ')), Some(Down));
        assert_eq!(key(KeyCode::Char('ㅏ')), Some(Up));
        // Caps Lock이나 Shift가 눌린 상태
        let shifted = |ch| action_for(KeyEvent::new(KeyCode::Char(ch), KeyModifiers::SHIFT));
        assert_eq!(shifted('C'), Some(ToggleCandidates));
        assert_eq!(shifted('R'), Some(Rescan));
        assert_eq!(shifted('F'), Some(ToggleLargeFiles));
        assert_eq!(shifted('Q'), Some(Quit));
        assert_eq!(shifted('J'), Some(Down));
        assert_eq!(shifted('K'), Some(Up));
        assert_eq!(
            action_for(KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL)),
            Some(Quit)
        );
    }
}
