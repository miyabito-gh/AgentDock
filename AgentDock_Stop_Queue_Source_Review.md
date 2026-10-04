# CLI 0.160.0 中断確認・キュー処理のソース照合

確認日: 2026-10-04（日本時間）
対象: openai/codex、rust-v0.160.0。GitHub検索は位置発見だけに使い、根拠本文はタグ指定で取得。

## 結論

対象ターンの終了状態を通知する経路には根拠がある。ただし中断要求の成功応答だけでは中断・子孫停止・外部プロセス停止を確定しない。App Server最終採用は保留。

## 中断要求と確認

- turn_interrupt_innerはthreadIdとturnIdを受け取り、通常はactive turnとの不一致や既に終了したターンを拒否する。通常中断はpending_interruptsへ登録し、Op::Interruptを提出後、その場では応答しない。
- turnIdが空のstartup interruptは提出成功直後に応答する。通常ターンの中断確認と同じ意味にはしない。
- bespoke_event_handlingはTurnAbortedで中断応答を返し、対象ターンの終了状態をInterruptedとして通知する。
- **TurnCompleteでもpending_interruptsへの応答を返す。** したがって成功応答だけからInterruptedを合成しない。自然完了との競合を考慮し、threadId・turnIdが一致する終了通知の状態を照合する。
- coreのhandle_task_abortはキャンセルトークン、猶予待ち、タスクabort、session_task.abort、interrupt hooks等を経てTurnAbortedを発行する。この処理はターン中断の根拠であり、すべての子孫・外部プロセスの停止証明ではない。
- abort_all_tasksは当該Sessionのactive turnを取り出す。この関数名だけで他threadの子孫を再帰停止すると判断しない。session_task.abortとhooksの具体的な伝播は追加照合が必要。
- close_unified_exec_processesは別関数として存在する。通常中断から全プロセス終了へ至る経路は今回未確認。外部プロセス・Windows上の実停止は未検証。
- 中断後にmaybe_start_turn_for_pending_workへ進む経路もある。Codex内部のpending workとAgentDock送信待ちを区別し、ターン終了を将来の全活動停止と同一視しない。

## 待機キュー

queue serviceのdispatch_if_idleは同じmanagerのthreadを取得し、保存キューの先頭を読む。有効なUserInputに対してstart_turn_if_idleを呼び、Startedなら該当項目を削除する。NotSubmitted・エラーならその時点で戻る。無効payloadや非UserInputは破棄する経路がある。

この関数内ではAgentDockの「親正常完了、子孫待ちなし、不明状態なし、切断復旧後の送信保留」という条件を確認していない。core側の詳細条件は今回網羅していないため、内部機構全体に子孫確認がないとは断定しない。Codex保存キューをAgentDockの合意済みキューの代替にする根拠は不足している。

前回確認のwarm resume→idle lifecycle→dispatch_if_idle経路により、保存キューがありcoreが開始を認める場合には入力開始へ進み得ることを確認。resumeで必ず送信するとはいえず、拡張の登録・有効化やcoreの拒否条件は未確認。

## 根拠

|対象タグファイル|確認行|blob SHA|
|---|---|---|
|[turn_processor.rs](https://github.com/openai/codex/blob/rust-v0.160.0/codex-rs/app-server/src/request_processors/turn_processor.rs#L1598)|1598–1660: turn_interrupt_inner|e42e53de1ff20d6b2049900442dde1e783f235c9|
|[bespoke_event_handling.rs](https://github.com/openai/codex/blob/rust-v0.160.0/codex-rs/app-server/src/bespoke_event_handling.rs#L188)|188: 自然完了時応答、1204–1219: abort処理、1533–1576: Interruptedと応答|d1b0ab89756d38579d0d976ee72a5ee80cc216f2|
|[tasks/mod.rs](https://github.com/openai/codex/blob/rust-v0.160.0/codex-rs/core/src/tasks/mod.rs#L536)|536–570: abort_all_tasks、903–918: process操作、921–1020: handle_task_abort|28a3b54b1c6fe481407ae5065bf549679d7e0609|
|[queue/service.rs](https://github.com/openai/codex/blob/rust-v0.160.0/codex-rs/ext/queue/src/service.rs#L405)|405–469: dispatch_if_idle、549–565: idle contributor|06aeb63b4cacccf562ee91897bafbcf962bd978c|

SHAはファイル内容識別子。commit SHAではない。ソース経路の確認と実動作成立を区別する。

## 要件・モックへの整理

1. 中断要求の応答、対象ターンの終了状態、子孫状態、プロセス状態を別に保持する候補。成功応答から全体停止を合成しない。
2. 自然完了と競合した場合は実際の終了状態を表示し、ユーザー中断操作後のAgentDock送信待ちは保留を維持する。
3. 合意済みの10秒後「停止未確認」を維持。通知不足・切断時には停止済みとしない。
4. 全対象停止の表示には各対象の根拠が必要。親だけ終了した場合、子孫・プロセスの未確認を残す表現を検討する。
5. 次はsession_task.abort・interrupt hooksの子孫伝播、background terminal停止の公開契約、coreのidle入力条件を照合する。HTMLは未変更。

実装、App Server起動、schema生成、実機検証、Codex設定変更、キュー操作は行っていない。
