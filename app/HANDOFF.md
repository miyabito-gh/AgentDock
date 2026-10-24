# 現在の引き継ぎ

更新: 2026-10-24。入口はREADME.mdとAGENTS.md。旧文書は docs/archive/HANDOFF_pre_subagent_migration.md に保存。

## 基準と現在地
- 旧実装の基準: fbe67ca。masterへのfast-forward統合・origin/masterへのpush済み。
- 保存タグ: baseline/org-engine-2026-10-24（push済み）。旧作業フォルダAgentDock_claudeを保持。
- 新フォルダ: C:\Users\wmasa\Documents\Rust\AgentDock_next。ブランチfeature/subagent-migration。
- 現段階はD0（文書整備）。通常機能の修正・新実行方式はまだ未実装。旧組織エンジンは有効なまま。

## 確認済み
- 基準fbe67caで cargo check --manifest-path app/src-tauri/Cargo.toml --locked 成功。
- 基準fbe67caで app/ の npm.cmd run build 成功。既存のchunkサイズと動的/静的import混在警告あり。
- 既存exe/NSISをハッシュ付きで .local-baseline/ へ保存。ソースとの厳密な一致は未確認（基準から再ビルドしたものではない）。Git対象外。

## 次の作業
app/MIGRATION_SUBAGENTS.mdのU1〜U5を個別に確認・改善し、関連検証を実施。その後S1〜S3、新方式受入後にL1。古いDEFERREDの記載はそのまま不具合認定せずコード照合する。
利用者の実機試験許可は会話での提案・説明と明示の範囲を区別し、未確定なら実行前に確認する。

## 取り扱い
設定・認証・通常チャットは引き継ぐ。保存形式を変えるまではデータコピー・バックアップを勝手に実行しない。変更時には停止確認後AgentDock領域をバックアップする。旧組織履歴は保持する。
旧フォルダの未追跡 .claude/launch.json、.claude/worktrees/、error.log は移さず元に保持。23個の既存追加worktreeも変更しない。
本段階ではアプリ・開発サーバーを起動していない。他の既存プロセスの稼働状況は起動前に確認する。
