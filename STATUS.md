# 保全状況

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
