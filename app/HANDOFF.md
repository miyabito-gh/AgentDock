# 現在の引き継ぎ

更新: 2026-10-26。読む順序は README → AGENTS → 本書 → 必要なら MIGRATION_SUBAGENTS。過去の詳細記録は [docs/archive/HANDOFF_visual_editor_2026-10.md](../docs/archive/HANDOFF_visual_editor_2026-10.md)（旧版全文。必要な日付の節だけ検索して読む。全文は読まない）。旧々版は docs/archive/HANDOFF_pre_subagent_migration.md。

## 現在地
- ブランチ: `feature/visual-editor`。最新の保存コミットは eeec475（リリースビルド記録）。master/origin/master は 09b00e3 時点。
- 新方式（親Codex＋サブエージェント）と Visual Editor は実装済み。ソース確認・関連テストは通過しているが、実Codexによる一連の受入と、最近の実機フィードバック対応（今の会話で続ける、右ペイン幅、編集の破棄）は**実機未確認**。
- Visual Editor の現状: 中央に既存チャット、右ペインに編集操作＋HTMLプレビュー。要素選択（クリック／範囲／親へ拡張）、編集用コピーでAIが編集、差分確認、直前のAI編集の取り消し、利用者操作での元への反映、編集の破棄を実装。未実装: 文字の直接編集、設定パネル、選択付近の依頼入力。
- 元への反映後に、元ファイルを反映前へ戻す製品UIは未実装（バックアップは保存されるのみ）。
- 直近リリースビルド: `app/src-tauri/target/release/bundle/nsis/AgentDock_0.0.1_x64-setup.exe`（SHA-256 f4ebafa235f4e8a4e245791dbcd2d96b89445f770a66707bd4b53c688ef6749d、インストール未実施）。

## 次の作業
- 方針の正本は [VISUAL_EDITOR_UX_DISCUSSION.md](VISUAL_EDITOR_UX_DISCUSSION.md) の §8（2026-10-26 合意。実装は未着手）。順序: 計測と土台 → 選択バーとチャット整理 → ドロワー化 → 文字の仮編集。色・サイズ・余白パネルは保留。
- 実装前に、対象ファイル・受入条件・確認コマンドを決める（AGENTS.md）。

## 既知の注意
- Codex観測版 0.163.0、TARGET_VERSION は 0.162.1。未検証で警告を消していない。
- 旧 org 実行は有効で、L1退役は未着手。
- 通常実行の `npm run tauri -- build` は esbuild のアクセス拒否で失敗することがあり、昇格実行で再試行した。
- Codex単体試験の会話（cxprobe-215824\copy など）が一覧に残っている可能性がある。~/.codex は変更していない。不要なら AgentDock の削除機能で消してよい。
- 日本語記録を PowerShell→Python 標準入力で書くと文字化けした実績がある。ファイル編集は Edit/Write を使う。

## 検証コマンド
- `cargo check --manifest-path app/src-tauri/Cargo.toml --locked`
- `cargo test --manifest-path app/src-tauri/Cargo.toml --lib visual --locked`（Visual Editor関連）
- app/ で `npm.cmd run type-check` と `npm.cmd run build`

## 未コミット・稼働状況
- 未コミット: `app/HANDOFF.md`、`app/VISUAL_EDITOR_UX_DISCUSSION.md`（新規）、`docs/archive/HANDOFF_visual_editor_2026-10.md`（新規）、`AGENTS.md`・`AgentDock_Requirements.md`（読み順・索引の更新）。`app/src-tauri/gen/schemas/*.json` は既存の変更で保持（触れていない）。
- 稼働中プロセス: なし。コミット・pushはしていない。
