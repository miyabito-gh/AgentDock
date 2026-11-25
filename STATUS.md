# 保全状況

## 2026-11-25 reception-triage の追加実機試験

- `feature/reception-flow` の `40c6bbc`。既存ログインの実 Codex `0.169.0` と `gpt-6.2-sol` を使用し、試験の受付2件にスキルを明示添付した。両方の Codex 履歴でスキル入力と本文の展開を確認した。画面操作は未実施。
- 明確な依頼も要確認となり、自動送信までの統合試験は不合格。依頼先頭の実チャットIDと候補一覧の連番IDの対応が分類モデルに渡らず、スキルは `routes: []` を返した。曖昧な事例は確認質問のみで、両事例とも担当ターン・結果カードなし。
- 試験受付を削除し、対象スコープの履歴が空であることを確認した。一時リポジトリと試験専用データ、検証コードも削除済み。製品コード変更なし、push なし。証跡は `Documents/consult/agentdock-reception/live-trial-2026-11-25.md`。

## 2026-11-25 受付チャットの実機試験

- `feature/reception-flow` の `0bf029e`（実装 `fd3ef04`）を既存の Codex ログインで試験した。Codex CLI は `0.169.0`、モデルは Sonnet が候補にないため試験中のみ `gpt-6.2-sol`。
- 手動確認と自動キュー登録を1件ずつ確認した。両方の担当が完了し、結果を保存して再起動後に照合した。自動キュー登録時は担当が再起動後で状態不明だったため、通常のキュー再開を1回行った。受付依頼の再送なし。
- Windows 操作ツールの native pipe が接続できず、画面操作は未実施。実 Codex に接続した AgentDock ホストで受付から結果回収まで実行し、保存された結果で React カードの静的描画を確認した。
- 試験会話2件、一時リポジトリ、アプリの試験専用データを削除し、試験範囲が空であることを確認した。試験プロセスは終了。製品コード変更なし、push なし。ID と後片付けの詳細は `Documents/consult/agentdock-reception/live-trial-2026-11-25.md`。

## 2026-11-25 受付チャットの送信と結果追跡

- ブランチは `feature/reception-flow`。開始時は差分なし、基点は `b6d08fd`。実装を `fd3ef04` にローカル保存。push なし。
- 受付依頼IDと候補一覧、振り分けとキュー項目、担当ターンと結果を対応付けた。明示した既存先が単一の高確信の分類結果と一致するときだけ自動送信する。恒久ルールはルール帳で編集する。既定の本文は整えた文。
- 担当は Codex。新規作成のモデルは一覧に `claude-sonnet-5-5` があれば初期選択とし、候補にない場合は選択を待つ。既存の担当は現在のチャット設定を使う。
- `cargo check --manifest-path app/src-tauri/Cargo.toml --locked`、app の型チェック・本番ビルド成功。Rust 全ライブラリ試験は1711件通過、1件無視。Node は src と scripts の548件通過。回帰テストで同一依頼の二重登録防止と復旧時に送信しないことを確認。
- 実機の一連試験は未実施。隔離した Codex プロファイルは未ログインで、試験専用認証を取得できなかった。隔離プロファイルは削除済み。実機試験用のプロジェクト・事例は未作成。既存プロジェクトや実データ、既存 Codex の設定・認証ファイルには触れていない。
- 要判断は、プロジェクト別担当表の管理者、並列数と費用の上限、Sonnet が Codex の一覧にないときの試験用モデル。今回の自動送信は依頼ごとに1件。次は隔離認証を用意し、受付ID・振り分けID・担当ターンID・結果回収を確認して試験プロジェクトと事例を削除する。
- 子スレッドは使用せず。作業で起動したバックグラウンド処理はすべて終了。

## 2026-11-25 既定ブランチへの統合

- feature/visual-editor（5b4ae50）、続いて wip/probe-and-ui-mock-inventory（4f0131f）を master に早送りで統合。開始時の master は 09b00e3。
- origin/claude/vibrant-feistel-fcb9f6 の独自コミット 729d871 と同等の変更は 3af8fae に含まれることを git cherry で確認。
- 下記の未追跡試行出力3ディレクトリを、入れ子リポジトリとともに隣接する Rust/_archive/AgentDock-probe-runs-2026-11-25 へ移動した。

- 統合後の 4f0131f で cargo check（--locked）、npm.cmd run type-check、本番ビルド、Node テスト548件が通過。
- Rust 全ライブラリテストは初回並列で一時 Git ディレクトリ衝突が1件発生。その後の単一スレッドと既定並列の両方で1709件通過、1件は既定で無視。該当テストと一時パス生成を含む import.rs は開始時点から変更なし。
- 開始時の master（09b00e3）の独立環境でも、既定並列の Rust 全試験で同じ Git テンプレートコピーの File exists を再現（1530件通過、1件失敗、1件無視）。対象モジュールに絞った13件は通過。並列テストの安定化は次節で対応した。

## 2026-11-25 並列テストの一時ディレクトリ衝突の解消

- 原因: テスト用一時ディレクトリ名を「プロセスID＋現在時刻」だけで作っていた（gitops::import の Rig::new は `agentdock-import-{pid}-{ナノ秒}`）。並列のテストが同じ時刻値（Windows は100ns刻み）を読むと同名になり、`create_dir_all` は既存でも成功するため、2つのテストが同じフォルダで `git init` してテンプレートのコピーが File exists で失敗していた。修正前の再現では import の2件が失敗（1707件通過）。同じ作りが gitops::org／prep／push／snapshot、host::baseline／baseline_snap／changes にあり、ミリ秒の now_ms() を使う continuity_handoff／org::artifacts／org::intake／org::mod（PIDなし）／router はさらに衝突しやすかった（artifacts と handoff は作成前に remove_dir_all するため、相手のフィクスチャを消す恐れもあった）。実装側のコードは無関係で、テストの作りの問題。
- 修正: テスト専用の `src/test_support.rs` に `unique_temp_dir(prefix)` を追加（プロセス内の通し番号＋PID＋時刻で命名し、既存なら失敗する `create_dir` で作成自体を排他にする。重なったら次の番号で再試行）。上記13箇所をこれに置き換え、artifacts／handoff の事前削除をやめた。並列呼び出しで重複しないことのテストを1件追加。
- 結果: cargo check（--locked）通過。`cargo test --manifest-path app/src-tauri/Cargo.toml --lib --locked` を既定並列で3回連続、いずれも1710件通過・0件失敗・1件無視（既存1709件＋追加1件）。負荷確認として `--test-threads=64` で全体3回（各1710件通過）、gitops に絞って `--test-threads=64` で10回（各59件通過）、すべて失敗なし。
## 統合前の保全記録

- ブランチ: `wip/probe-and-ui-mock-inventory`
- 基点: `5b4ae50`（基点時点で `origin/feature/visual-editor` より9コミット先行）
- 今回保全したもの: UIモック2件、Reviewer再現probe 3件を個別コミット。`.env` 無視ルールを独立コミット。
- 実機確認: 未実施。
- 旧フォルダ `AgentDock_claude` 固有コミット、`SKILL.md`、Pythonスクリプトの保全判断: 未決。

## 除外した未コミット項目

試行出力（`probes/app-server/runs/`）:
- `2026-11-12T12-54-57-407Z-reviewer/`: `result.json`, `summary.md`, `fail-assert/`, `fail-import/`, `pass/`
- `2026-11-12T12-55-11-618Z-subagent/`: `result.json`, `summary.md`, `free/repo/`, `free/wt/`, `single/repo/`, `single/wt/`
- `2026-11-12T12-58-33-312Z-subagent/`: `result.json`, `summary.md`, `free/repo/`, `free/wt/`, `single/repo/`, `single/wt/`

入れ子リポジトリ（各パス内に `.git`）:
- `2026-11-12T12-54-57-407Z-reviewer/{fail-assert,fail-import,pass}/`
- `2026-11-12T12-55-11-618Z-subagent/{free/repo,single/repo}/`
- `2026-11-12T12-58-33-312Z-subagent/{free/repo,single/repo}/`
- 2026-11-22 フォルダ統一: パス変更 C:\Users\wmasa\Documents\Rust\AgentDock_next → C:\Users\wmasa\Documents\Rust\AgentDock（旧フォルダ AgentDock / AgentDock_claude は Rust\_archive へ退避）
