//! Markdownエクスポート（P7、M41）。純粋ロジック。
//!
//! - 会話本文（Codexの `read` で取得したturn）と、添付・成果物のファイル名を出す。ファイルの中身は含めない。
//! - 「監視活動履歴も含める」なら、保存済みの監視活動（子・孫を含む）を末尾に追加する。
//! - 取得できなかった部分は「未取得」と書き、空で代用しない。

use crate::backend::local::{ArtifactEntry, AttachmentEntry};
use crate::backend::model::*;
use crate::store::records::ActivityLine;

pub struct ExportInput<'a> {
    pub chat_name: Known<String>,
    pub turns: &'a [TurnRecord],
    /// 本文が完全に取れたか（`TurnRecord.complete` の集約）。false なら冒頭に注記する。
    pub transcript_complete: bool,
    pub attachments: &'a [AttachmentEntry],
    pub artifacts: &'a [ArtifactEntry],
    /// None＝監視活動を含めない。
    pub monitor_activity: Option<&'a [ActivityLine]>,
    pub exported_at_local: String,
}

pub fn render_markdown(input: &ExportInput<'_>) -> String {
    let _ = input;
    todo!("P7")
}

/// 本文中のバッククォートの最長連続より長いフェンスを選ぶ（コードを壊さない）。
pub fn fence_for(text: &str) -> String {
    let _ = text;
    todo!("P7")
}
