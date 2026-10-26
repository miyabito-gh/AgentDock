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
- 保存済み（未push）: 文書3件（1165c03、f7193d0、4528921）と段階1の試作（8d48569）。未コミットは `app/src-tauri/gen/schemas/*.json` のみ（既存の変更で保持、触れていない）。
- 稼働中プロセス: なし。pushはしていない。

## 2026-10-26 段階1: チャットを畳む試作（実装済み・実機未確認）
- 内容: Visual Editor表示中、上部バーに「チャット」ボタンを追加。畳むと中央列が0になり、右ペイン（プレビュー）が残り幅を使う（`.body.v-chat-off`、`--rw: calc(100% - var(--lw))`、0.16秒の列幅トランジション、reduced-motion時は無効）。チャットは非表示にするだけでアンマウントしない。既定は従来どおり表示。承認・質問のバッジは未実装のため、既定を畳む状態にしていない。
- 検証: type-check・build成功。実機（利用者、HTML未表示の状態）: 畳むとプレビュー領域が左へ広がり、ちらつきなし、戻すと幅も戻る。**HTMLを開いたネイティブWebViewの追従は未確認**。ちらつく場合はトランジションを外す（visual.cssの `.body.visual-mode{transition}` を削除）。
- 実機確認済み（段階1）。続きは下の段階2〜5の節。

## 2026-10-26 段階2〜5（実装済み・自動検証済み・実機未確認）
- 保存済み: d834e51（2a 依頼バー）、1552f64（2b チャット整理）、6ea0c30（3 既定で畳む・バッジ）、d4c5eee（4 文章直し）と、検証指摘の修正コミット。
- 2a: プレビュー下部は、選択があるときだけ出る1行の依頼バー（選択チップ・解除・＋書き出し例・入力・送信）。「場所」ボタンでファイルの場所を開閉。旧「細かく指示する」とセレクタJSONの書き出しは廃止。送信は中央の入力欄と同じ送信不可の判定（sendLockReason等）を通り、送れない理由を表示する。
- 2b: 送信に添える参照情報は {sel（毎回）, fixed（固定の注意書き）}。fixed は会話ごとに内容が変わるまで一度だけ、受理を確認できたときだけ記録して省略する。チャット表示は参照情報を「参照情報（選択N件）を表示」の折りたたみにする。
- 3: 編集を始めるとチャットは既定で畳む。承認・質問、受理されない／確認できない送信が増えたら自動で開き、ボタンにバッジ。
- 4（縮小版）: 子要素のない単一要素は「文章」ボタンで変更後の文章を入力でき、前後の文章をそのままAIへ送る（ページ内の直接編集・仮表示は未実装）。selection.js/rs に leaf を追加（serde default、deny_unknown_fields維持）。
- 自動検証: type-check、build、`cargo test --lib visual` 38件成功。独立検証担当の指摘（送信不可判定の迂回、理由の非表示、組織/クイック画面からの送信、未信頼文字列の扱い、aria重複、狭幅折り返し）は修正済み。
- 実機未確認: 畳み時のWebView幅追従（HTML表示あり、利用者確認済み）、900px未満の崩れ、日本語IMEのEnter、バッジによる自動展開、実Codexでの固定注意書き・文章直しの解釈、依頼バーの見た目。
- 未実装・保留: ページ内の直接文字編集と仮表示、色・サイズ・余白パネル、選択の真横の入力、開始設定の1行化（現状も開始後は状態1行＋操作1行）。
