//! 添付のコピーと成果物の確認（P4、要件§3.8、M31・M32）。
//!
//! コピー手順（不完全なコピーを利用可能にしない）:
//! 1. 元がフォルダなら止める（`FolderIsWorkspace`。作業フォルダの指定として扱い、中身をコピーしない）
//! 2. サイズを取得し、`Store::check_space(size)` で空きを確認（不足なら `InsufficientSpace` で止める）
//! 3. 台帳に `Copying` で登録して保存（落ちても再起動時に検出できるように）
//! 4. `attachments\<attId>\<name>.partial` へ `create_new` で書き、`sync_all`、サイズ照合
//! 5. `.partial` を最終名へ rename し、台帳を `Ready` にして保存
//! 6. 途中で失敗したら `.partial` を消し、台帳を `CopyFailed{reason}` にする（元ファイルには触れない）
//!
//! 送信時は `Ready` の添付だけを `model::Attachment`（kind＋copy_path）に変換してバックエンドへ渡す。
//! UI・保存形式にはCodexの `UserInput` を出さない（§2.3）。Codexアダプターでの変換は DESIGN_P2 §3.3。
//!
//! 手順1〜3で止まったときは台帳に何も作らず `Err` を返す。手順4以降の失敗は台帳に残すため、
//! `Ok(entry)`（`state` が `CopyFailed`）を返す。呼び出し側は戻り値の `state` を見て扱う。

use std::io::{Read, Write};
use std::path::Path;

use crate::backend::local::*;
use crate::backend::model::*;
use crate::store::{atomic, layout, space_check, StoreError};

#[derive(Debug, thiserror::Error)]
pub enum AttachError {
    #[error("folder is a workspace, not an attachment")]
    FolderIsWorkspace,
    #[error("insufficient space: required {required}, available {available}")]
    InsufficientSpace { required: u64, available: u64 },
    #[error("copy failed: {0:?}")]
    Copy(CopyFailure),
    /// 台帳への登録（手順3の保存）に失敗した。コピーは始めていない。
    #[error("could not record the attachment: {0}")]
    Register(String),
}

const IMAGE_EXTENSIONS: [&str; 6] = ["png", "jpg", "jpeg", "gif", "webp", "bmp"];

/// 拡張子から種別を決める（画像: png/jpg/jpeg/gif/webp/bmp。音声は段階②では File 扱い）。
pub fn kind_for(path: &Path) -> AttachmentKind {
    let ext = path.extension().map(|e| e.to_string_lossy().to_ascii_lowercase());
    match ext {
        Some(e) if IMAGE_EXTENSIONS.contains(&e.as_str()) => AttachmentKind::Image,
        _ => AttachmentKind::File,
    }
}

fn check_space(chat_dir: &Path, size: u64) -> Result<(), AttachError> {
    // 空きを取得できないときは止めない（書込み自体が失敗すればその結果を報告する）。
    match atomic::free_space(chat_dir).map(|available| space_check(size, available)) {
        Ok(Err(StoreError::InsufficientSpace { required, available })) => Err(AttachError::InsufficientSpace { required, available }),
        _ => Ok(()),
    }
}

fn failed(entry: &mut AttachmentEntry, reason: CopyFailure, message: impl Into<String>) {
    entry.state = AttachmentState::CopyFailed { reason, message: message.into() };
    entry.copy_path = None;
}

/// 手順4・5・6。`write` は `.partial` の書込み先を受け取る。書いたサイズの照合はここで行う。
/// 失敗したら `.partial` を消して `CopyFailed` にする（最終名のファイルは作らない）。
fn finish_copy(chat_dir: &Path, mut entry: AttachmentEntry, expected: u64, write: impl FnOnce(&mut std::fs::File) -> Result<(), CopyFailure>) -> AttachmentEntry {
    let (final_path, partial) = layout::attachment_paths(chat_dir, &entry.id, &entry.display_name);
    let run = || -> Result<(), (CopyFailure, String)> {
        if let Some(dir) = partial.parent() {
            std::fs::create_dir_all(dir).map_err(|e| (CopyFailure::WriteFailed, format!("コピー先を作れません: {e}")))?;
        }
        let mut f = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&partial)
            .map_err(|e| (CopyFailure::WriteFailed, format!("コピー先を開けません: {e}")))?;
        write(&mut f).map_err(|r| {
            let m = match r {
                CopyFailure::SourceUnreadable => "元ファイルを読めませんでした".to_string(),
                _ => "コピー先へ書き込めませんでした（空き容量が足りない可能性があります）".to_string(),
            };
            (r, m)
        })?;
        f.sync_all().map_err(|e| (CopyFailure::WriteFailed, format!("コピーを確定できません: {e}")))?;
        drop(f);
        let len = std::fs::metadata(&partial).map_err(|e| (CopyFailure::WriteFailed, format!("コピーを確認できません: {e}")))?.len();
        if len != expected {
            return Err((CopyFailure::SizeMismatch, format!("コピーしたサイズが一致しません（期待 {expected} バイト、実際 {len} バイト）")));
        }
        std::fs::rename(&partial, &final_path).map_err(|e| (CopyFailure::WriteFailed, format!("コピーを確定できません: {e}")))?;
        Ok(())
    };
    match run() {
        Ok(()) => {
            entry.copy_path = Some(final_path.to_string_lossy().into_owned());
            entry.state = AttachmentState::Ready;
        }
        Err((reason, message)) => {
            let _ = std::fs::remove_file(&partial);
            failed(&mut entry, reason, message);
        }
    }
    entry
}

/// ファイルをチャット領域へコピーする（上の手順1〜6）。`on_registered` は手順3の保存を行うコールバック。
pub fn copy_file_into(
    chat_dir: &Path,
    chat: &ChatKey,
    id: LocalId,
    source: &Path,
    now: UnixMillis,
    on_registered: &mut dyn FnMut(&AttachmentEntry) -> Result<(), crate::store::StoreError>,
) -> Result<AttachmentEntry, AttachError> {
    let meta = std::fs::metadata(source).map_err(|_| AttachError::Copy(CopyFailure::SourceUnreadable))?;
    if meta.is_dir() {
        return Err(AttachError::FolderIsWorkspace);
    }
    let size = meta.len();
    check_space(chat_dir, size)?;
    let display_name = source.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| "file".into());
    let entry = AttachmentEntry {
        id,
        chat: chat.clone(),
        kind: kind_for(source),
        display_name,
        source: AttachmentSource::File { original_path: source.to_string_lossy().into_owned() },
        copy_path: None,
        size: Known::direct(size),
        attached_at: now,
        state: AttachmentState::Copying,
        used_by: Vec::new(),
    };
    on_registered(&entry).map_err(|e| AttachError::Register(e.to_string()))?;
    let source = source.to_path_buf();
    Ok(finish_copy(chat_dir, entry, size, move |dst| {
        // 元ファイルは読むだけ。
        let mut src = std::fs::File::open(&source).map_err(|_| CopyFailure::SourceUnreadable)?;
        let mut buf = vec![0u8; 256 * 1024];
        loop {
            let n = src.read(&mut buf).map_err(|_| CopyFailure::SourceUnreadable)?;
            if n == 0 {
                return Ok(());
            }
            dst.write_all(&buf[..n]).map_err(|_| CopyFailure::WriteFailed)?;
        }
    }))
}

/// クリップボード画像（UIから生バイトで受けたPNG）を保存する。手順は同じ。
pub fn write_image_into(
    chat_dir: &Path,
    chat: &ChatKey,
    id: LocalId,
    display_name: &str,
    bytes: &[u8],
    now: UnixMillis,
    on_registered: &mut dyn FnMut(&AttachmentEntry) -> Result<(), crate::store::StoreError>,
) -> Result<AttachmentEntry, AttachError> {
    let size = bytes.len() as u64;
    check_space(chat_dir, size)?;
    let entry = AttachmentEntry {
        id,
        chat: chat.clone(),
        kind: AttachmentKind::Image,
        display_name: layout::sanitize_file_name(display_name),
        source: AttachmentSource::ClipboardImage,
        copy_path: None,
        size: Known::direct(size),
        attached_at: now,
        state: AttachmentState::Copying,
        used_by: Vec::new(),
    };
    on_registered(&entry).map_err(|e| AttachError::Register(e.to_string()))?;
    Ok(finish_copy(chat_dir, entry, size, |dst| dst.write_all(bytes).map_err(|_| CopyFailure::WriteFailed)))
}

/// 送信に使える形へ変換する。`Ready` 以外は `Err(id)`（送信を止めて案内する）。
pub fn to_send_attachments(entries: &[AttachmentEntry]) -> Result<Vec<Attachment>, LocalId> {
    entries
        .iter()
        .map(|e| match (&e.state, &e.copy_path) {
            (AttachmentState::Ready, Some(copy)) => Ok(Attachment {
                id: e.id.clone(),
                kind: e.kind,
                // クリップボード画像には元ファイルがないので、コピーのパスを入れる（送信にはコピーだけを使う）。
                original_path: match &e.source {
                    AttachmentSource::File { original_path } => original_path.clone(),
                    AttachmentSource::ClipboardImage => copy.clone(),
                },
                copy_path: Some(copy.clone()),
                attached_at: e.attached_at,
                exists: Known::NotFetched,
                owner_chat: e.chat.clone(),
            }),
            _ => Err(e.id.clone()),
        })
        .collect()
}

/// 実在確認（会話を開いたとき・開く／保存の直前）。ないと確認できたら `false`、権限等で判定不能なら NotFetched。
pub fn check_exists(path: &Path) -> Known<bool> {
    match path.try_exists() {
        Ok(v) => Known::direct(v),
        Err(_) => Known::NotFetched,
    }
}

/// 名前を付けて保存。`dest` が存在し `overwrite_confirmed=false` なら `Ok(false)` を返し、書かない（無断上書きしない）。
/// 書くときは `dest` と同じフォルダの一時ファイルへコピーしてから置換する。
pub fn save_copy_as(src: &Path, dest: &Path, overwrite_confirmed: bool) -> std::io::Result<bool> {
    if !overwrite_confirmed && dest.try_exists()? {
        return Ok(false);
    }
    let dir = dest.parent().filter(|p| !p.as_os_str().is_empty()).unwrap_or_else(|| Path::new("."));
    let name = dest.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| "file".into());
    let tmp = dir.join(format!(".{name}.agentdock-{}.tmp", std::process::id()));
    let copied = (|| -> std::io::Result<()> {
        let mut s = std::fs::File::open(src)?;
        let mut d = std::fs::OpenOptions::new().write(true).create(true).truncate(true).open(&tmp)?;
        std::io::copy(&mut s, &mut d)?;
        d.sync_all()?;
        Ok(())
    })();
    if let Err(e) = copied {
        let _ = std::fs::remove_file(&tmp);
        return Err(e);
    }
    // コピー中に同名ファイルが現れていたら、確認なしでは置き換えない。
    if !overwrite_confirmed && dest.try_exists()? {
        let _ = std::fs::remove_file(&tmp);
        return Ok(false);
    }
    if let Err(e) = std::fs::rename(&tmp, dest) {
        let _ = std::fs::remove_file(&tmp);
        return Err(e);
    }
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};

    fn temp_dir(name: &str) -> std::path::PathBuf {
        static N: AtomicU64 = AtomicU64::new(0);
        let d = std::env::temp_dir().join(format!("agentdock-attach-{name}-{}-{}", std::process::id(), N.fetch_add(1, Ordering::SeqCst)));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    fn key() -> ChatKey {
        ChatKey { backend: BackendKind::Codex, id: ExternalId("t1".into()) }
    }

    fn entry(id: &str, state: AttachmentState, copy: Option<&str>) -> AttachmentEntry {
        AttachmentEntry {
            id: LocalId(id.into()),
            chat: key(),
            kind: AttachmentKind::File,
            display_name: "a.txt".into(),
            source: AttachmentSource::File { original_path: r"C:\src\a.txt".into() },
            copy_path: copy.map(str::to_string),
            size: Known::direct(1),
            attached_at: UnixMillis(5),
            state,
            used_by: vec![],
        }
    }

    #[test]
    fn kind_is_decided_by_extension() {
        assert_eq!(kind_for(Path::new("a.PNG")), AttachmentKind::Image);
        assert_eq!(kind_for(Path::new("a.jpeg")), AttachmentKind::Image);
        assert_eq!(kind_for(Path::new("a.webp")), AttachmentKind::Image);
        assert_eq!(kind_for(Path::new("a.pdf")), AttachmentKind::File);
        assert_eq!(kind_for(Path::new("a.mp3")), AttachmentKind::File);
        assert_eq!(kind_for(Path::new("noext")), AttachmentKind::File);
    }

    #[test]
    fn only_ready_attachments_can_be_sent() {
        let ready = entry("a1", AttachmentState::Ready, Some(r"C:\c\a.txt"));
        let out = to_send_attachments(std::slice::from_ref(&ready)).unwrap();
        assert_eq!(out[0].copy_path.as_deref(), Some(r"C:\c\a.txt"));
        assert_eq!(out[0].original_path, r"C:\src\a.txt");
        for bad in [
            entry("a2", AttachmentState::Copying, None),
            entry("a3", AttachmentState::Missing, None),
            entry("a4", AttachmentState::CopyFailed { reason: CopyFailure::WriteFailed, message: "m".into() }, None),
            entry("a5", AttachmentState::Ready, None),
        ] {
            let id = bad.id.clone();
            assert_eq!(to_send_attachments(&[ready.clone(), bad]).unwrap_err(), id);
        }
    }

    #[test]
    fn copy_succeeds_and_leaves_no_partial() {
        let root = temp_dir("ok");
        let src = root.join("src.txt");
        std::fs::write(&src, b"hello").unwrap();
        let chat_dir = root.join("chat");
        let mut registered = Vec::new();
        let e = copy_file_into(&chat_dir, &key(), LocalId("att-1".into()), &src, UnixMillis(9), &mut |e| {
            registered.push(e.state.clone());
            Ok(())
        })
        .unwrap();
        assert_eq!(registered, vec![AttachmentState::Copying]);
        assert_eq!(e.state, AttachmentState::Ready);
        let copy = std::path::PathBuf::from(e.copy_path.clone().unwrap());
        assert_eq!(std::fs::read(&copy).unwrap(), b"hello");
        assert!(!copy.with_file_name("src.txt.partial").exists());
        assert_eq!(std::fs::read(&src).unwrap(), b"hello", "the original is untouched");
        // 同じファイルをもう一度付けると別コピー。
        let e2 = copy_file_into(&chat_dir, &key(), LocalId("att-2".into()), &src, UnixMillis(10), &mut |_| Ok(())).unwrap();
        assert_ne!(e.copy_path, e2.copy_path);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn folders_are_refused_without_a_ledger_entry() {
        let root = temp_dir("folder");
        let mut called = false;
        let r = copy_file_into(&root, &key(), LocalId("att-1".into()), &root, UnixMillis(1), &mut |_| {
            called = true;
            Ok(())
        });
        assert!(matches!(r, Err(AttachError::FolderIsWorkspace)));
        assert!(!called);
        assert!(!root.join("attachments").exists());
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn missing_source_is_refused() {
        let root = temp_dir("nosrc");
        let r = copy_file_into(&root, &key(), LocalId("att-1".into()), &root.join("nope.txt"), UnixMillis(1), &mut |_| Ok(()));
        assert!(matches!(r, Err(AttachError::Copy(CopyFailure::SourceUnreadable))));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn failure_midway_never_becomes_available() {
        let root = temp_dir("fail");
        let e = entry("att-1", AttachmentState::Copying, None);
        let out = finish_copy(&root, e, 10, |dst| {
            dst.write_all(b"half").unwrap();
            Err(CopyFailure::WriteFailed)
        });
        assert!(matches!(out.state, AttachmentState::CopyFailed { reason: CopyFailure::WriteFailed, .. }));
        assert_eq!(out.copy_path, None);
        let (final_path, partial) = layout::attachment_paths(&root, &LocalId("att-1".into()), "a.txt");
        assert!(!final_path.exists() && !partial.exists());
        assert!(to_send_attachments(&[out]).is_err());
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn size_mismatch_is_a_failure() {
        let root = temp_dir("size");
        let e = entry("att-1", AttachmentState::Copying, None);
        let out = finish_copy(&root, e, 10, |dst| dst.write_all(b"abc").map_err(|_| CopyFailure::WriteFailed));
        assert!(matches!(out.state, AttachmentState::CopyFailed { reason: CopyFailure::SizeMismatch, .. }));
        let (final_path, partial) = layout::attachment_paths(&root, &LocalId("att-1".into()), "a.txt");
        assert!(!final_path.exists() && !partial.exists());
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn clipboard_image_is_written_as_a_ready_image() {
        let root = temp_dir("clip");
        let e = write_image_into(&root, &key(), LocalId("att-1".into()), "clipboard-1.png", b"\x89PNG", UnixMillis(1), &mut |_| Ok(())).unwrap();
        assert_eq!((e.kind, e.state.clone()), (AttachmentKind::Image, AttachmentState::Ready));
        assert_eq!(std::fs::read(e.copy_path.unwrap()).unwrap(), b"\x89PNG");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn save_as_does_not_overwrite_without_confirmation() {
        let root = temp_dir("save");
        let (src, dest) = (root.join("s.txt"), root.join("d.txt"));
        std::fs::write(&src, b"new").unwrap();
        std::fs::write(&dest, b"old").unwrap();
        assert!(!save_copy_as(&src, &dest, false).unwrap());
        assert_eq!(std::fs::read(&dest).unwrap(), b"old");
        assert!(save_copy_as(&src, &dest, true).unwrap());
        assert_eq!(std::fs::read(&dest).unwrap(), b"new");
        let fresh = root.join("e.txt");
        assert!(save_copy_as(&src, &fresh, false).unwrap());
        assert_eq!(std::fs::read(&fresh).unwrap(), b"new");
        let mut names: Vec<String> = std::fs::read_dir(&root).unwrap().map(|e| e.unwrap().file_name().to_string_lossy().into_owned()).collect();
        names.sort();
        assert_eq!(names, vec!["d.txt", "e.txt", "s.txt"], "no temp files left");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn existence_check_reports_false_for_missing_paths() {
        let root = temp_dir("exists");
        assert_eq!(check_exists(&root), Known::direct(true));
        assert_eq!(check_exists(&root.join("nope")), Known::direct(false));
        let _ = std::fs::remove_dir_all(&root);
    }
}
