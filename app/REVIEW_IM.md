# REVIEW_IM — IM-1・IM-2・IM-5b（＋IM-4・lastNewChat）の差分レビュー（2026-10-15）

対象: 0e76a06（IM-1）、8604d63（IM-2）、37cb7d9（IM-5b）、参考に dcae82f（IM-4）・c6eae31（IM-3 の lastNewChat 部分）。コードは修正していない。
観点: 並行処理・状態遷移・合意違反（無条件再送・監視目的のresume・承認代行・通信断のdone/failed化）。

## 結論

高1・中3・低5。無条件再送・承認代行・監視目的のresume・通信断のdone/failed化は**なし**。
`FirstSendInFlight` は手動送信・再送・キュー（自動・再開）には効いているが、**送信以外の「turnを始める／チャットを消す」操作に効いていない**のが主な穴。

確認できた点（問題なし）:
- `FirstSendRelease` は Drop で解除するので、名前付け失敗・送信の3値どれでも・panicでも解除される。受理不明は `register_unknown` が先に走り、解除後は `AcceptanceUnknown` で止まる（隙間なし）。
- 切断は rpc 層で「書込み前＝NotConnected（拒否・再送可は利用者操作のみ）／書込み後＝OutcomeUnknown（照合のみ）」に分かれ、自動再送なし。
- 戻り値 Sending と `sendUpdated` の順序逆転は `mergeAttempt`（同じ attemptId の確定を Sending で上書きしない）で吸収される。
- 作成の失敗／不明: rpc は書込み後の失敗を OutcomeUnknown にするので、`resultOfError` が `notConnected` を failed（入力し直し可）にしても thread/start の二重作成にはならない。
- ドックの回答は `RequestCard` → `onRespond(r, a)` で押した1件だけ。中央とドックに同じ要求のボタンが2つ出るが、rpc の `server_pending.remove` が原子的で2回目は AlreadyResolved（二重応答は出ない）。
- 監視窓: `actions={mini ? undefined : actions}` かつ `canWrite = !!actions && !mini` で、回答ボタン・指示欄は出ない（要求は種類と要約のみ）。
- ドックのカードはスナップショットだけで描き、read・resume を追加していない。状態の文言は既存の `chatStatusText`／`FRESH` のまま（不明を完了・失敗にしていない）。
- `lastNewChat`: `#[serde(default)]`、`NewChatDefaults.model` は Option（欠けても None）、`ModelChoice.speedTier` は default・TS optional。旧 settings.json は読める（テストあり）。`deny_unknown_fields` は無いので旧版が新ファイルを読んでも落ちない。更新は thread/start 成功時のみ。
- IM-4 `paneModes`: 設計 §5.1 の式どおり。閉じた側は「開いたら」の判定で、`toggleLeft/Right` は一時パネル時に `lNarrow/rNarrow` だけを動かすので、折りたたみ中に勝手にオーバーレイは開かない。保存値は不変。指摘なし。

---

## 高

### H1 最初の送信が確定する前に、削除・作業中アーカイブが「停止確認済み」として進む（合意「停止を確認できないまま削除しない」違反）

- 場所: `app/src-tauri/src/host/manage.rs:149-152`（`chat_work`）、`app/src-tauri/src/host/lifecycle.rs:68-93`（`work_of`）、`app/src-tauri/src/host/mod.rs:715-738`（後続タスク）
- 何が起きるか: IM-1 で `start_chat` は thread/start 直後に戻り、根は Idle（`mod.rs:677-683`）のまま後続タスクが 名前付け→送信前記録→控え（基準B）→`turn/start` を進める。`chat_work` は `first_sends` を見ないので、`delete_decision` は Proceed になり、削除（Codex 側の削除＋一般チャットの作業領域の削除）がそのまま進む。一方、後続タスクは `check_sendable` を通らない（`check_not_delete_pending` も見ない）ので、削除後に `record_presend`／`update_local` でチャットの記録を作り直し、消したスレッドへ `turn/start` を送る。結果として (a) 削除したはずのチャットで turn が動く／受理不明の記録が残る、(b) 消した作業領域へ書き込むturn、(c) 削除済みチャットの chat.json が復活する。作業中アーカイブ（`manage.rs:215` の `archive_ready(&self.chat_work(..))`）も同様に `thread/archive` を最初のturnと並行して送る。
  - 削除は `baseline_rt.queue_lock` を取るので、後続タスクが控えの取得中なら待つが、控えの後の `turn/start` とは排他されない。
- 再現: 作業フォルダの大きい開発チャットを最初の依頼つきで作成（控えの取得が数秒かかる）→ 一覧に実チャット行が出た直後に行の「…」→削除 → 確認画面が「停止不要」で出て、削除が完了する。その後 `turn/start` が送られる（診断ログ・App Server 側のスレッド）。一般チャットでも、名前付けRPCが遅い環境で同様。
- 直し方: `ChatWork` に「最初の送信が未確定」を入れる（例: `chat_work` で `w.unresolved_send |= self.first_send_in_flight(chat)`、または専用フィールド `first_send_in_flight`）。`delete_decision`／`archive_ready`／終了確認の集計が「未完了」として扱うようにする。加えて後続タスク側でも、送信直前に `check_not_delete_pending` を確認し、削除中なら送らず Rejected（再送可）で確定させる。

## 中

### M1 `turn_op_blocker` 等が `FirstSendInFlight` を見ず、最初の turn/start と レビュー・圧縮・Goal・変更を戻す 等が並行する

- 場所: `app/src-tauri/src/host/thread_ops.rs:96-107`（`turn_op_blocker`）、`thread_ops.rs:394`（resend の busy）、`host/changes.rs:1064`、`host/work_folders.rs:223`、`host/cloud.rs:235`、`host/baseline.rs:825`（いずれも `unknown_attempt` だけを見る）
- 何が起きるか: 作成直後は根が Idle・`running_turn` なし・受理不明なしなので、これらの判定は「空いている」と答える。IM-1 前は `start_chat` が送信の受理まで戻らなかったので UI から触れなかった窓が、今は利用者に開いている。最初の `turn/start` と `review/start`・`thread/compact/start` 等が同じスレッドへ同時に出る、作業フォルダを書き換える操作（変更を戻す・取込み）と最初のturnが並行する、作業フォルダの変更が最初の送信の `work_folders` と食い違う、など。
- 再現: 最初の依頼つきで作成 → 選ばれた直後にメニューからレビュー開始（または圧縮）を押す → 2つの turn 開始要求が並ぶ（片方が拒否されるか、両方走る）。
- 直し方: `check_first_send_settled` を `turn_op_blocker` の先頭と、上記の busy 判定（`unknown_attempt(..).is_some()` の箇所）に OR で足す。共通の述語 `send_unsettled(chat) = unknown_attempt.is_some() || first_send_in_flight` を作って置き換えるのが漏れにくい。

### M2 最初の依頼の「送信中」表示がなく、作成後の中央・ドックは送れる見た目のまま（設計 §2.2・§2.3 との差）

- 場所: `app/src/App.tsx:840`（`noteAttempt` だけで PendingSend を作らない）、`app/src/App.tsx:1526-1536`（ドックの `block`）、`app/src/ui/CenterPane.tsx` の `sendLockReason`
- 何が起きるか: 戻り値の Sending は `attempts` に入るだけで、`noticeOf` は sending を表示しない（`App.tsx:83-98` の default）。`pendingSends` に行が無いので段階（SendPhase）も出ない。利用者からは「作成したチャットが空で、最初の依頼がどこにも見えない」状態が turn の開始通知まで続く。入力欄・ドックの指示欄は有効なので、もう一度送ると `FirstSendInFlight` で拒否され、未送信の仮行が残る（二重送信にはならないが、失敗表示が出る）。クイック新規では入力欄の文を消してから作成する（`App.tsx:855`）ので、なおさら「消えた」ように見える。
- 再現: クイック新規で長めの依頼を送信 → 作成されたチャットが選ばれる → turn が始まるまで依頼が表示されない。その間に中央で何か送ると「最初の依頼を送信しています」で失敗行が出る。
- 直し方: `createChat` の成功時、`r.firstSend` があれば `pendingSends[chatId]` に最初の依頼の行（`text=i.firstMessage`、`outcome` は `attempts` に同じ attemptId の確定があれば `outcomeOfAttempt` で即反映）を作る。`sendLockReason`（またはドックの `block`）に「`attempts[chatId]` が sending」を足し、「最初の依頼を送信しています」で無効化する。

### M3 入力中の凍結が「並び」だけで「所属」を凍結しない。入力中のカードが帯・ほかへ移ると作り直され、フォーカスと IME 変換が失われる

- 場所: `app/src/ui/Dock.tsx:431-432`（`stableOrder` を帯・タスクで別々に適用）、`Dock.tsx:492`・`:495`（帯と列で別の親要素）、`app/src/ui/dockTasks.ts:127-133`（frozen でも `next` に無い ID は落とす）、`dockTasks.ts:96-97`（上位5件の切り出し）
- 何が起きるか: (a) 指示欄に入力中のチャットに承認・質問が来ると、そのチャットは `attention` に入り `tasks` から外れる。凍結中でも `stableOrder` は「今の next に無い ID」を落とすので、カードは帯の `<div>` 側へ移り、React はカードを作り直す（親が違う）。textarea は新しく作られ、フォーカスと変換中の文字が失われる。(b) 別のチャットが動いて最近順の上位5件が入れ替わると、入力中のカードが `overflow` に落ちて消える。設計 §4.5「入力中はスクロール・展開・並べ替えをしない」に反する（下書き自体は `drafts` に残る）。
- 再現: ドックで終了カードの指示欄に日本語を変換中のまま、そのチャットに承認要求が来る（または、6件以上のチャットがあり5番目のカードに入力中に、他のチャットが動く）→ 変換が途切れ、カードが帯へ移る／消える。
- 直し方: 凍結中は所属も前回のまま保つ。例: `typing` の間は前回の `bandIds`／`taskIds` に含まれていた ID はその容器に残し（存在するチャットに限る）、新しい ID だけを帯へ追加、上位5件の切り出しも凍結前の集合を優先する。最低限、フォーカス中のカード（`document.activeElement` を含むカード）だけは容器を移さない。

## 低

### L1 `first_sends` への登録が、実チャットの通知（`ChatUpdated`）より後

- 場所: `app/src-tauri/src/host/mod.rs:685-692`（`mutate` で一覧へ出す）→ `:717`（`first_sends.insert`）
- 何が起きるか: ごく短い間だが、UI が実チャットを知った後に未登録の窓がある。実害はほぼ無いが、順序として逆。
- 再現: 理論上のみ（作成直後の行に即座に送信）。
- 直し方: `first_message` がある場合は、`mutate` より前（thread/start の応答直後）に `insert` する。

### L2 終了確認・強制終了の件数に、最初の送信中のチャットが入らない

- 場所: `app/src-tauri/src/host/lifecycle.rs:95` 以降（終了確認用の作業状況）
- 何が起きるか: 作成直後に終了すると確認なしで閉じられる。送信前記録が残るので再起動後は受理不明として照合され、再送はしない（合意違反ではない）が、利用者には「作業中なし」と見える。
- 再現: 最初の依頼つきで作成 → すぐ「AgentDock を終了」。
- 直し方: H1 の `ChatWork` 拡張と合わせて、終了確認の集計にも含める。

### L3 作成が「不明」のとき、最初の依頼の本文を取り戻す手段がない

- 場所: `app/src/ui/CreatingRows.tsx`（不明は「一覧を更新」「閉じる」だけ）、`app/src/App.tsx:855`（クイック新規は作成前に本文を消す）
- 何が起きるか: thread/start の応答が来ない・Io 等で不明になると、仮行のラベル（先頭40字）以外に本文が残らない。押し直しを促さない方針は正しいが、作られていなかった場合に利用者が長文を打ち直すことになる。
- 再現: クイック新規で送信 → 応答待ちで切断 →「作成を確認できません」→ 閉じる。
- 直し方: 不明の行に「本文をコピー」（クリップボードへ。送らない）を置く、または「閉じる」時にクイック新規の下書きへ本文を戻す。

### L4 「一覧を更新」が成功しただけで仮行を外す

- 場所: `app/src/App.tsx` の `creatingAct`（`op === "refresh"`）
- 何が起きるか: 作られていなかった場合も、更新が成功すれば仮行が消え、結果（作られていない）が示されない。状態の誤表示ではないが、確認の結果が分からない。
- 再現: 不明の仮行で「一覧を更新」→ 実チャットが無いまま仮行が消える。
- 直し方: 更新後に `startedAt` 以降に作成された一般チャット／同じ cwd のチャットが増えたかで「見つかりました／見つかりません（必要なら入力し直してください）」を出す。見つからない場合だけ「入力し直す…」を出してよい（作られていないとまでは断定しない文言で）。

### L5 （既存・参考）thread/start が受理不明のとき、一般チャットの作業領域を消してしまう

- 場所: `app/src-tauri/src/host/mod.rs:646-651`（`discard_general_workspace`。今回の差分外）
- 何が起きるか: IM-2 で「不明→一覧を更新」が明示されたので、実際には作られていたスレッドが、消された cwd を指す外部扱いのチャットとして一覧に出る。
- 直し方: `OutcomeUnknown` のときは作業領域を消さずに保留扱いで残す（拒否・未接続のときだけ消す）。今回の範囲外なので別タスクでよい。
