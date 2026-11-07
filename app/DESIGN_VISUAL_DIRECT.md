# Visual Editor 直接編集方式 設計メモ

作成: 2026-11-07。利用者決定: 「編集用コピー＋元に反映」方式をやめ、**元のファイルを直接編集し、ターンごとに戻せる**方式へ置き換える。過去の要件・旧データの互換は不要（個人利用。旧 `visual-editor` の保存フォルダは利用者が手動で消す）。

## 1. 方針
- 会話は**通常の会話**（中央で選んでいる会話）。編集専用の会話・分岐・「今の会話で続ける」・編集用コピー・反映・結び付けは作らない。
- Visual Editor は「要素を選ぶ → 依頼を会話へ送る（選択・ソース抜粋を添える）→ AI が元のファイルを直接直す → プレビューが更新される」だけ。
- 安全性は「戻せること」で取る。AI へ依頼を送る**直前**に対象ファイルをスナップショットし（失敗したら送らない）、ターンごとに「戻す／やり直す」を出す。
- 完了判定・重なる会話の停止確認・履歴の読み取りによる確認は持たない。

## 2. 削るもの（旧方式）
Rust: `visual_edit/{session,apply,apply_tests,decision,draft,tests}.rs`、`host/visual.rs` の作成・終了・解放・破棄・整理・一覧・状態・反映・取消・差分・ゲート・FastGuard・`visual_before_send`/`after_send`/`mark_restored`/`observe_event`、`host/mod.rs` の呼び出し、`thread_ops.rs`・`work_folders.rs`・`continuity_handoff.rs`・`queue_driver.rs` の visual 判定、`commands.rs`/`lib.rs` の旧 IPC（`visual_edit_create/list/status/end/release/discard/adopt/undo/revert/cleanup/diff/source_hint/draft_stamp`）。残すもの: `scope.rs`（FileScope/RelativePath/ProjectRoot）、`snapshot.rs`、`source_hint.rs`、`preview/*`、`host/visual.rs` の `discover`・`known_access`。
TS: `VisualEditControls.tsx` の編集セッション操作、`App.tsx` の bound/dedicated/origins/notes/peers/終了メモ/切り離し/復元案内、`api.ts` の旧セッション型と関数、`friendlyVisualError` の旧文言、`stampSync` は残す。

## 3. 新しい保存と状態
`<app_data>/visual-editor/direct/<key>/` （`key` = ルートと HTML の相対パスの正規化文字列のハッシュ）。
```
turns.json            ... {nextSeq,turns:[{id,seq,atMs,state,files:[{path,change}],scope,afterScope,chat}]}（原子的に更新。`nextSeq` は単調増加で番号を再利用しない。旧形式の配列も読める）
t-0001/before/<rel>   ... 依頼の直前の対象ファイル（スコープ内の全ファイル）
t-0001/after/<rel>    ... ターン終了時の対象ファイル（turn_end で作る。無くても可）
```
- 対象ファイル = `discover` で得たスコープ（HTML＋関連 CSS/JS 等。edit/readOnly 両方）。スコープ外・AI が新規に作った別ファイルは戻せない（画面に一言出す）。
- `state`: `open`（開始済み・終了前）／`done`／`undone`。`open` のまま残ったターン（アプリ終了など）は、次の open/begin 時に「いまの内容を after として確定」して done にする（変更が無ければ削除）。
- ターンは begin 時の会話（`chat`）を記録する。`open` を確定するのは、その会話が実行中でないとき（停止済み・不明・削除済み）だけ。会話の記録が無い旧ターンは最新の1件だけ。確定に失敗しても begin/open は止めない（保存内容が壊れたターンは破棄、それ以外は open のまま残して diag に記録）。
- `turns.json` が壊れていたら `turns.json.bad` へ退避して新規に始める（diag に記録）。保存量が 400MB を超えたら古いターンから削除（最新の1ターンと open は残す）。
- 変更が無いターンは `turn_end` で削除。最新 30 ターンだけ保持（超過分は古い順に削除）。`undone` が残っている状態で新しい `turn_begin` が来たら、`undone` のターンは捨てる。
- 戻す対象は常に「最新の done（または open）」。やり直し対象は「最も seq が小さい undone」。
- 戻す条件: 現在のファイルが after（無ければ検査しない）と一致。不一致なら `conflict:` で拒否し、`force` 指定のときだけ上書き。対象ターンに記録された会話（旧ターンは渡された `chat`）が実行中なら `running:` で拒否（UI でも無効化）。停止を確認できないだけ（未接続・再起動直後）なら実行中と断定せず許可する。会話が分からない open のターンは拒否。書き込みは一時ファイル＋置換、追加ファイルは削除、パスは `RelativePath`/`ProjectRoot` で検証（リンク・外部への脱出を拒否）。

## 4. IPC 契約（引数は既存と同じく `{ args: {...} }`、JSON は camelCase）
型:
- `DirectFile { path, change: "add"|"change"|"delete" }`
- `DirectTurn { id, seq, atMs, state: "open"|"done"|"undone", files: DirectFile[], canUndo, canRedo }`
- `DirectOpened { rootPath, htmlPath, scope: {path, access:"edit"|"readOnly"}[], turns: DirectTurn[] }`
- `DirectDiff { path, before: string|null, after: string|null, truncated, binary }`

コマンド（すべて `rootPath`, `htmlPath` を取る）:
- `visual_direct_open` → `DirectOpened`（検証・復旧。`open` のターンを確定）
- `visual_direct_turn_begin { chat? }` → `DirectTurn`（スナップショット。失敗したらエラー＝送らない）
- `visual_direct_turn_end { turnId }` → `DirectTurn | null`（変更なしなら null で削除）。冪等
- `visual_direct_turns` → `DirectTurn[]`（読み取りのみ）
- `visual_direct_undo { turnId, force, chat? }` / `visual_direct_redo { turnId, force, chat? }` → `DirectTurn[]`（更新後の一覧）。拒否文言は先頭に `conflict:`（続けて改行区切りのパス）／`running:` ／`nothing:` を付ける
- `visual_direct_stamp` → string（スコープ探索は (root,html) ごとに5秒キャッシュ。探索失敗は空文字でなく日本語のエラー。スコープの編集対象ファイルの path・size・mtime の連結。旧 draft_stamp 相当）
- `visual_direct_source_hint { selection }` → string（旧 source_hint を元ルートの HTML に対して）
- `visual_direct_diff { turnId, path, side? }` → `DirectDiff`（そのターンの before と after／現在）

## 5. UI
- プレビューは元の `rootPath`/`htmlPath` を直接開く（編集用コピー無し）。ファイルの変化（stamp）で自動再読み込み（既存の stampSync を流用）。
- 依頼の送信: `turn_begin` → 成功したら会話へ送信（選択・ソース抜粋・方針を添える。文言は「編集対象: …（元のファイルを直接編集します。ターンごとに戻せます）」）。送信が受理されなかったときは `turn_end`。会話が終了（実行中→停止）で `turn_end`。
- 状態バー（入力欄の近く）: 最新ターンの「N件変更」＋［戻す］［やり直す］［差分］。戻す／やり直しは会話の実行中は無効。conflict は確認ダイアログ（「元のファイルが、AI の編集後に変更されています。それでも戻しますか」）。初回だけ「元のファイルを直接編集します。いつでも戻せます」を 1 行。
- 編集に使う会話 = 中央で選択中の会話。その作業フォルダが編集対象のルートを含まないとき（AI が書けない）は、理由を 1 行出し、「この場所で新しい会話を作る」を出す（既存の新規会話作成の経路を使う）。
- 残すもの: プレビュー境界同期、要素選択、親/子、チャットを畳む表示（返答・承認カード）、送信キー、ソース抜粋の先読み、保存場所（root）選択。

## 6. 段階
1. 追加: Rust `visual_edit/direct.rs`＋`host/visual_direct.rs`＋IPC＋テスト／TS 新 UI（並行、担当は別ファイル）。
2. 切替確認後、旧 Rust・旧 IPC・旧 TS を削除。
3. 検証: type-check／TS テスト／build／cargo test、検証担当による差分照合。
4. HANDOFF.md に記録。実機（Tauri）は未確認のまま明記。

## 7. 既知の限界（受け入れ）
- スコープ外のファイルの変更・新規ファイルは戻せない。
- 他の会話や外部エディタが同時に同じファイルを触ると、戻すときの一致検査（after との比較）でしか検出できない。
- 複数ファイルの戻しは原子的ではない（途中で失敗したら失敗を表示し、状態は `open` 扱いのまま再試行できる）。
