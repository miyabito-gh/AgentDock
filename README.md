# AgentDock

Windows上でCodex App Serverを操作するTauri 2 + React + Rustクライアント。

## 現在地
masterは09b00e3へ早送りし、origin/masterへのpushを確認済み（2026-10-25）。通常改善と新サブエージェント方式を含む。実機受入の実績と未確認条件はapp/HANDOFF.mdを参照。旧組織実行は有効なまま。旧基準fbe67ca / baseline/org-engine-2026-10-24は履歴として保持する。

開発ブランチはfeature/visual-editor。明示したファイルの分離コピー・控え・競合判定のRust基盤と、WebView2検証用exampleを実装。関連Rust試験7件・通常cargo check・exampleビルドは成功。実機probeは厳格なTCP遮断に不合格で、製品UI/AI連携/採用処理は未実装・未公開。[設計と受入条件](app/DESIGN_VISUAL_EDITOR.md)を参照。

## 最初に読む資料
1. [AI共通指示](AGENTS.md)
2. [現在の引き継ぎ](app/HANDOFF.md)
3. [移行設計と受入条件](app/MIGRATION_SUBAGENTS.md)
4. [要件の正本](AgentDock_Requirements.md)の必要な節

旧組織実行の設計・検証資料は旧方式の資料として保持する。app/DESIGN_ORG_TRIAGE.md等の提案を実装済みと扱わない。app/DEFERRED.md、app/IMPROVEMENTS.mdは過去の状態も含むため、コードと後続コミットを照合する。旧引き継ぎは docs/archive/ に保存。

## ビルド
基準版のpackage-lock.jsonとCargo.lockを40f3ae4でGit管理した。PowerShellで app/ に移動し npm.cmd ci、npm.cmd run build。Rustはリポジトリ直下で cargo check --manifest-path app/src-tauri/Cargo.toml --locked。デスクトップ起動は app/ で npm.cmd run tauri -- dev。必要なNode/Rust/Tauri/Windowsビルド環境は各コマンドの要求を満たすものを使用する。

## データと互換性
ソースのcloneは保存データを分離しない。%LOCALAPPDATA%\com.agentdock.app に設定、下書き、キュー、活動記録、添付、控え、worktreeと組織履歴がある。Codexの設定・認証・会話は既存の ~/.codex を利用する。
設定・通常会話を引き継ぐ。保存形式の変更前は停止確認とバックアップを行い、旧版との同時書込みを避ける。旧組織履歴は自動変換・削除しない。

## 検証状態
基準ソースで cargo check --locked と npm.cmd run build は成功。既存インストーラーはファイル時刻だけでコミット一致を保証できないため、保存時のハッシュと来歴を .local-baseline/manifest.json に記録する（ローカルのみ）。現在の実装・確認結果の詳細はHANDOFFを参照。
