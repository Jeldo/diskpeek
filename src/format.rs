use std::path::Path;

/// 10진 단위(1 KB = 1000 B)로, 소수 첫째 자리까지 표시한다.
pub fn format_size(bytes: u64) -> String {
    const UNITS: [&str; 4] = ["KB", "MB", "GB", "TB"];
    if bytes < 1000 {
        return format!("{bytes} B");
    }
    let mut value = bytes as f64 / 1000.0;
    let mut unit = 0;
    // 999.95 이상은 반올림하면 1000.0이 되므로 다음 단위로 넘긴다.
    while value >= 999.95 && unit < UNITS.len() - 1 {
        value /= 1000.0;
        unit += 1;
    }
    format!("{value:.1} {}", UNITS[unit])
}

/// 홈 폴더 아래의 경로는 `~`로 줄여서 표시한다.
pub fn display_path(path: &Path, home: &Path) -> String {
    match path.strip_prefix(home) {
        Ok(rest) if rest.as_os_str().is_empty() => "~".to_string(),
        Ok(rest) => format!("~/{}", rest.display()),
        Err(_) => path.display().to_string(),
    }
}

/// `ratio`(0~1)만큼 채워진, 폭이 `width`칸인 막대.
pub fn bar(ratio: f64, width: usize) -> String {
    let ratio = if ratio.is_nan() { 0.0 } else { ratio.clamp(0.0, 1.0) };
    let filled = (ratio * width as f64).round() as usize;
    "█".repeat(filled) + &"░".repeat(width - filled)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    #[test]
    fn sizes() {
        assert_eq!(format_size(0), "0 B");
        assert_eq!(format_size(999), "999 B");
        assert_eq!(format_size(1_000), "1.0 KB");
        assert_eq!(format_size(1_500_000), "1.5 MB");
        assert_eq!(format_size(128_400_000_000), "128.4 GB");
        assert_eq!(format_size(2_000_000_000_000), "2.0 TB");
    }

    #[test]
    fn paths() {
        let home = Path::new("/Users/a");
        assert_eq!(display_path(Path::new("/Users/a"), home), "~");
        assert_eq!(display_path(Path::new("/Users/a/Github/x"), home), "~/Github/x");
        assert_eq!(display_path(Path::new("/Users/ab/x"), home), "/Users/ab/x");
        assert_eq!(display_path(Path::new("/opt/x"), home), "/opt/x");
    }

    #[test]
    fn bars() {
        assert_eq!(bar(0.5, 10), "█████░░░░░");
        assert_eq!(bar(0.0, 4), "░░░░");
        assert_eq!(bar(1.0, 4), "████");
        assert_eq!(bar(2.0, 4), "████");
        assert_eq!(bar(f64::NAN, 4), "░░░░");
        assert_eq!(bar(0.5, 0), "");
    }
}
