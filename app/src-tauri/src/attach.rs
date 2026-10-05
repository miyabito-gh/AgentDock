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

use std::path::Path;

use crate::backend::local::*;
use crate::backend::model::*;

#[derive(Debug, thiserror::Error)]
pub enum AttachError {
    #[error("folder is a workspace, not an attachment")]
    FolderIsWorkspace,
    #[error("insufficient space: required {required}, available {available}")]
    InsufficientSpace { required: u64, available: u64 },
    #[error("copy failed: {0:?}")]
    Copy(CopyFailure),
}

/// 拡張子から種別を決める（画像: png/jpg/jpeg/gif/webp/bmp。音声は段階②では File 扱い）。
pub fn kind_for(path: &Path) -> AttachmentKind {
    let _ = path;
    todo!("P4")
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
    let _ = (chat_dir, chat, id, source, now, on_registered);
    todo!("P4")
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
    let _ = (chat_dir, chat, id, display_name, bytes, now, on_registered);
    todo!("P4")
}

/// 送信に使える形へ変換する。`Ready` 以外は `Err(id)`（送信を止めて案内する）。
pub fn to_send_attachments(entries: &[AttachmentEntry]) -> Result<Vec<Attachment>, LocalId> {
    let _ = entries;
    todo!("P4")
}

/// 実在確認（会話を開いたとき・開く／保存の直前）。読めなければ `Missing`、権限等で判定不能なら NotFetched。
pub fn check_exists(path: &Path) -> Known<bool> {
    let _ = path;
    todo!("P4")
}

/// 名前を付けて保存。`dest` が存在し `overwrite_confirmed=false` なら `Ok(false)` を返し、書かない（無断上書きしない）。
/// 書くときは `dest` と同じフォルダの一時ファイルへコピーしてから置換する。
pub fn save_copy_as(src: &Path, dest: &Path, overwrite_confirmed: bool) -> std::io::Result<bool> {
    let _ = (src, dest, overwrite_confirmed);
    todo!("P4")
}
