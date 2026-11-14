//! 디스크 순회. 파일 메타데이터와 디렉터리 목록만 읽는다.

use std::collections::HashSet;
use std::ffi::OsString;
use std::fs::{self, Metadata};
use std::io;
use std::os::macos::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

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

/// 이 프로세스가 클라우드 항목의 내려받기를 일으키지 않게 한다.
///
/// 내려받지 않은 폴더는 목록을 읽는 것만으로 macOS가 내용을 내려받아 디스크에
/// 쓸 수 있다. 스캔은 그런 폴더에 들어가지 않지만, 이 설정이 그것을 보장한다.
pub fn disable_dataless_materialization() {
    const IOPOL_TYPE_VFS_MATERIALIZE_DATALESS_FILES: i32 = 3;
    const IOPOL_SCOPE_PROCESS: i32 = 0;
    const IOPOL_MATERIALIZE_DATALESS_FILES_OFF: i32 = 1;

    unsafe extern "C" {
        fn setiopolicy_np(iotype: i32, scope: i32, policy: i32) -> i32;
    }

    // SAFETY: 정수 인자만 받는 libSystem 함수이고, 실패하면 -1을 돌려줄 뿐이다.
    unsafe {
        setiopolicy_np(
            IOPOL_TYPE_VFS_MATERIALIZE_DATALESS_FILES,
            IOPOL_SCOPE_PROCESS,
            IOPOL_MATERIALIZE_DATALESS_FILES_OFF,
        );
    }
}

/// `/`를 스캔할 때 `/System/Volumes/Data`는 같은 데이터가 한 번 더 보이는 경로다.
fn is_duplicate_data_volume(root: &Path, dir: &Path) -> bool {
    root == Path::new("/") && dir == Path::new("/System/Volumes/Data")
}

/// 아직 처리하지 않은 폴더.
struct Pending {
    id: NodeId,
    path: PathBuf,
    inside_candidate: bool,
}

/// `root` 아래를 모두 읽어 트리를 만든다.
///
/// `root`는 정규화된 절대 경로여야 한다. `root` 자체를 읽지 못할 때만 `Err`를 돌려준다.
pub fn scan(root: &Path, home: &Path, progress: &Progress) -> io::Result<Tree> {
    let root_meta = fs::symlink_metadata(root)?;
    let root_dev = root_meta.st_dev();
    let mut tree = Tree::new(root.to_path_buf(), disk_size(&root_meta));
    // 하드 링크로 연결된 파일을 한 번만 세기 위한 (장치, inode) 기록.
    let mut seen_links: HashSet<(u64, u64)> = HashSet::new();

    // 루트를 읽지 못하면 스캔 전체가 실패한다. 그 아래의 폴더는 건너뛸 뿐이다.
    let mut root_entries = Some(read_entries(root)?);
    let mut pending =
        vec![Pending { id: Tree::ROOT, path: root.to_path_buf(), inside_candidate: false }];

    while let Some(dir) = pending.pop() {
        let (entries, complete) = match root_entries.take() {
            Some(entries) => entries,
            None => match read_entries(&dir.path) {
                Ok(entries) => entries,
                Err(_) => {
                    tree.skipped += 1;
                    continue;
                }
            },
        };
        if !complete {
            tree.skipped += 1;
        }

        let mut children = Vec::with_capacity(entries.len());
        let mut subdirs = Vec::new();
        let mut bytes = 0;

        for (index, (name, meta)) in entries.iter().enumerate() {
            let file_type = meta.file_type();
            let mut size = disk_size(meta);
            let mut category = None;

            let kind = if file_type.is_dir() {
                let path = dir.path.join(name);
                if !dir.inside_candidate {
                    category = classify(&path, home);
                }
                let descend = meta.st_dev() == root_dev
                    && !is_dataless(meta.st_flags())
                    && !is_duplicate_data_volume(root, &path);
                if descend {
                    subdirs.push((index, path, dir.inside_candidate || category.is_some()));
                }
                NodeKind::Dir
            } else if file_type.is_file() {
                if meta.st_nlink() > 1 && !seen_links.insert((meta.st_dev(), meta.st_ino())) {
                    size = 0;
                }
                NodeKind::File
            } else {
                NodeKind::Other
            };

            bytes += size;
            children.push(NewNode { name: name.to_string_lossy().into(), kind, size, category });
        }

        progress.files.fetch_add(children.len() as u64, Ordering::Relaxed);
        progress.bytes.fetch_add(bytes, Ordering::Relaxed);
        let ids = tree.add_children(dir.id, children);

        // 스택이므로 역순으로 넣어야 이름순으로 꺼내진다.
        for (index, path, inside_candidate) in subdirs.into_iter().rev() {
            pending.push(Pending { id: ids.start + index as NodeId, path, inside_candidate });
        }
    }

    tree.finalize();
    Ok(tree)
}

/// 실제 디스크 점유량. `st_blocks`의 단위는 항상 512바이트다.
fn disk_size(meta: &Metadata) -> u64 {
    meta.st_blocks() * 512
}

/// 폴더의 항목을 이름의 바이트 순서로 정렬해서 돌려준다.
///
/// 두 번째 값은 목록을 끝까지 읽었는지 여부다. 스캔 도중 사라진 항목은 조용히 버린다.
fn read_entries(dir: &Path) -> io::Result<(Vec<(OsString, Metadata)>, bool)> {
    let mut entries = Vec::new();
    let mut complete = true;
    for entry in fs::read_dir(dir)? {
        let Ok(entry) = entry else {
            complete = false;
            break;
        };
        // `DirEntry::metadata`는 심볼릭 링크를 따라가지 않는다.
        if let Ok(meta) = entry.metadata() {
            entries.push((entry.file_name(), meta));
        }
    }
    entries.sort_by(|a, b| a.0.cmp(&b.0));
    Ok((entries, complete))
}

#[cfg(test)]
mod tests {
    use super::*;

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
