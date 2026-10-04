# CLI 0.160.0 子孫中断・バックグラウンド実行終了の照合

確認日: 2026-10-04（日本時間）。対象ref: rust-v0.160.0。検索は位置発見のみ。根拠はタグ指定本文。

## 結論

管理下のバックグラウンド実行の一覧・個別終了APIは対象版にあるがexperimental。親の中断だけで子・孫・外部プロセス全体が停止する保証は未確認。App Server最終採用は保留。

## 子孫の中断

- RegularTaskはSessionTaskのabortをoverrideしていない。tasks/mod.rsの210–218行の既定abortは何もしない。この呼出だけに子孫の再帰停止を期待しない。
- hook_runtime.rsの502–542行は設定済みInterrupt hooksを実行する。SubAgentなら早期returnする。フックを標準の子孫停止契約と扱わない。ユーザー定義フックによる影響は別途あり得る。
- agent/control.rsの299–314行は指定agent_idひとつへOp::Interruptを送る。ここには子孫列挙・一括中断はない。
- agent/control/interrupt.rsの16–50行は中断前snapshotを返し、未ロード等を許容する場合もある。この内部APIの戻り値を停止後状態として扱わない。App Serverのturn/interruptと同一契約ではない。
- 今回の処理に再帰停止がないことは全経路の不存在証明ではない。キャンセル伝播・lifecycle contributor・生成途中の競合は未網羅。

## バックグラウンド実行の公開経路

protocol/common.rsの737–754行に以下3メソッドがあり、すべてexperimental指定。

|メソッド|確認した処理|制限|
|---|---|---|
|thread/backgroundTerminals/list|ロード済みthreadの管理下実行をページ取得|全OSプロセス一覧ではない。os_pid/cpu_percent/rss_kbはNone|
|thread/backgroundTerminals/terminate|threadId・processIdから終了処理を呼びboolを返す|processIdは管理ID。OS PIDとして使わない|
|thread/backgroundTerminals/clean|Op::CleanBackgroundTerminals提出後に空応答|提出成功から実停止を合成しない|

thread_processor.rsの2387–2449行で上記入口を確認。process_manager.rsの1843–1874行では対象なしでfalse、未終了ならterminate_confirmedを待ち、エラーならfalse。成功後の管理情報削除には条件があり、initial_exec_command_activeなら残る場合もある。

process.rsの259–271行はLocalでprocess_handle.terminateを呼び、ExecServerでterminate応答を待ち、その後signal_exit/finish_terminationを行う。下層のWindows終了処理は未照合。terminated=trueは管理APIの結果であり、全OS子孫の実停止保証へ拡大しない。falseも稼働中だけを意味しない。

対象版にAPIがあることと、安定した製品契約・ユーザー実環境の成立を区別する。

## 全対象停止の設計候補

1. AgentDock送信待ちを保留し、親・子・孫のIDとターンIDを照合する。
2. 各対象へ中断を要求し、それぞれの終了通知を確認する。自然完了との競合は実状態を保持する。
3. 各threadの管理プロセス一覧と必要な終了要求・結果を別に記録する候補。具体的操作順序は未決定。
4. 生成途中・ページ増減・接続断・通知欠落を考慮。初回一覧だけで全対象確認済みとしない。取得不能は不明、10秒後は合意済みの停止未確認。
5. モック状態候補は「親は中断済み／子孫は確認中／バックグラウンド実行は未確認」。HTMLは未変更。

要件を緩めず候補として記録。全対象停止の成立確認は未完了。

## 取得した根拠

|対象タグファイル|blob SHA|
|---|---|
|[codex-rs/core/src/tasks/regular.rs](https://github.com/openai/codex/blob/rust-v0.160.0/codex-rs/core/src/tasks/regular.rs)|bb8f00dc521fb15cb9e60f1e6dab6d5f64b10b53|
|[codex-rs/core/src/tasks/mod.rs](https://github.com/openai/codex/blob/rust-v0.160.0/codex-rs/core/src/tasks/mod.rs)|28a3b54b1c6fe481407ae5065bf549679d7e0609|
|[codex-rs/core/src/hook_runtime.rs](https://github.com/openai/codex/blob/rust-v0.160.0/codex-rs/core/src/hook_runtime.rs)|30c4162ffc06307dc0a73189cb4b7944b6a949c2|
|[codex-rs/core/src/agent/control.rs](https://github.com/openai/codex/blob/rust-v0.160.0/codex-rs/core/src/agent/control.rs)|565d36519aa5be4fbd9fc4a6d3fc2336f2856a05|
|[codex-rs/core/src/agent/control/interrupt.rs](https://github.com/openai/codex/blob/rust-v0.160.0/codex-rs/core/src/agent/control/interrupt.rs)|5ad4c193519259673568aa56b31edc2aa3949cce|
|[codex-rs/app-server-protocol/src/protocol/common.rs](https://github.com/openai/codex/blob/rust-v0.160.0/codex-rs/app-server-protocol/src/protocol/common.rs)|c37c4956f1c3cbf21f293ca2aa41a6fec209bcfa|
|[codex-rs/app-server/src/request_processors/thread_processor.rs](https://github.com/openai/codex/blob/rust-v0.160.0/codex-rs/app-server/src/request_processors/thread_processor.rs)|289cb63953662eb5fba00222ec49432dd976b944|
|[codex-rs/core/src/unified_exec/process_manager.rs](https://github.com/openai/codex/blob/rust-v0.160.0/codex-rs/core/src/unified_exec/process_manager.rs)|3dc3e97d91245b3326637817570b06ead902db96|
|[codex-rs/core/src/unified_exec/process.rs](https://github.com/openai/codex/blob/rust-v0.160.0/codex-rs/core/src/unified_exec/process.rs)|8305c02894737e70708a19b9dace6fee8d425ae5|

blob SHAはファイル内容識別子。実動作の証明ではない。

次はWindowsの下層終了範囲とexperimental利用条件を照合し、Plugins等の残る採用条件と合わせて判定表を更新する。実装・App Server起動・schema生成・実機検証・既存設定変更は行っていない。

