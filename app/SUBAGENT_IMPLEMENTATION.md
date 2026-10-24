# S1–S3 実装と停止時点の引き継ぎ

更新: 2026-10-24。利用者がconfig.tomlを変更するため「切りのよいところで止めて」と指示し、新規実装を停止した。作業はfeature/subagent-migrationの未コミット差分。S1–S3の実機受入完了ではない。旧orgは有効なまま。

## 現在の接続

- `src-tauri/src/backend/subagents.rs`: 新方式の実行記録・開始/明示復旧・取り込みプレビュー/台帳の型。ts-rsによる8型の生成が成功。
- `src-tauri/src/host/subagents.rs`: 通常hostのworktree作成→親チャット作成→親ID保存→通常送信。native子生成は親Codexへ指示し、アプリから架空のspawn RPCは送らない。親プロンプトは編集担当同時1体/検証順/適切な成果コミットを依頼し、機械的な排他・子別sandbox保証ではないと明記。
- `src-tauri/src/host/subagents_tests.rs`: 終端/未発見子/鮮度の厳格ゲート、保存競合、future schema/破損保護、台帳の不明/破損/重複、実Gitによるsource identity/dirty/取り込み先重複変更/begin保存失敗/成功の試験。
- `backend/mod.rs`, `host/mod.rs`, `host/queue_driver.rs`, `commands.rs`, `lib.rs`: module/Host runtime、観測フック、受理→イベントの空白期間ガード、6コマンドを接続済み。
- UI/TS IPCの担当は別worker。停止時点のその状態はparentのHANDOFFを参照。本workerはUIを変更していない。

保存先は既存データと区別した追加ディレクトリ `subagent-runs/<run-id>/run.json` と `imports.jsonl`。設定・認証・通常チャットの保存形式は変更していない。既存データのコピー/バックアップ、アプリ/Codex起動、実データへの書込みは行っていない。

## API（実装済み）

Tauri invokeはrunsだけ引数なし、他は `{args: ...}`。

- `subagent_start`: `{repo, request, model: ModelChoice|null}` → `SubagentRunView`
- `subagent_runs`: 引数なし → `SubagentRunView[]`
- `subagent_recover`: `{id, parent: ChatKey}` → `SubagentRunView`
- `subagent_import_preview`: `{id}` → `SubagentImportPreview`
- `subagent_import`: `{id, token}` → `OrgImportOutcome`
- `subagent_verify_import`: `{id}` → `SubagentImportRecord[]`

`SubagentRunView`はrun/agents/blockers/imports/recoveryCandidatesを含む。run.phaseのreadyは開始工程の記録であり、成果成功・全子孫停止・取り込み成功を意味しない。親の会話は通常open_chatから選択し、composer/steer/承認/質問/停止/監視は既存通常経路を使用する。

## 安全性の変更

親作成前の工程を保存し、親IDを初回モデル送信より先に保存する。保存失敗時は送らない。不明工程を自動再実行せず、worktreeへ既に結び付いた既存親候補を利用者が明示選択して、読取り履歴の親/作業場所を確認する復旧だけを提供する。復旧操作で送信/resumeしない。worktree作成自体が不明で記録がない場合は自動回復せず、既存worktree管理での手動確認を残す。

実行記録のI/Oはblocking taskへ逃がし、子IDは最新記録とのunionで保存する。走査で発見した全IDを保持するため、取得上限によりまだHostDataへ採用できなかった子も未確認として取り込みを止める。直接親が不明の走査も完了扱いしない。破損/future schemaを上書きせず、記録ディレクトリ/ファイルのリンク・ジャンクションを拒否する。

取り込みは既存gitops/import.rsの低レベルprepare/check_worktree/executeを再利用。偽のorg workflowは作らない。互換性優先でOrgArtifactFile/OrgImportOutcome等の既存型を再利用しているため、旧orgの完全撤去時には型/低レベル規則の共通化が後続課題となる。

元repoはcanonicalなGitルートを保存する。成果worktreeは専用領域配下・管理台帳・Git worktree一覧・Gitルート/管理ディレクトリ・branch/HEAD・base祖先・cleanを照合し、previewとexecute前に再確認する。取り込み先のbranch/HEAD/進行中操作/dirty重複/ファイルロックは低レベル処理が実行直前に再検査する。期限付き一回token、同じ親の送信ロック、Git操作ガードとfolder guardを使用し、別チャットの取り込み先での実行も拒否する。

親完了でunknown子の条件を緩めない。全既知子の最新turn完了、接続/鮮度/走査、停止・受理不明・未確定操作・キューの不存在を要求する。Failed/Interruptedの子の成果を明示受入する経路は提供していない。停止を成功に変換しない。

begin台帳を書けなければGitを実行しない。実行後のHEAD読戻しでのみAppliedとなる。end台帳の保存失敗はエラーとして返し、再実行でなくHEAD照合を案内する。beginのみ/Unknownの記録は結果未確認としてブロックし、明示のHEAD照合ではexact resultだけAppliedにする。旧HEADだけで未適用と断定しない。Partialは自動解消しない。壊れた台帳を行スキップで成功扱いしない。

前回停止文書に記載したfolder guard自己ブロックは再調査で該当しなかった。queue_gate_inputはfolder_busy=falseで、親送信ロックを保持しながらfolder guardを取って検査できる。受理からイベント到着までの別の空白期間に対してsubagent_turn_pendingを追加した。

## 検証

- `cargo test --manifest-path app/src-tauri/Cargo.toml --lib host::subagents --locked`: 初回はコンパイル成功、3件成功/1件失敗。失敗は実Git fixtureのWindows autocrlfによるLF/CRLF差。fixtureにcore.autocrlf=falseを設定して修正した。
- 修正後 `cargo test --manifest-path app/src-tauri/Cargo.toml --lib subagent --locked`: **16 passed / 0 failed**。本機能の4件のhost安全テストと8件の型export、既存の関連4テストを含む。実Gitテストは専用temp配下のみ使用し、アプリ/Codexは起動していない。終了時にfixtureを除去。
- ts-rsの既存serde transparent解釈警告あり。
- cargo check単独、フロントtype-check/build、UI操作、全エンドツーエンド試験、独立レビューは本workerでは未実施。上記Rust試験は現在接続済みコードをコンパイルしているが、実機受入の代わりにはならない。

## 次に必要なこと

1. UI担当の停止記録・差分を照合し、未完なら新方式入口/履歴/通常親チャット選択/復旧/取り込み確認を完成する。元repo dirtyを持ち込まないこと、編集1体は指示だけであることを表示する。
2. 独立レビューで競合/復旧/取り込みを確認する。token期限・再利用、親生成直後の保存失敗、begin後end保存失敗のホスト全経路、実際のnative spawn/承認/steer/子孫停止/切断再起動の統合試験は未検証。
3. 一時Git試験はsource dirty/identityと取り込み先dirty/begin失敗/Appliedを確認済みだが、source HEADがpreview→execute間に変わる競合のホスト全経路、junction拒否のOS統合試験は未実施。
4. アプリが管理する親送信・Git操作はガードするが、外部CLIやnative子の自律再活動をOSレベルで排除する保証ではない。実機でイベント/走査の鮮度と停止を受け入れ確認する。モデル制約は今回のAstra禁止を恒久的製品ハードコードへ拡張していない。
5. 実Codex試験の利用許可は未確定。旧アプリの稼働を再確認し、必要な停止/AgentDock領域バックアップを行う範囲をparentと利用者の許可に従って判断する。無断起動しない。
6. 実機受入まではL1を行わず旧org開始/実行/履歴を保持する。S1–S3の完了扱い・pushはしない。

コミットはparent担当。本workerはcommit/pushを行っていない。共有HANDOFF/README/MIGRATIONはparentが更新する。

## ?????????????

- UI????state updater?await???????npm.cmd run build????????????????????ID??????????
- ??Sol???????????2????????????????15???/??Accepted???????????????????????????
- ??????????????????????Accepted?turn ID??????????15???????
- ????????/??/????/??????????????????????folder guard?????????try-lock????????????????turn???????????review/compact??????????
- ????Sol??????????????????Luna??????????????????????????????1??????cargo check --manifest-path app/src-tauri/Cargo.toml --locked???
- ??Luna???2??????????????????????????????????????????????????????
- ???????/??Git????Codex?????????????CLI??????OS??????????????
