//! ファイル書込みの低水準処理（P2）。
//!
//! 原子的書込みの手順（`write_atomic`）:
//! 1. 同じディレクトリに `<name>.tmp-<pid>-<counter>` を `create_new` で作る
//! 2. 全量を書き、`sync_all`
//! 3. `std::fs::rename` で置換（Windowsでは `MoveFileExW(MOVEFILE_REPLACE_EXISTING)`。同一ボリューム内で原子的）
//! 4. 失敗したら一時ファイルを消し、元のファイルは変えない
//!
//! 共有違反（ウイルス対策ソフト等）での rename 失敗は短い間隔で数回だけ再試行し、それでも失敗なら `SaveFailed` にする。

use std::io::{self, Write};
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

/// rename の共有違反に対する再試行回数と間隔（ミリ秒）。
pub const RENAME_RETRIES: u32 = 5;
pub const RENAME_RETRY_INTERVAL_MS: u64 = 50;

static TMP_COUNTER: AtomicU64 = AtomicU64::new(0);

pub fn write_atomic(path: &Path, bytes: &[u8]) -> io::Result<()> {
    let dir = path.parent().filter(|d| !d.as_os_str().is_empty()).ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "path has no parent"))?;
    let name = path.file_name().ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "path has no file name"))?;
    std::fs::create_dir_all(dir)?;
    let mut tmp_name = name.to_os_string();
    tmp_name.push(format!(".tmp-{}-{}", std::process::id(), TMP_COUNTER.fetch_add(1, Ordering::SeqCst)));
    let tmp = dir.join(tmp_name);
    let result = (|| {
        let mut f = std::fs::OpenOptions::new().write(true).create_new(true).open(&tmp)?;
        f.write_all(bytes)?;
        f.sync_all()?;
        drop(f);
        rename_with_retry(&tmp, path)
    })();
    if result.is_err() {
        // 元のファイルには触れていない。一時ファイルだけ片付ける。
        let _ = std::fs::remove_file(&tmp);
    }
    result
}

fn rename_with_retry(from: &Path, to: &Path) -> io::Result<()> {
    let mut last = None;
    for attempt in 0..=RENAME_RETRIES {
        match std::fs::rename(from, to) {
            Ok(()) => return Ok(()),
            // 元ファイルが無い等は再試行しても変わらない。
            Err(e) if e.kind() == io::ErrorKind::NotFound => return Err(e),
            Err(e) => last = Some(e),
        }
        if attempt < RENAME_RETRIES {
            std::thread::sleep(Duration::from_millis(RENAME_RETRY_INTERVAL_MS));
        }
    }
    Err(last.unwrap_or_else(|| io::Error::other("rename failed")))
}

/// 1行を追記して flush する（JSONL）。行末の `\n` はこの関数が付ける。
pub fn append_line(path: &Path, line: &str) -> io::Result<()> {
    if let Some(dir) = path.parent().filter(|d| !d.as_os_str().is_empty()) {
        std::fs::create_dir_all(dir)?;
    }
    // 前回の異常終了で最終行が途中で切れていたら、改行を補う（次の行と混ざって一緒に読めなくならないように）。
    let needs_break = match std::fs::File::open(path) {
        Ok(mut r) => {
            use std::io::{Read, Seek, SeekFrom};
            let len = r.metadata()?.len();
            if len == 0 {
                false
            } else {
                r.seek(SeekFrom::Start(len - 1))?;
                let mut last = [0u8; 1];
                r.read_exact(&mut last)?;
                last[0] != b'\n'
            }
        }
        Err(e) if e.kind() == io::ErrorKind::NotFound => false,
        Err(e) => return Err(e),
    };
    let mut f = std::fs::OpenOptions::new().create(true).append(true).open(path)?;
    // 1回の書込みにまとめ、行の途中だけが残る可能性を減らす。
    let mut buf = String::with_capacity(line.len() + 2);
    if needs_break {
        buf.push('\n');
    }
    buf.push_str(line);
    buf.push('\n');
    f.write_all(buf.as_bytes())?;
    f.flush()?;
    f.sync_data()
}

/// パスのあるボリュームの空き（呼出し元が使える量）。Windowsでは `GetDiskFreeSpaceExW`（windows-sys）。
/// パスがまだ無ければ、存在する最も近い親で調べる。
pub fn free_space(path: &Path) -> io::Result<u64> {
    let mut probe = path;
    while !probe.exists() {
        match probe.parent() {
            Some(p) if !p.as_os_str().is_empty() => probe = p,
            _ => break,
        }
    }
    free_space_of(probe)
}

#[cfg(windows)]
fn free_space_of(path: &Path) -> io::Result<u64> {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::Storage::FileSystem::GetDiskFreeSpaceExW;
    let wide: Vec<u16> = path.as_os_str().encode_wide().chain(std::iter::once(0)).collect();
    let mut available: u64 = 0;
    // SAFETY: `wide` はNUL終端のUTF-16。出力は有効な `u64` へのポインタ（総量・空き総量は不要なのでNULL）。
    let ok = unsafe { GetDiskFreeSpaceExW(wide.as_ptr(), &mut available, std::ptr::null_mut(), std::ptr::null_mut()) };
    if ok == 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(available)
}

#[cfg(not(windows))]
fn free_space_of(_path: &Path) -> io::Result<u64> {
    Err(io::Error::new(io::ErrorKind::Unsupported, "free space query is Windows-only"))
}

/// ディレクトリ配下の合計サイズ。読めなかったパスを別に返す（0で代用しない）。
/// パスが無ければ 0（読めなかったのではない）。シンボリックリンク・ジャンクションは辿らない（領域の外を数えない）。
/// ファイルならそのサイズ。
pub fn dir_size(path: &Path) -> (u64, Vec<String>) {
    let mut total = 0u64;
    let mut unreadable = Vec::new();
    let meta = match std::fs::symlink_metadata(path) {
        Ok(m) => m,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return (0, unreadable),
        Err(_) => {
            unreadable.push(path.to_string_lossy().into_owned());
            return (0, unreadable);
        }
    };
    if meta.file_type().is_symlink() {
        return (0, unreadable);
    }
    if meta.is_file() {
        return (meta.len(), unreadable);
    }
    match std::fs::read_dir(path) {
        Ok(rd) => {
            for e in rd {
                match e {
                    Ok(e) => {
                        let (n, mut u) = dir_size(&e.path());
                        total = total.saturating_add(n);
                        unreadable.append(&mut u);
                    }
                    Err(_) => unreadable.push(path.to_string_lossy().into_owned()),
                }
            }
        }
        Err(_) => unreadable.push(path.to_string_lossy().into_owned()),
    }
    (total, unreadable)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn append_after_a_torn_last_line_starts_on_a_new_line() {
        let d = std::env::temp_dir().join(format!("agentdock-append-torn-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        let p = d.join("x.jsonl");
        append_line(&p, "{\"a\":1}").unwrap();
        std::fs::write(&p, b"{\"a\":1}\n{\"torn\":").unwrap();
        append_line(&p, "{\"b\":2}").unwrap();
        let text = std::fs::read_to_string(&p).unwrap();
        assert_eq!(text, "{\"a\":1}\n{\"torn\":\n{\"b\":2}\n");
        append_line(&p, "{\"c\":3}").unwrap();
        assert!(std::fs::read_to_string(&p).unwrap().ends_with("{\"b\":2}\n{\"c\":3}\n"), "a complete last line gets no extra blank line");
        let _ = std::fs::remove_dir_all(&d);
    }

    fn temp_dir(name: &str) -> std::path::PathBuf {
        let d = std::env::temp_dir().join(format!("agentdock-atomic-{name}-{}-{}", std::process::id(), TMP_COUNTER.fetch_add(1, Ordering::SeqCst)));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    fn names(dir: &Path) -> Vec<String> {
        let mut v: Vec<String> = std::fs::read_dir(dir).unwrap().map(|e| e.unwrap().file_name().to_string_lossy().into_owned()).collect();
        v.sort();
        v
    }

    #[test]
    fn creates_then_replaces_without_leftovers() {
        let dir = temp_dir("replace");
        let file = dir.join("a.json");
        write_atomic(&file, b"one").unwrap();
        assert_eq!(std::fs::read(&file).unwrap(), b"one");
        write_atomic(&file, b"two-two").unwrap();
        assert_eq!(std::fs::read(&file).unwrap(), b"two-two");
        assert_eq!(names(&dir), vec!["a.json"]);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn creates_missing_parent_directories() {
        let dir = temp_dir("parents");
        let file = dir.join("x").join("y").join("a.json");
        write_atomic(&file, b"v").unwrap();
        assert_eq!(std::fs::read(&file).unwrap(), b"v");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 置換できなかったとき、旧内容が残り、一時ファイルも残らない（共有違反を作って確認）。
    #[cfg(windows)]
    #[test]
    fn failed_replace_keeps_old_content() {
        use std::os::windows::fs::OpenOptionsExt;
        let dir = temp_dir("fail");
        let file = dir.join("a.json");
        write_atomic(&file, b"old").unwrap();
        // 共有なしで開いたままにすると、rename（置換）は共有違反で失敗する。
        let held = std::fs::OpenOptions::new().read(true).share_mode(0).open(&file).unwrap();
        let r = write_atomic(&file, b"new");
        assert!(r.is_err(), "replace must fail while the file is held open exclusively");
        drop(held);
        assert_eq!(std::fs::read(&file).unwrap(), b"old");
        assert_eq!(names(&dir), vec!["a.json"], "temp file must be removed");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn append_adds_newline_terminated_lines() {
        let dir = temp_dir("append");
        let file = dir.join("a.jsonl");
        append_line(&file, "{\"a\":1}").unwrap();
        append_line(&file, "{\"a\":2}").unwrap();
        assert_eq!(std::fs::read_to_string(&file).unwrap(), "{\"a\":1}\n{\"a\":2}\n");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn dir_size_sums_nested_files_and_treats_a_missing_path_as_zero() {
        let dir = temp_dir("size");
        std::fs::create_dir_all(dir.join("a").join("b")).unwrap();
        std::fs::write(dir.join("x.bin"), [0u8; 10]).unwrap();
        std::fs::write(dir.join("a").join("y.bin"), [0u8; 5]).unwrap();
        std::fs::write(dir.join("a").join("b").join("z.bin"), [0u8; 1]).unwrap();
        assert_eq!(dir_size(&dir), (16, vec![]));
        assert_eq!(dir_size(&dir.join("x.bin")), (10, vec![]));
        assert_eq!(dir_size(&dir.join("none")), (0, vec![]));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[cfg(windows)]
    #[test]
    fn free_space_is_reported_even_for_a_path_that_does_not_exist_yet() {
        let dir = temp_dir("space");
        let n = free_space(&dir.join("not").join("yet")).unwrap();
        assert!(n > 0);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
