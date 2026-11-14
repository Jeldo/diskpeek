//! 스캔 결과를 담는 트리.
//!
//! 홈 폴더에는 파일이 수백만 개 있을 수 있으므로 노드를 `Vec` 하나에 담고,
//! 한 폴더의 자식들에게 연속된 id를 준다. 노드마다 자식 목록을 따로 두지 않는다.

use std::cmp::Reverse;
use std::ops::Range;
use std::path::PathBuf;

use crate::classify::Category;

pub type NodeId = u32;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NodeKind {
    Dir,
    File,
    /// 심볼릭 링크, 소켓 등 그 외의 항목.
    Other,
}

/// `Tree::add_children`에 넘기는 새 항목. `size`는 그 항목 자체의 크기다.
pub struct NewNode {
    pub name: Box<str>,
    pub kind: NodeKind,
    pub size: u64,
    pub category: Option<Category>,
}

pub struct Node {
    pub name: Box<str>,
    pub kind: NodeKind,
    /// `finalize` 이후에는 폴더의 경우 모든 하위 항목을 포함한 크기다.
    pub size: u64,
    /// 정리 후보로 분류된 폴더에만 값이 있다.
    pub category: Option<Category>,
    pub parent: Option<NodeId>,
    first_child: NodeId,
    child_count: u32,
}

pub struct Tree {
    pub root_path: PathBuf,
    /// 읽지 못해서 건너뛴 폴더의 수.
    pub skipped: u64,
    nodes: Vec<Node>,
}

impl Tree {
    pub const ROOT: NodeId = 0;

    pub fn new(root_path: PathBuf, root_own_size: u64) -> Tree {
        let root = Node {
            name: "".into(),
            kind: NodeKind::Dir,
            size: root_own_size,
            category: None,
            parent: None,
            first_child: 0,
            child_count: 0,
        };
        Tree { root_path, skipped: 0, nodes: vec![root] }
    }

    /// `parent`의 자식들을 추가하고 그 id 범위를 돌려준다.
    ///
    /// 자식들이 연속된 id를 가져야 하므로 부모마다 한 번만 호출할 수 있다.
    pub fn add_children(&mut self, parent: NodeId, children: Vec<NewNode>) -> Range<NodeId> {
        debug_assert_eq!(self.nodes[parent as usize].child_count, 0, "add_children called twice");
        let start = self.nodes.len() as NodeId;
        self.nodes.extend(children.into_iter().map(|child| Node {
            name: child.name,
            kind: child.kind,
            size: child.size,
            category: child.category,
            parent: Some(parent),
            first_child: 0,
            child_count: 0,
        }));
        let end = self.nodes.len() as NodeId;
        let parent = &mut self.nodes[parent as usize];
        parent.first_child = start;
        parent.child_count = end - start;
        start..end
    }

    /// 폴더의 크기를 "자체 크기 + 모든 하위 항목"으로 바꾼다. 스캔이 끝난 뒤 한 번 호출한다.
    pub fn finalize(&mut self) {
        // 자식의 id는 항상 부모보다 크므로, 역순으로 훑으면 하위 합계가 먼저 완성된다.
        for id in (1..self.nodes.len()).rev() {
            let size = self.nodes[id].size;
            let parent = self.nodes[id].parent.expect("non-root node has a parent");
            self.nodes[parent as usize].size += size;
        }
    }

    pub fn node(&self, id: NodeId) -> &Node {
        &self.nodes[id as usize]
    }

    /// 루트를 포함한 전체 노드 수.
    pub fn node_count(&self) -> usize {
        self.nodes.len()
    }

    fn children(&self, id: NodeId) -> Range<NodeId> {
        let node = self.node(id);
        node.first_child..node.first_child + node.child_count
    }

    /// 자식들을 크기 내림차순(같으면 이름 오름차순)으로 돌려준다.
    pub fn children_by_size(&self, id: NodeId) -> Vec<NodeId> {
        let mut children: Vec<NodeId> = self.children(id).collect();
        children.sort_by(|&a, &b| {
            let (a, b) = (self.node(a), self.node(b));
            b.size.cmp(&a.size).then_with(|| a.name.cmp(&b.name))
        });
        children
    }

    pub fn path(&self, id: NodeId) -> PathBuf {
        let mut names = Vec::new();
        let mut current = id;
        while let Some(parent) = self.node(current).parent {
            names.push(&*self.node(current).name);
            current = parent;
        }
        let mut path = self.root_path.clone();
        path.extend(names.into_iter().rev());
        path
    }

    /// 정리 후보를 종류 순서, 그 안에서 크기 내림차순(같으면 경로 오름차순)으로 돌려준다.
    pub fn candidates(&self) -> Vec<NodeId> {
        let mut candidates: Vec<NodeId> =
            (0..self.nodes.len() as NodeId).filter(|&id| self.node(id).category.is_some()).collect();
        candidates.sort_by_cached_key(|&id| {
            let node = self.node(id);
            (node.category, Reverse(node.size), self.path(id))
        });
        candidates
    }

    pub fn candidate_total(&self) -> u64 {
        self.nodes.iter().filter(|node| node.category.is_some()).map(|node| node.size).sum()
    }

    /// 정리 후보 폴더 밖에 있는 일반 파일 중 큰 것부터 `limit`개.
    /// 크기 내림차순(같으면 경로 오름차순)이다.
    pub fn largest_files(&self, limit: usize) -> Vec<NodeId> {
        let mut files = Vec::new();
        let mut pending = vec![Self::ROOT];
        while let Some(dir) = pending.pop() {
            for id in self.children(dir) {
                let node = self.node(id);
                match node.kind {
                    NodeKind::File => files.push(id),
                    NodeKind::Dir if node.category.is_none() => pending.push(id),
                    _ => {}
                }
            }
        }
        // 전체를 정렬하지 않고 상위 `limit`개만 골라낸 뒤 그것만 정렬한다.
        if limit < files.len() {
            if limit > 0 {
                files.select_nth_unstable_by_key(limit - 1, |&id| (Reverse(self.node(id).size), id));
            }
            files.truncate(limit);
        }
        files.sort_by_cached_key(|&id| (Reverse(self.node(id).size), self.path(id)));
        files
    }
}

/// 여러 모듈의 테스트가 함께 쓰는 작은 트리.
///
/// ```text
/// /home/u
/// ├── proj/
/// │   ├── package.json     1_000
/// │   ├── node_modules/    [빌드 산출물]
/// │   │   └── big.bin      5_000_000
/// │   └── main.rs          2_000
/// ├── Library/
/// │   └── Caches/          [캐시]
/// │       └── blob         3_000_000
/// ├── movie.mov            9_000_000
/// ├── notes.md             4_000
/// └── empty/
/// ```
#[cfg(test)]
pub(crate) fn sample() -> Tree {
    use NodeKind::*;
    let n = |name: &str, kind, size, category| NewNode { name: name.into(), kind, size, category };
    let mut t = Tree::new(PathBuf::from("/home/u"), 0);
    let top = t.add_children(
        Tree::ROOT,
        vec![
            n("proj", Dir, 0, None),
            n("Library", Dir, 0, None),
            n("movie.mov", File, 9_000_000, None),
            n("notes.md", File, 4_000, None),
            n("empty", Dir, 0, None),
        ],
    );
    let proj = t.add_children(
        top.start,
        vec![
            n("package.json", File, 1_000, None),
            n("node_modules", Dir, 0, Some(Category::Build)),
            n("main.rs", File, 2_000, None),
        ],
    );
    t.add_children(proj.start + 1, vec![n("big.bin", File, 5_000_000, None)]);
    let lib = t.add_children(top.start + 1, vec![n("Caches", Dir, 0, Some(Category::Cache))]);
    t.add_children(lib.start, vec![n("blob", File, 3_000_000, None)]);
    t.finalize();
    t
}

/// 이름으로 내려가며 노드를 찾는다. 예: `find(&t, "proj/node_modules")`.
#[cfg(test)]
pub(crate) fn find(t: &Tree, rel: &str) -> NodeId {
    rel.split('/').fold(Tree::ROOT, |dir, name| {
        t.children_by_size(dir)
            .into_iter()
            .find(|&id| &*t.node(id).name == name)
            .unwrap_or_else(|| panic!("{name} not found in {rel}"))
    })
}

#[cfg(test)]
pub(crate) fn names(t: &Tree, ids: &[NodeId]) -> Vec<String> {
    ids.iter().map(|&id| t.node(id).name.to_string()).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finalize_sums_directories() {
        let t = sample();
        assert_eq!(t.node(Tree::ROOT).size, 17_007_000);
        assert_eq!(t.node(find(&t, "proj")).size, 5_003_000);
        assert_eq!(t.node(find(&t, "proj/node_modules")).size, 5_000_000);
        assert_eq!(t.node(find(&t, "Library")).size, 3_000_000);
        assert_eq!(t.node(find(&t, "empty")).size, 0);
    }

    #[test]
    fn children_sorted_by_size() {
        let t = sample();
        assert_eq!(
            names(&t, &t.children_by_size(Tree::ROOT)),
            ["movie.mov", "proj", "Library", "notes.md", "empty"]
        );
        assert_eq!(
            names(&t, &t.children_by_size(find(&t, "proj"))),
            ["node_modules", "main.rs", "package.json"]
        );
        assert!(t.children_by_size(find(&t, "empty")).is_empty());
        assert!(t.children_by_size(find(&t, "movie.mov")).is_empty());
    }

    #[test]
    fn equal_sizes_sorted_by_name() {
        let n = |name: &str| NewNode { name: name.into(), kind: NodeKind::File, size: 7, category: None };
        let mut t = Tree::new(PathBuf::from("/r"), 0);
        t.add_children(Tree::ROOT, vec![n("b"), n("c"), n("a")]);
        t.finalize();
        assert_eq!(names(&t, &t.children_by_size(Tree::ROOT)), ["a", "b", "c"]);
    }

    #[test]
    fn paths() {
        let t = sample();
        assert_eq!(t.path(Tree::ROOT), PathBuf::from("/home/u"));
        assert_eq!(
            t.path(find(&t, "proj/node_modules")),
            PathBuf::from("/home/u/proj/node_modules")
        );
    }

    #[test]
    fn candidates_grouped_then_by_size() {
        let t = sample();
        assert_eq!(names(&t, &t.candidates()), ["Caches", "node_modules"]);
        assert_eq!(t.candidate_total(), 8_000_000);
    }

    #[test]
    fn largest_files_exclude_candidate_contents() {
        let t = sample();
        assert_eq!(
            names(&t, &t.largest_files(100)),
            ["movie.mov", "notes.md", "main.rs", "package.json"]
        );
        assert_eq!(names(&t, &t.largest_files(2)), ["movie.mov", "notes.md"]);
        assert!(t.largest_files(0).is_empty());
    }

    #[test]
    fn node_count_includes_root() {
        assert_eq!(sample().node_count(), 12);
    }

    #[test]
    fn parent_links() {
        let t = sample();
        let proj = find(&t, "proj");
        assert_eq!(t.node(find(&t, "proj/node_modules")).parent, Some(proj));
        assert_eq!(t.node(proj).parent, Some(Tree::ROOT));
        assert_eq!(t.node(Tree::ROOT).parent, None);
    }

    #[test]
    fn root_own_size_is_counted() {
        let mut t = Tree::new(PathBuf::from("/r"), 100);
        t.add_children(
            Tree::ROOT,
            vec![NewNode { name: "f".into(), kind: NodeKind::File, size: 5, category: None }],
        );
        t.finalize();
        assert_eq!(t.node(Tree::ROOT).size, 105);
    }
}
