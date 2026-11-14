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

/// 아직 처리하지 않은 폴더.
struct Pending {
    id: NodeId,
    path: PathBuf,
    inside_candidate: bool,
}

/// `root` 아래를 모두 읽어 트리를 만든다.
///
/// `root`는 정규화된 절대 경로여야 한다. `root` 자체를 읽지 못하거나 읽기 전용
/// 설정을 적용하지 못할 때만 `Err`를 돌려준다.
pub fn scan(root: &Path, home: &Path, progress: &Progress) -> io::Result<Tree> {
    set_read_only_io_policy()?;
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
                if should_descend(root, root_dev, &path, meta.st_dev(), meta.st_flags()) {
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
