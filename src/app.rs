//! 화면 상태와 키 동작. 터미널과 무관하다.

use std::mem;
use std::path::{Path, PathBuf};

use crate::classify::Category;
use crate::tree::{NodeId, NodeKind, Tree};

/// 큰 파일 모아 보기에 표시하는 파일 수.
pub const LARGE_FILE_LIMIT: usize = 100;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum View {
    Browse,
    Candidates,
    LargeFiles,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Action {
    Up,
    Down,
    Enter,
    Back,
    ToggleCandidates,
    ToggleLargeFiles,
    Rescan,
    Quit,
}

/// 정리 후보 모아 보기의 한 줄.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Row {
    /// 종류 묶음의 제목. `total`은 그 종류의 합계다.
    Header { category: Category, total: u64 },
    Item(NodeId),
}

pub struct App {
    pub tree: Tree,
    pub home: PathBuf,
    pub view: View,
    pub should_quit: bool,
    /// 다시 스캔해 달라는 요청. 새 트리를 `replace_tree`로 넘기면 지워진다.
    pub wants_rescan: bool,

    /// 탐색 화면: 현재 폴더, 그 자식들(크기순), 선택한 줄.
    pub cwd: NodeId,
    pub items: Vec<NodeId>,
    pub selected: usize,

    /// 정리 후보 모아 보기. `candidate_selected`는 항상 `Row::Item`을 가리킨다.
    pub candidate_rows: Vec<Row>,
    pub candidate_selected: usize,

    /// 큰 파일 모아 보기.
    pub large_files: Vec<NodeId>,
    pub large_selected: usize,
}

impl App {
    pub fn new(tree: Tree, home: PathBuf) -> App {
        let items = tree.children_by_size(Tree::ROOT);
        let candidate_rows = candidate_rows(&tree);
        let large_files = tree.largest_files(LARGE_FILE_LIMIT);
        App {
            tree,
            home,
            view: View::Browse,
            should_quit: false,
            wants_rescan: false,
            cwd: Tree::ROOT,
            items,
            selected: 0,
            // 후보가 있으면 0번 줄은 묶음 제목이고 1번 줄이 첫 항목이다.
            candidate_selected: if candidate_rows.is_empty() { 0 } else { 1 },
            candidate_rows,
            large_files,
            large_selected: 0,
        }
    }

    pub fn handle(&mut self, action: Action) {
        match action {
            Action::Quit => self.should_quit = true,
            Action::Rescan => self.wants_rescan = true,
            Action::ToggleCandidates => self.toggle(View::Candidates),
            Action::ToggleLargeFiles => self.toggle(View::LargeFiles),
            Action::Up => self.move_selection(false),
            Action::Down => self.move_selection(true),
            Action::Enter => self.enter(),
            Action::Back => self.back(),
        }
    }

    /// 다시 스캔한 트리로 바꾼다. 보던 화면과 선택한 항목을 되도록 유지한다.
    pub fn replace_tree(&mut self, tree: Tree) {
        // 이전 트리의 id는 새 트리에서 쓸 수 없으므로 위치를 경로로 옮긴다.
        let cwd = self.tree.path(self.cwd);
        let selected = self.items.get(self.selected).map(|&id| self.tree.path(id));
        let candidate = match self.candidate_rows.get(self.candidate_selected) {
            Some(&Row::Item(id)) => Some(self.tree.path(id)),
            _ => None,
        };
        let large_file = self.large_files.get(self.large_selected).map(|&id| self.tree.path(id));

        let next = App::new(tree, self.home.clone());
        let old = mem::replace(self, next);

        match self.find(selected.as_deref()) {
            Some(target) => self.reveal(target),
            None => {
                // 선택한 항목이 사라졌으면 보던 폴더에, 그 폴더도 사라졌으면 남아 있는
                // 가장 가까운 상위 폴더에 머문다.
                let (mut folder, _) = self.tree.locate(&cwd);
                if self.tree.node(folder).kind != NodeKind::Dir {
                    folder = self.tree.node(folder).parent.unwrap_or(Tree::ROOT);
                }
                self.cwd = folder;
                self.items = self.tree.children_by_size(folder);
                self.selected = old.selected.min(self.items.len().saturating_sub(1));
            }
        }
        self.view = old.view;

        let candidate = self.find(candidate.as_deref());
        self.candidate_selected = candidate
            .and_then(|id| self.candidate_rows.iter().position(|&row| row == Row::Item(id)))
            .unwrap_or_else(|| {
                // 같은 자리가 묶음 제목이면 바로 아래 항목을 고른다. 제목 다음에는 항상 항목이 있다.
                let index = old.candidate_selected.min(self.candidate_rows.len().saturating_sub(1));
                let on_header = matches!(self.candidate_rows.get(index), Some(Row::Header { .. }));
                if on_header { index + 1 } else { index }
            });

        let large_file = self.find(large_file.as_deref());
        self.large_selected = large_file
            .and_then(|id| self.large_files.iter().position(|&file| file == id))
            .unwrap_or(old.large_selected.min(self.large_files.len().saturating_sub(1)));
    }

    /// 이전 트리의 경로가 지금 트리에도 있으면 그 노드를 돌려준다.
    fn find(&self, path: Option<&Path>) -> Option<NodeId> {
        match self.tree.locate(path?) {
            (id, true) => Some(id),
            (_, false) => None,
        }
    }

    /// 이미 그 화면이면 탐색으로 돌아간다. 탐색 위치는 바뀌지 않는다.
    fn toggle(&mut self, view: View) {
        self.view = if self.view == view { View::Browse } else { view };
    }

    fn move_selection(&mut self, down: bool) {
        match self.view {
            View::Browse => self.selected = step(self.selected, self.items.len(), down),
            View::LargeFiles => {
                self.large_selected = step(self.large_selected, self.large_files.len(), down)
            }
            View::Candidates => {
                // 묶음 제목은 건너뛴다. 그 방향에 항목이 없으면 그대로 둔다.
                let is_item = |&index: &usize| matches!(self.candidate_rows[index], Row::Item(_));
                let next = if down {
                    (self.candidate_selected + 1..self.candidate_rows.len()).find(is_item)
                } else {
                    (0..self.candidate_selected).rev().find(is_item)
                };
                if let Some(next) = next {
                    self.candidate_selected = next;
                }
            }
        }
    }

    fn enter(&mut self) {
        match self.view {
            View::Browse => {
                let Some(&target) = self.items.get(self.selected) else { return };
                if self.tree.node(target).kind == NodeKind::Dir {
                    self.cwd = target;
                    self.items = self.tree.children_by_size(target);
                    self.selected = 0;
                }
            }
            View::Candidates => {
                if let Some(&Row::Item(target)) = self.candidate_rows.get(self.candidate_selected) {
                    self.reveal(target);
                }
            }
            View::LargeFiles => {
                if let Some(&target) = self.large_files.get(self.large_selected) {
                    self.reveal(target);
                }
            }
        }
    }

    fn back(&mut self) {
        match self.view {
            View::Browse => self.reveal(self.cwd),
            // 모아 보기에서는 들어오기 전의 탐색 위치로 돌아간다.
            View::Candidates | View::LargeFiles => self.view = View::Browse,
        }
    }

    /// 탐색 화면에서 `target`이 들어 있는 폴더를 열고 `target`을 선택한다.
    /// `target`이 루트이면 아무 일도 하지 않는다.
    fn reveal(&mut self, target: NodeId) {
        let Some(parent) = self.tree.node(target).parent else { return };
        self.view = View::Browse;
        self.cwd = parent;
        self.items = self.tree.children_by_size(parent);
        self.selected = self.items.iter().position(|&id| id == target).unwrap_or(0);
    }
}

/// 길이가 `len`인 목록 안에서 선택을 한 칸 옮긴다.
fn step(selected: usize, len: usize, down: bool) -> usize {
    if down { (selected + 1).min(len.saturating_sub(1)) } else { selected.saturating_sub(1) }
}

/// 종류마다 제목 줄 다음에 그 종류의 후보들이 오는 목록을 만든다.
fn candidate_rows(tree: &Tree) -> Vec<Row> {
    let candidates = tree.candidates();
    let category_of = |&id: &NodeId| tree.node(id).category;
    let mut rows = Vec::new();
    for group in candidates.chunk_by(|a, b| category_of(a) == category_of(b)) {
        let Some(category) = category_of(&group[0]) else { continue };
        let total = group.iter().map(|&id| tree.node(id).size).sum();
        rows.push(Row::Header { category, total });
        rows.extend(group.iter().map(|&id| Row::Item(id)));
    }
    rows
}

#[cfg(test)]
mod tests {
    use super::Action::*;
    use super::*;
    use crate::tree::{NewNode, find, names, sample};

    fn app() -> App {
        App::new(sample(), PathBuf::from("/home/u"))
    }

    fn press(app: &mut App, actions: &[Action]) {
        for &action in actions {
            app.handle(action);
        }
    }

    fn selected_name(app: &App) -> &str {
        &app.tree.node(app.items[app.selected]).name
    }

    #[test]
    fn starts_in_browse_at_root() {
        let app = app();
        assert_eq!(app.view, View::Browse);
        assert_eq!(app.cwd, Tree::ROOT);
        assert_eq!(names(&app.tree, &app.items), ["movie.mov", "proj", "Library", "notes.md", "empty"]);
        assert_eq!(app.selected, 0);
        assert!(!app.should_quit);
    }

    #[test]
    fn selection_stays_in_bounds() {
        let mut app = app();
        press(&mut app, &[Up]);
        assert_eq!(app.selected, 0);
        press(&mut app, &[Down; 10]);
        assert_eq!(app.selected, 4);
    }

    #[test]
    fn enter_and_back() {
        let mut app = app();
        press(&mut app, &[Down, Enter]);
        assert_eq!(app.cwd, find(&app.tree, "proj"));
        assert_eq!(names(&app.tree, &app.items), ["node_modules", "main.rs", "package.json"]);
        assert_eq!(app.selected, 0);

        press(&mut app, &[Back]);
        assert_eq!(app.cwd, Tree::ROOT);
        assert_eq!(app.selected, 1);

        press(&mut app, &[Back]);
        assert_eq!(app.cwd, Tree::ROOT);
        assert_eq!(app.selected, 1);
    }

    #[test]
    fn enter_on_file_does_nothing() {
        let mut app = app();
        press(&mut app, &[Enter]);
        assert_eq!(app.cwd, Tree::ROOT);
        assert_eq!(app.selected, 0);
        assert_eq!(app.view, View::Browse);
    }

    #[test]
    fn empty_directory_is_safe() {
        let mut app = app();
        press(&mut app, &[Down, Down, Down, Down, Enter]);
        assert_eq!(app.cwd, find(&app.tree, "empty"));
        assert!(app.items.is_empty());

        press(&mut app, &[Up, Down, Enter]);
        assert_eq!(app.cwd, find(&app.tree, "empty"));

        press(&mut app, &[Back]);
        assert_eq!(app.cwd, Tree::ROOT);
        assert_eq!(app.selected, 4);
    }

    #[test]
    fn candidate_rows_and_header_skipping() {
        let mut app = app();
        press(&mut app, &[ToggleCandidates]);
        assert_eq!(app.view, View::Candidates);
        assert_eq!(
            app.candidate_rows,
            [
                Row::Header { category: Category::Cache, total: 3_000_000 },
                Row::Item(find(&app.tree, "Library/Caches")),
                Row::Header { category: Category::Build, total: 5_000_000 },
                Row::Item(find(&app.tree, "proj/node_modules")),
            ]
        );
        assert_eq!(app.candidate_selected, 1);
        press(&mut app, &[Down]);
        assert_eq!(app.candidate_selected, 3);
        press(&mut app, &[Down]);
        assert_eq!(app.candidate_selected, 3);
        press(&mut app, &[Up]);
        assert_eq!(app.candidate_selected, 1);
        press(&mut app, &[Up]);
        assert_eq!(app.candidate_selected, 1);
    }

    #[test]
    fn enter_on_candidate_jumps_to_location() {
        let mut app = app();
        press(&mut app, &[ToggleCandidates, Down, Enter]);
        assert_eq!(app.view, View::Browse);
        assert_eq!(app.cwd, find(&app.tree, "proj"));
        assert_eq!(selected_name(&app), "node_modules");
    }

    #[test]
    fn toggling_keeps_browse_position() {
        let mut app = app();
        press(&mut app, &[Down, Enter, Down, Down]);
        assert_eq!(app.selected, 2);

        press(&mut app, &[ToggleCandidates, ToggleCandidates]);
        assert_eq!(app.view, View::Browse);
        assert_eq!(app.cwd, find(&app.tree, "proj"));
        assert_eq!(app.selected, 2);

        press(&mut app, &[ToggleLargeFiles, ToggleLargeFiles]);
        assert_eq!(app.view, View::Browse);
        assert_eq!(app.cwd, find(&app.tree, "proj"));
        assert_eq!(app.selected, 2);
    }

    #[test]
    fn large_files_view() {
        let mut app = app();
        press(&mut app, &[ToggleLargeFiles]);
        assert_eq!(app.view, View::LargeFiles);
        assert_eq!(
            names(&app.tree, &app.large_files),
            ["movie.mov", "notes.md", "main.rs", "package.json"]
        );
        assert_eq!(app.large_selected, 0);

        press(&mut app, &[Up]);
        assert_eq!(app.large_selected, 0);
        press(&mut app, &[Down; 10]);
        assert_eq!(app.large_selected, 3);

        press(&mut app, &[Up, Enter]);
        assert_eq!(app.view, View::Browse);
        assert_eq!(app.cwd, find(&app.tree, "proj"));
        assert_eq!(selected_name(&app), "main.rs");
    }

    #[test]
    fn switching_between_views() {
        let mut app = app();
        press(&mut app, &[ToggleCandidates, ToggleLargeFiles]);
        assert_eq!(app.view, View::LargeFiles);
        press(&mut app, &[ToggleCandidates]);
        assert_eq!(app.view, View::Candidates);
        press(&mut app, &[ToggleLargeFiles, ToggleLargeFiles]);
        assert_eq!(app.view, View::Browse);
    }

    #[test]
    fn back_leaves_collected_views_and_keeps_browse_position() {
        let mut app = app();
        press(&mut app, &[Down, Enter, Down, Down]);

        press(&mut app, &[ToggleCandidates, Back]);
        assert_eq!(app.view, View::Browse);
        assert_eq!(app.cwd, find(&app.tree, "proj"));
        assert_eq!(app.selected, 2);

        press(&mut app, &[ToggleLargeFiles, Back]);
        assert_eq!(app.view, View::Browse);
        assert_eq!(app.cwd, find(&app.tree, "proj"));
        assert_eq!(app.selected, 2);
    }

    #[test]
    fn no_candidates_is_safe() {
        let mut tree = Tree::new(PathBuf::from("/r"), 0);
        tree.add_children(
            Tree::ROOT,
            vec![NewNode { name: "f".into(), kind: NodeKind::File, size: 5, category: None }],
        );
        tree.finalize();
        let mut app = App::new(tree, PathBuf::from("/home/u"));

        press(&mut app, &[ToggleCandidates, Up, Down, Enter]);
        assert!(app.candidate_rows.is_empty());
        assert_eq!(app.view, View::Candidates);
    }

    #[test]
    fn empty_tree_is_safe() {
        let mut tree = Tree::new(PathBuf::from("/r"), 0);
        tree.finalize();
        let mut app = App::new(tree, PathBuf::from("/home/u"));

        press(&mut app, &[Up, Down, Enter, Back, ToggleLargeFiles, Up, Down, Enter]);
        assert!(app.items.is_empty());
        assert!(app.large_files.is_empty());
        assert_eq!(app.view, View::LargeFiles);
    }

    #[test]
    fn large_files_are_limited() {
        let files = (0..150)
            .map(|i| NewNode { name: format!("f{i:03}").into(), kind: NodeKind::File, size: 1000 - i, category: None })
            .collect();
        let mut tree = Tree::new(PathBuf::from("/r"), 0);
        tree.add_children(Tree::ROOT, files);
        tree.finalize();
        let app = App::new(tree, PathBuf::from("/home/u"));

        assert_eq!(app.large_files.len(), LARGE_FILE_LIMIT);
        assert_eq!(LARGE_FILE_LIMIT, 100);
        assert_eq!(&*app.tree.node(app.large_files[99]).name, "f099");
    }

    /// `sample()`에서 `proj/main.rs`와 `movie.mov`만 남은 트리.
    fn smaller() -> Tree {
        let n = |name: &str, kind, size| NewNode { name: name.into(), kind, size, category: None };
        let mut tree = Tree::new(PathBuf::from("/home/u"), 0);
        let top = tree.add_children(
            Tree::ROOT,
            vec![n("proj", NodeKind::Dir, 0), n("movie.mov", NodeKind::File, 9_000_000)],
        );
        tree.add_children(top.start, vec![n("main.rs", NodeKind::File, 2_000)]);
        tree.finalize();
        tree
    }

    /// 루트 바로 아래에 `(이름, 크기, 종류)` 항목만 있는 트리. 종류가 있으면 폴더다.
    fn flat(entries: &[(&str, u64, Option<Category>)]) -> Tree {
        let children = entries
            .iter()
            .map(|&(name, size, category)| NewNode {
                name: name.into(),
                kind: if category.is_some() { NodeKind::Dir } else { NodeKind::File },
                size,
                category,
            })
            .collect();
        let mut tree = Tree::new(PathBuf::from("/home/u"), 0);
        tree.add_children(Tree::ROOT, children);
        tree.finalize();
        tree
    }

    #[test]
    fn rescan_is_requested_and_cleared() {
        let mut app = app();
        assert!(!app.wants_rescan);
        press(&mut app, &[Rescan]);
        assert!(app.wants_rescan);
        assert!(!app.should_quit);

        app.replace_tree(sample());
        assert!(!app.wants_rescan);
    }

    #[test]
    fn replace_tree_keeps_folder_and_selection() {
        let mut app = app();
        press(&mut app, &[Down, Enter, Down]);
        assert_eq!(selected_name(&app), "main.rs");

        app.replace_tree(sample());
        assert_eq!(app.view, View::Browse);
        assert_eq!(app.cwd, find(&app.tree, "proj"));
        assert_eq!(selected_name(&app), "main.rs");

        // 다른 항목이 사라져 순서가 바뀌어도 같은 항목을 따라간다.
        app.replace_tree(smaller());
        assert_eq!(app.cwd, find(&app.tree, "proj"));
        assert_eq!(selected_name(&app), "main.rs");
        assert_eq!(app.selected, 0);
    }

    #[test]
    fn replace_tree_keeps_position_when_selected_item_is_gone() {
        let mut app = app();
        press(&mut app, &[Down, Enter, Down, Down]);
        assert_eq!(selected_name(&app), "package.json");

        app.replace_tree(smaller());
        assert_eq!(app.cwd, find(&app.tree, "proj"));
        assert_eq!(names(&app.tree, &app.items), ["main.rs"]);
        assert_eq!(app.selected, 0);
    }

    #[test]
    fn replace_tree_moves_up_when_folder_is_gone() {
        let mut app = app();
        press(&mut app, &[Down, Enter, Enter]);
        assert_eq!(app.cwd, find(&app.tree, "proj/node_modules"));
        app.replace_tree(smaller());
        assert_eq!(app.cwd, find(&app.tree, "proj"));
        assert!(app.selected < app.items.len());

        let mut app = App::new(sample(), PathBuf::from("/home/u"));
        press(&mut app, &[Down, Down, Enter, Enter]);
        assert_eq!(app.cwd, find(&app.tree, "Library/Caches"));
        app.replace_tree(smaller());
        assert_eq!(app.cwd, Tree::ROOT);
        assert!(app.selected < app.items.len());
    }

    #[test]
    fn replace_tree_with_empty_tree_is_safe() {
        let mut app = app();
        press(&mut app, &[Down, Enter, Down]);
        app.replace_tree(flat(&[]));
        assert_eq!(app.cwd, Tree::ROOT);
        assert!(app.items.is_empty());
        assert_eq!(app.selected, 0);
        press(&mut app, &[Up, Down, Enter, Back, ToggleCandidates, Down, Enter, ToggleLargeFiles, Down, Enter]);
    }

    #[test]
    fn replace_tree_keeps_candidate_selection() {
        let cache = Some(Category::Cache);
        let build = Some(Category::Build);
        let mut app = App::new(
            flat(&[("c1", 30, cache), ("c2", 20, cache), ("b1", 10, build)]),
            PathBuf::from("/home/u"),
        );
        press(&mut app, &[ToggleCandidates, Down]);
        assert_eq!(app.candidate_rows[app.candidate_selected], Row::Item(find(&app.tree, "c2")));

        // 크기 순서가 바뀌어도 같은 후보를 따라간다.
        app.replace_tree(flat(&[("c1", 30, cache), ("c2", 40, cache), ("b1", 10, build)]));
        assert_eq!(app.view, View::Candidates);
        assert_eq!(app.candidate_selected, 1);
        assert_eq!(app.candidate_rows[1], Row::Item(find(&app.tree, "c2")));

        // 선택한 후보가 사라지면 같은 자리 근처의 항목을 고른다. 묶음 제목은 고르지 않는다.
        press(&mut app, &[Down]);
        assert_eq!(app.candidate_selected, 2);
        app.replace_tree(flat(&[("c2", 40, cache), ("b1", 10, build)]));
        assert_eq!(app.candidate_rows[app.candidate_selected], Row::Item(find(&app.tree, "b1")));

        app.replace_tree(flat(&[("f", 5, None)]));
        assert!(app.candidate_rows.is_empty());
        assert_eq!(app.candidate_selected, 0);
    }

    #[test]
    fn replace_tree_keeps_large_file_selection() {
        let mut app =
            App::new(flat(&[("f1", 30, None), ("f2", 20, None), ("f3", 10, None)]), PathBuf::from("/home/u"));
        press(&mut app, &[ToggleLargeFiles, Down]);
        assert_eq!(app.large_files[app.large_selected], find(&app.tree, "f2"));

        app.replace_tree(flat(&[("f1", 30, None), ("f2", 40, None), ("f3", 10, None)]));
        assert_eq!(app.view, View::LargeFiles);
        assert_eq!(app.large_selected, 0);
        assert_eq!(app.large_files[0], find(&app.tree, "f2"));

        press(&mut app, &[Down, Down]);
        assert_eq!(app.large_selected, 2);
        app.replace_tree(flat(&[("f1", 30, None)]));
        assert_eq!(app.large_selected, 0);
    }

    #[test]
    fn quit() {
        let mut app = app();
        press(&mut app, &[Quit]);
        assert!(app.should_quit);
    }
}
