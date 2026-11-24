//! テスト専用の補助。
//!
//! 一時ディレクトリ名をプロセスIDと時刻だけで作ると、並列実行のテストが同じ時刻を
//! 読んだときに同じ名前になる（Windows の時刻は100ns刻みで、ミリ秒の `now_ms` はさらに粗い）。
//! 同じフォルダで2つのテストが `git init` すると、テンプレートのコピーが
//! `File exists` で失敗したり、片方の後始末がもう片方のフィクスチャを消したりする。

use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};

/// `prefix` で始まる、まだ誰も使っていない一時ディレクトリを作って返す。
///
/// プロセス内の通し番号で並列テスト同士を区別し、`create_dir`（既存なら失敗する）で
/// 作成そのものを排他にする。前回の実行の残骸や別プロセスと重なった場合は次の番号で作り直す。
pub(crate) fn unique_temp_dir(prefix: &str) -> PathBuf {
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let parent = std::env::temp_dir();
    loop {
        let n = NEXT.fetch_add(1, Ordering::Relaxed);
        let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or(0);
        let dir = parent.join(format!("{prefix}-{}-{nanos}-{n}", std::process::id()));
        match std::fs::create_dir(&dir) {
            Ok(()) => return dir,
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(e) => panic!("cannot create temp dir {}: {e}", dir.display()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::unique_temp_dir;

    #[test]
    fn concurrent_callers_never_share_a_directory() {
        let handles: Vec<_> = (0..16).map(|_| std::thread::spawn(|| (0..32).map(|_| unique_temp_dir("agentdock-uniq-test")).collect::<Vec<_>>())).collect();
        let mut all: Vec<_> = handles.into_iter().flat_map(|h| h.join().unwrap()).collect();
        let total = all.len();
        all.sort();
        all.dedup();
        assert_eq!(all.len(), total, "every call returns a distinct directory");
        for d in &all {
            assert!(d.is_dir());
            std::fs::remove_dir(d).unwrap();
        }
    }
}
