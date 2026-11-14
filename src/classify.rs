//! 정리 후보 분류 규칙.
//!
//! 원칙: 확실한 경우에만 표시한다. 알려진 고정 경로이거나, 같은 폴더에
//! 프로젝트 표지 파일이 있을 때만 후보로 본다.

use std::path::Path;

/// 정리 후보의 종류. 선언 순서가 화면 표시 순서다.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Category {
    Cache,
    Build,
    Temp,
}

impl Category {
    pub fn label(self) -> &'static str {
        match self {
            Category::Cache => "캐시",
            Category::Build => "빌드 산출물",
            Category::Temp => "임시 데이터",
        }
    }
}

/// 홈 폴더 기준의 고정 경로.
const FIXED_PATHS: &[(&str, Category)] = &[
    ("Library/Caches", Category::Cache),
    (".cache", Category::Cache),
    (".npm", Category::Cache),
    (".yarn/cache", Category::Cache),
    ("Library/pnpm/store", Category::Cache),
    (".cargo/registry", Category::Cache),
    (".cargo/git", Category::Cache),
    (".gradle/caches", Category::Cache),
    (".m2/repository", Category::Cache),
    ("go/pkg/mod", Category::Cache),
    ("Library/Developer/Xcode/DerivedData", Category::Build),
    ("Library/Logs", Category::Temp),
    (".Trash", Category::Temp),
];

/// 폴더 이름과, 같은 폴더에 있어야 하는 표지 파일들(하나만 있으면 된다).
const MARKER_RULES: &[(&str, &[&str])] = &[
    ("node_modules", &["package.json"]),
    (".next", &["package.json"]),
    ("target", &["Cargo.toml", "pom.xml"]),
    ("build", &["build.gradle", "build.gradle.kts"]),
    ("Pods", &["Podfile"]),
];

/// `dir`이 정리 후보이면 그 종류를 돌려준다.
///
/// `dir`은 실제 디렉터리(심볼릭 링크 아님)의 절대 경로여야 한다.
pub fn classify(dir: &Path, home: &Path) -> Option<Category> {
    if let Ok(rel) = dir.strip_prefix(home) {
        for (fixed, category) in FIXED_PATHS {
            if rel == Path::new(fixed) {
                return Some(*category);
            }
        }
    }

    let name = dir.file_name()?.to_str()?;
    match name {
        // Python이 자동으로 만드는 폴더 외에 이 이름을 쓰는 경우가 사실상 없다.
        "__pycache__" => Some(Category::Build),
        ".venv" | "venv" => dir.join("pyvenv.cfg").is_file().then_some(Category::Build),
        _ => {
            let (_, markers) = MARKER_RULES.iter().find(|(dir_name, _)| *dir_name == name)?;
            let parent = dir.parent()?;
            markers
                .iter()
                .any(|marker| parent.join(marker).is_file())
                .then_some(Category::Build)
        }
    }
}
