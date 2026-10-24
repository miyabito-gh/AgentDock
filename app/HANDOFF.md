# 現在の引き継ぎ

更新: 2026-10-24。通常機能改善と新方式のソース実装・関連検証・独立レビューを保存済み。実Codexによる受入は未実施。README → AGENTS → 本書 → MIGRATION_SUBAGENTSの順に読む。

## 保存地点
- 旧版はfbe67ca。masterとorigin/masterへ保存済み。タグbaseline/org-engine-2026-10-24もpush済み。旧フォルダと既存worktreeを保持。
- 作業場所はAgentDock_next、ブランチfeature/subagent-migration。新ブランチは未push。
- bdcd23f: 共通指示。40f3ae4: 基準版依存lock。0a136a1: 通常機能U1〜U5。3ac8b9f: 新方式。d7a4c28: 検証記録。

## 検証
- 通常改善: Rust8件、画面build（tsc含む）成功。Luna静的確認で回帰指摘なし。
- 新方式: cargo test --manifest-path app/src-tauri/Cargo.toml --lib subagent --locked は16件成功。型8件・host安全性4件・既存関連4件を含む。一時Gitで検証。
- Solの初回レビューで受理ガードの期限失効/照合Accepted未記録と、取り込み先の別チャットとの競合を検出。Sol利用上限後はLunaが限定修正。
- 受理turnの終端までガードを維持し、取り込み先と重なる全チャットの送信ロック・未確定操作・停止を検査。初回送信・review/compactもガードへ参加。通常キューの15秒方針は維持。
- 追加回帰2件、最終cargo check、最終画面build成功。別Lunaの独立再レビューで追加必須修正なし。
- 詳細はNORMAL_IMPROVEMENTS.md、SUBAGENT_IMPLEMENTATION.md、SUBAGENT_UI.md。GUI/外部アプリ/実Codexの受入は未実施。旧orgは有効でL1退役は未着手。

## 残りの受入と環境
- 専用ローカルrepoで子生成、追加指示、承認/質問、親子停止、再起動後照合、成果取り込みを確認する。Astraは使わない。
- 実Codex利用量・会話作成の許可は明示回答未確認。旧アプリの作業停止・終了を確認してからバックアップし、新旧同時書込みを避ける。
- Codex観測版0.163.0、TARGET_VERSIONは0.162.1。未検証で警告を消していない。
- 設定・認証・通常履歴は継承。~/.codexをコピー/変更していない。実アプリ保存領域への書込み試験は未実施。
- 旧exe/NSISは.local-baseline/にハッシュ付き保存。基準ソース一致は未検証。
- .git書込み許可指定後も通常実行のindex.lockが拒否された。必要時は昇格経路を使用し、制限解除を断定しない。

## 配布物
- NSIS x64を作成済み。終了コード0。詳細と実行コマンドはINSTALLER_BUILD.md。インストーラー実行は利用者が行う。
- app/で npm.cmd run tauri -- build --bundles nsis。通常実行はesbuildのアクセス拒否で失敗し、昇格実行で再試行。
- 前回のPowerShell→Python標準入力で文字コード変換が起き、日本語記録の一部が「?」になったため修復。アプリソースは変更していない。
