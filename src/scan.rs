//! 디스크 순회. 파일 메타데이터와 디렉터리 목록만 읽는다.

use std::collections::{HashMap, HashSet};
use std::ffi::OsString;
use std::fs::{self, Metadata};
use std::io;
use std::os::macos::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Condvar, Mutex};
use std::thread;

use crate::classify::classify;
use crate::tree::{NewNode, NodeId, NodeKind, Tree};

/// 스캔 스레드가 갱신하고 화면이 읽는 진행 상황.
#[derive(Default)]
pub struct Progress {
    pub files: AtomicU64,
    pub bytes: AtomicU64,
}

/// `st_flags`의 `SF_DATALESS` 비트: 내용을 아직 내려받지 않은 클라우드 항목.
const SF_DATALESS: u32 = 0x4000_0000;

pub fn is_dataless(st_flags: u32) -> bool {
    st_flags & SF_DATALESS != 0
}

/// 이 프로세스가 디스크를 읽는 것만으로는 디스크가 바뀌지 않게 한다.
///
/// macOS는 읽기만 해도 두 가지를 디스크에 쓴다. 폴더 목록을 읽으면 접근 시각을
/// 기록하고, 아직 내려받지 않은 클라우드 폴더의 목록을 읽으면 내용을 내려받는다.
/// 둘 다 프로세스 단위로 끈다. 여러 번 호출해도 된다.
pub fn set_read_only_io_policy() -> io::Result<()> {
    const IOPOL_SCOPE_PROCESS: i32 = 0;
    const IOPOL_TYPE_VFS_ATIME_UPDATES: i32 = 2;
    const IOPOL_ATIME_UPDATES_OFF: i32 = 1;
    const IOPOL_TYPE_VFS_MATERIALIZE_DATALESS_FILES: i32 = 3;
    const IOPOL_MATERIALIZE_DATALESS_FILES_OFF: i32 = 1;

    unsafe extern "C" {
        fn setiopolicy_np(iotype: i32, scope: i32, policy: i32) -> i32;
    }

    for (iotype, policy) in [
        (IOPOL_TYPE_VFS_ATIME_UPDATES, IOPOL_ATIME_UPDATES_OFF),
        (IOPOL_TYPE_VFS_MATERIALIZE_DATALESS_FILES, IOPOL_MATERIALIZE_DATALESS_FILES_OFF),
    ] {
        // SAFETY: 정수 인자만 받는 libSystem 함수이고, 실패하면 -1을 돌려주고 errno를 설정한다.
        if unsafe { setiopolicy_np(iotype, IOPOL_SCOPE_PROCESS, policy) } != 0 {
            return Err(io::Error::last_os_error());
        }
    }
    Ok(())
}

/// `/`를 스캔할 때 `/System/Volumes/Data`는 같은 데이터가 한 번 더 보이는 경로다.
fn is_duplicate_data_volume(root: &Path, dir: &Path) -> bool {
    root == Path::new("/") && dir == Path::new("/System/Volumes/Data")
}

/// 폴더 안으로 들어가도 되는지 판단한다. 들어가지 않는 폴더도 노드는 만든다.
fn should_descend(root: &Path, root_dev: u64, dir: &Path, dev: u64, st_flags: u32) -> bool {
    dev == root_dev && !is_dataless(st_flags) && !is_duplicate_data_volume(root, dir)
}

/// 폴더를 읽는 스레드 수. 성능 코어가 4개인 Mac에서 재 보니 그보다 늘려도 거의 빨라지지 않았다.
const SCAN_THREADS: usize = 4;

/// 아직 처리하지 않은 폴더.
struct Pending {
    id: NodeId,
    path: PathBuf,
    inside_candidate: bool,
}

/// 폴더 하나를 읽은 결과. 트리에 넣기 전이라 자식들은 폴더 안에서의 순번으로 가리킨다.
struct Listing {
    children: Vec<NewNode>,
    /// 들어가서 읽을 하위 폴더: (순번, 경로, 정리 후보 안쪽인지).
    subdirs: Vec<(usize, PathBuf, bool)>,
    /// 하드 링크가 둘 이상인 파일: (순번, (장치, inode)).
    links: Vec<(usize, (u64, u64))>,
    /// 모든 항목을 읽었는지.
    complete: bool,
}

/// 스캔 스레드들이 함께 쓰는 상태.
struct Shared {
    tree: Tree,
    pending: Vec<Pending>,
    /// 지금 폴더를 읽고 있는 스레드 수. `pending`이 비었고 이 값이 0이면 스캔이 끝난 것이다.
    active: usize,
    /// 하드 링크가 둘 이상인 파일들. 스캔이 끝난 뒤 한 번만 세도록 정리한다.
    links: HashMap<NodeId, (u64, u64)>,
    /// 진행 상황의 용량에 같은 파일을 두 번 더하지 않기 위한 기록.
    seen_links: HashSet<(u64, u64)>,
}

struct Scanner<'a> {
    root: &'a Path,
    home: &'a Path,
    root_dev: u64,
    progress: &'a Progress,
    shared: Mutex<Shared>,
    /// 읽을 폴더가 새로 생겼거나 스캔이 끝났음을 기다리는 스레드에게 알린다.
    changed: Condvar,
}

/// `root` 아래를 모두 읽어 트리를 만든다.
///
/// `root`는 정규화된 절대 경로여야 한다. `root` 자체를 읽지 못하거나 읽기 전용
/// 설정을 적용하지 못할 때만 `Err`를 돌려준다.
pub fn scan(root: &Path, home: &Path, progress: &Progress) -> io::Result<Tree> {
    set_read_only_io_policy()?;
    let root_meta = fs::symlink_metadata(root)?;
    // 루트를 읽지 못하면 스캔 전체가 실패한다. 그 아래의 폴더는 건너뛸 뿐이다.
    let root_entries = read_entries(root)?;

    let scanner = Scanner {
        root,
        home,
        root_dev: root_meta.st_dev(),
        progress,
        shared: Mutex::new(Shared {
            tree: Tree::new(root.to_path_buf(), disk_size(&root_meta)),
            pending: Vec::new(),
            active: 0,
            links: HashMap::new(),
            seen_links: HashSet::new(),
        }),
        changed: Condvar::new(),
    };
    let root_dir = Pending { id: Tree::ROOT, path: root.to_path_buf(), inside_candidate: false };
    let listing = scanner.inspect(&root_dir, root_entries);
    scanner.add(&mut scanner.shared.lock().unwrap(), &root_dir, listing);

    thread::scope(|scope| {
        for _ in 0..SCAN_THREADS {
            scope.spawn(|| scanner.work());
        }
    });

    let Shared { mut tree, links, .. } = scanner.shared.into_inner().unwrap();
    count_links_once(&mut tree, &links);
    tree.finalize();
    Ok(tree)
}

impl Scanner<'_> {
    /// 읽을 폴더가 남아 있는 동안 하나씩 꺼내 읽는다. 디스크를 읽는 동안에는 잠금을 잡지 않는다.
    fn work(&self) {
        let mut shared = self.shared.lock().unwrap();
        loop {
            let Some(dir) = shared.pending.pop() else {
                if shared.active == 0 {
                    return;
                }
                shared = self.changed.wait(shared).unwrap();
                continue;
            };
            shared.active += 1;
            drop(shared);

            let listing = read_entries(&dir.path).map(|entries| self.inspect(&dir, entries));

            shared = self.shared.lock().unwrap();
            shared.active -= 1;
            match listing {
                Ok(listing) => self.add(&mut shared, &dir, listing),
                Err(_) => shared.tree.skipped += 1,
            }
            self.changed.notify_all();
        }
    }

    /// 폴더의 항목들을 노드로 바꾸고, 들어갈 하위 폴더와 하드 링크를 골라낸다.
    fn inspect(&self, dir: &Pending, (entries, complete): (Vec<(OsString, Metadata)>, bool)) -> Listing {
        let mut listing = Listing {
            children: Vec::with_capacity(entries.len()),
            subdirs: Vec::new(),
            links: Vec::new(),
            complete,
        };

        for (index, (name, meta)) in entries.iter().enumerate() {
            let file_type = meta.file_type();
            let mut category = None;

            let kind = if file_type.is_dir() {
                let path = dir.path.join(name);
                if !dir.inside_candidate {
                    category = classify(&path, self.home);
                }
                if should_descend(self.root, self.root_dev, &path, meta.st_dev(), meta.st_flags()) {
                    listing.subdirs.push((index, path, dir.inside_candidate || category.is_some()));
                }
                NodeKind::Dir
            } else if file_type.is_file() {
                if meta.st_nlink() > 1 {
                    listing.links.push((index, (meta.st_dev(), meta.st_ino())));
                }
                NodeKind::File
            } else {
                NodeKind::Other
            };

            listing.children.push(NewNode {
                name: name.to_string_lossy().into(),
                kind,
                size: disk_size(meta),
                category,
            });
        }
        listing
    }

    /// 읽은 결과를 트리에 넣고, 하위 폴더를 읽을 목록에 올린다.
    fn add(&self, shared: &mut Shared, dir: &Pending, listing: Listing) {
        if !listing.complete {
            shared.tree.skipped += 1;
        }
        let mut bytes: u64 = listing.children.iter().map(|child| child.size).sum();
        for &(index, key) in &listing.links {
            if !shared.seen_links.insert(key) {
                bytes -= listing.children[index].size;
            }
        }
        self.progress.files.fetch_add(listing.children.len() as u64, Ordering::Relaxed);
        self.progress.bytes.fetch_add(bytes, Ordering::Relaxed);

        let ids = shared.tree.add_children(dir.id, listing.children);
        let id_of = |index: usize| ids.start + index as NodeId;
        shared.links.extend(listing.links.into_iter().map(|(index, key)| (id_of(index), key)));
        shared.pending.extend(listing.subdirs.into_iter().map(|(index, path, inside_candidate)| {
            Pending { id: id_of(index), path, inside_candidate }
        }));
    }
}

/// 하드 링크로 연결된 같은 파일은 처음 나오는 것만 세고 나머지는 크기를 0으로 만든다.
///
/// 스레드들이 폴더를 읽는 순서는 실행마다 다르다. 그래서 스캔이 끝난 뒤 정해진
/// 순서(폴더 단위로 깊이 우선, 폴더 안에서는 이름순)로 훑어 결과가 항상 같게 한다.
fn count_links_once(tree: &mut Tree, links: &HashMap<NodeId, (u64, u64)>) {
    if links.is_empty() {
        return;
    }
    let mut seen = HashSet::new();
    let mut pending = vec![Tree::ROOT];
    while let Some(dir) = pending.pop() {
        let children = tree.children(dir);
        for id in children.clone() {
            if let Some(&key) = links.get(&id)
                && !seen.insert(key)
            {
                tree.clear_size(id);
            }
        }
        // 스택이므로 역순으로 넣어야 이름순으로 꺼내진다.
        pending.extend(children.rev().filter(|&id| tree.node(id).kind == NodeKind::Dir));
    }
}

/// 실제 디스크 점유량. `st_blocks`의 단위는 항상 512바이트다.
fn disk_size(meta: &Metadata) -> u64 {
    meta.st_blocks() * 512
}

/// 폴더의 항목을 이름의 바이트 순서로 정렬해서 돌려준다.
///
/// 두 번째 값은 모든 항목을 읽었는지 여부다. 읽지 못한 항목이 있으면 `false`다.
/// 스캔 도중 사라진 항목은 읽지 못한 것으로 보지 않고 조용히 버린다.
fn read_entries(dir: &Path) -> io::Result<(Vec<(OsString, Metadata)>, bool)> {
    let mut entries = Vec::new();
    let mut complete = true;
    for entry in fs::read_dir(dir)? {
        let Ok(entry) = entry else {
            complete = false;
            break;
        };
        // `DirEntry::metadata`는 심볼릭 링크를 따라가지 않는다.
        match entry.metadata() {
            Ok(meta) => entries.push((entry.file_name(), meta)),
            Err(err) if err.kind() == io::ErrorKind::NotFound => {}
            Err(_) => complete = false,
        }
    }
    entries.sort_by(|a, b| a.0.cmp(&b.0));
    Ok((entries, complete))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn descends_only_into_local_downloaded_folders() {
        let root = Path::new("/Users/a");
        let dir = Path::new("/Users/a/x");
        assert!(should_descend(root, 1, dir, 1, 0));
        assert!(should_descend(root, 1, dir, 1, 0x20));
        // 다른 볼륨
        assert!(!should_descend(root, 1, dir, 2, 0));
        // 아직 내려받지 않은 클라우드 폴더
        assert!(!should_descend(root, 1, dir, 1, SF_DATALESS));
        assert!(!should_descend(root, 1, dir, 1, SF_DATALESS | 0x20));
        // `/`에서 본 데이터 볼륨
        assert!(!should_descend(Path::new("/"), 1, Path::new("/System/Volumes/Data"), 1, 0));
    }

    #[test]
    fn data_volume_is_skipped_only_when_scanning_root() {
        let data = Path::new("/System/Volumes/Data");
        assert!(is_duplicate_data_volume(Path::new("/"), data));
        assert!(!is_duplicate_data_volume(Path::new("/"), Path::new("/System/Volumes")));
        assert!(!is_duplicate_data_volume(Path::new("/"), Path::new("/Users")));
        assert!(!is_duplicate_data_volume(
            Path::new("/Users/a"),
            Path::new("/Users/a/System/Volumes/Data")
        ));
    }
}
