//! AIバックエンド境界（T1設計）。
//!
//! - [`model`]: バックエンド共通の正規化データモデル（要件§4・§5）。Codex固有型を含めない。
//! - [`backend`]: AIバックエンドtrait（要件§2.3・§3.1〜3.6・§3.9）。Codex App Serverはその実装の一つ（T3）。
//! - [`ipc`]: UI⇔ホストのTauri IPCペイロード（`app/src/ipc/types.ts`と1対1で対応させる）。
//!
//! 設計メモは `app/DESIGN_T1.md`。

pub mod backend;
/// 段階③ P3B-1: 変更の控え（Git基準）の中立型（`app/DESIGN_P3B.md` §2）。
pub mod baseline;
/// 段階③ P3-1: 変更ファイルの一覧・差分・変更を戻す（`app/DESIGN_P3.md` §1 #1・#2）。
pub mod changes;
pub mod ipc;
/// 段階②のアプリ側データとIPC追加分（`app/DESIGN_P2.md`）。
pub mod local;
pub mod model;
/// 段階③: VS Code拡張との同等性の操作のtrait（`app/DESIGN_P3.md` §2.2）。
pub mod parity;
/// 段階③ P3-7: Git worktree（`app/DESIGN_P3.md` §1 #14）のIPC型。`WorktreeRecord` は `model`。
pub mod worktree;
/// 段階③ P3-6: クラウド委任（`app/DESIGN_P3.md` §1 #13）のIPC型。`CloudTaskRecord` は `model`、CLIの出力型は `parity`。
pub mod cloud;
