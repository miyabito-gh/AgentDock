//! ファイル書込みの低水準処理（P2）。
//!
//! 原子的書込みの手順（`write_atomic`）:
//! 1. 同じディレクトリに `<name>.tmp-<pid>-<counter>` を `create_new` で作る
//! 2. 全量を書き、`sync_all`
//! 3. `std::fs::rename` で置換（Windowsでは `MoveFileExW(MOVEFILE_REPLACE_EXISTING)`。同一ボリューム内で原子的）
//! 4. 失敗したら一時ファイルを消し、元のファイルは変えない
//!
//! 共有違反（ウイルス対策ソフト等）での rename 失敗は短い間隔で数回だけ再試行し、それでも失敗なら `SaveFailed` にする。

use std::io;
use std::path::Path;

/// rename の共有違反に対する再試行回数と間隔（ミリ秒）。
pub const RENAME_RETRIES: u32 = 5;
pub const RENAME_RETRY_INTERVAL_MS: u64 = 50;

pub fn write_atomic(path: &Path, bytes: &[u8]) -> io::Result<()> {
    let _ = (path, bytes);
    todo!("P2")
}

/// 1行を追記して flush する（JSONL）。行末の `\n` はこの関数が付ける。
pub fn append_line(path: &Path, line: &str) -> io::Result<()> {
    let _ = (path, line);
    todo!("P2")
}

/// パスのあるボリュームの空き（呼出し元が使える量）。Windowsでは `GetDiskFreeSpaceExW`（windows-sys）。
pub fn free_space(path: &Path) -> io::Result<u64> {
    let _ = path;
    todo!("P2")
}

/// ディレクトリ配下の合計サイズ。読めなかったパスを別に返す（0で代用しない）。
pub fn dir_size(path: &Path) -> (u64, Vec<String>) {
    let _ = path;
    todo!("P7")
}
