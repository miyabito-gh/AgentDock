//! 段階②の純粋ロジック（時刻・状態を引数で受け取り、I/Oをしない）。単体テストはここにだけ書く。
//!
//! - [`queue`]: 完了後キューの自動送信判断（§3.7）
//! - [`notify`]: 通知の2秒集約・重複防止・抑制（§3.11、D03）
//! - [`manage`]: 削除保留・アーカイブ反映・完全終了・wake（§3.4・§3.6・§3.11）
//! - [`window`]: 窓位置の画面外補正（M37）
//! - [`export`]: Markdownエクスポートの組立て（M41）
//! - [`diff`]・[`baseline`]: 統一diffの分割・status解析と、Git基準の控えによる変更を戻す判定（段階③ P3-1、P3B）
//! - [`thread_ops`]: レビュー対象の可否・分岐の照合・受理不明の操作の照合（段階③ P3-2）
//! - [`worktree`]: worktree一覧の解析・名前の組立て・削除可否の判定（段階③ P3-7）
//! - [`work_folders`]: 作業フォルダ（複数）の正規化・拒否・強い確認の判定（項目39 WF-1）
//! - [`org`]: 組織実行のsandbox健全性の判定とconfig.tomlのtrust追記の分類（P-ORG-4c）
//! - [`org_interject`]・[`org_timeline`]: 組織実行の割り込み指示の経路の判定、進行タイムラインの整形（DESIGN_ORG_R2）
//! - [`org_manage`]: 組織ワークフローの削除・アーカイブの可否と削除の手順（DESIGN_ORG_DELETE）
//! - [`org_worktree`]: 組織実行のworktreeの所有確認・ブランチ名・コミット間差分の解析（P-ORG-5）
//! - [`org_artifacts`]: 組織実行の成果物パネルと確認つきの取り込みの判定・解析・台帳（DESIGN_ORG_R3）
//! - [`continuity`]: 継続チャットの資料の組立て・使用率の表示区分・引き継ぎの前提・起動時の照合（G-P、DESIGN_CONT）
//! - [`purpose_draft`]: 目的メモの下書き支援の指示文・出力の検査（DESIGN_CONT_DRAFT）
//! - [`org_intake`]・[`git_prep`]・[`git_push`]: 依頼整理の要約・依頼文・状態遷移、Gitの準備、アプリからのプッシュ（DESIGN_ORG_INTAKE）
//! - [`compose`]・[`side`]: 参考ブロックの組立て、side相談の停止範囲・引渡し（段階③ P3-4）

pub mod baseline;
pub mod compose;
pub mod continuity;
pub mod decision;
pub mod diff;
pub mod export;
pub mod git_prep;
pub mod git_push;
pub mod manage;
pub mod notify;
pub mod org;
pub mod org_artifacts;
pub mod org_interject;
pub mod org_intake;
pub mod org_manage;
pub mod org_timeline;
pub mod org_worktree;
pub mod purpose_draft;
pub mod queue;
pub mod side;
pub mod thread_ops;
pub mod window;
pub mod work_folders;
pub mod worktree;
