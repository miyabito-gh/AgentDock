# REVIEW_R3: 段階③ 重要差分のレビュー

対象: P3-1 ebecdff・P3-4 ee772a0・P3-5 49a7acd・P3-7 cb10214（主）、P3-0 b11c905・P3-2 e425493・P3-6 a861a02・P3-9 0bc8af7（補助）。照合: `app/DESIGN_P3.md`。
行番号は master（0bc8af7）時点。件数: **高 2・中 4・低 6**。

合意違反（監視目的のresume・承認代行・通信断のdone/failed化・受理不明の無条件再送・~/.codex無断変更・自動削除）は見つからなかった。境界へのCodex型の漏れもない。

---

## 高

### H1. レビュー・圧縮の受付直後に、キューが自動送信しうる（送信条件の違反）
- 場所: `host/thread_ops.rs:204-208`（`Accepted` で即 `resolve_pending_op`）、`host/pending_ops.rs:49-53`（解消時に `kick_queue`）、`host/queue_driver.rs:105-108,177`（手動送信だけが `manual_turn_pending` で保留される）
- 症状: `review/start`・`thread/compact/start` の応答（受付）で PendingOp を消してキューを起こすが、その時点ではレビュー／圧縮のturnの `turn/started`・状態通知をまだ処理していないことがある（応答と通知は別経路で、圧縮の応答は空）。親は待機のままに見えるので、`evaluate` が `Send` を返し、送信待ちの先頭がレビュー・圧縮のturnの開始と競合して送られる。手動送信には「受理からturn終端まで保留」（`note_manual_accept`）があるが、この2操作には相当するものがない。
- 再現: 送信待ち1件（完了後送信）があるチャットで、待機中に「圧縮」を実行。応答直後の `kick_queue` で送信待ちが `turn/start` される（Codex側の扱いは拒否または割込みで、どちらも利用者の意図と違う）。
- 修正案: 受付時に「操作の受付→turnの終端を観測するまで保留」を記録する。`QueueRuntime` に `note_op_accept(chat, since)` を足し、`gate_input` で「`since` 以降に始まったturnの終端（`last_end` の時刻 ≥ since）を観測するまで、ただし上限時間（手動と同じ15秒）内にturn開始を観測できなければ解除」とする。レビューは応答の `turn.id` が取れれば `note_manual_accept` をそのまま使える。PendingOp の解消自体は今のままでよい。

### H2. 「戻し」の記録がパスと時刻だけで、戻していない変更まで「戻し済み」になる（誤表示・以後戻せない）
- 場所: `rules/revert.rs:153-159`（`is_consumed`）、`host/changes.rs:580-582`（`ChangeLine::Reverted{at, paths}` を書く）、`backend/changes.rs` の `ChangeLine::Reverted`
- 症状: 消化の判定が「記録の観測時刻 ≤ 戻した時刻、かつ同じパス」で、どのチャット・どの記録を戻したかを見ていない。
  1. 同じチャットで t1・t2 が同じファイルを変更し、t2だけ戻す（`Selection{turn: t2}`、テスト `later_turns_of_the_same_chat...` の後半と同じ状況）。ファイルは t1 適用後の内容に戻るが、t1 の記録も消化済みになり、一覧で t1 が `reverted: true`（戻し済み）と表示され、t1 を後から戻せなくなる。
  2. 別チャット c2 が先に同じファイルを変更し、c1 が自分の変更を戻した場合、c2 の記録も消化済みになり、c2 側の一覧に「戻し済み」と出る（実際は c2 の変更は残っている）。
- 再現: 上記1。`preview_revert{turn: t2}` → `revert_changes` → `get_change_list{Chat}` で t1 のファイルが戻し済み表示。
- 修正案: `Reverted` に戻した記録の識別子 `records: Vec<{chat, item, path}>`（`#[serde(default)]`）を追加し、計画の `chain` に入った記録だけを消化する。`is_consumed` は識別子があればそれで判定し、旧形式の行（識別子なし）だけ現行の時刻＋パス判定にフォールバックする。保存形式の追加なので早めに直すのがよい。

---

## 中

### M1. 書換え直前にハッシュを照合しておらず、計画後の変更を上書きしうる（TOCTOU）
- 場所: `host/changes.rs:536`（再計画）→ `:564-568` → `execute_revert` `:638-707`
- 症状: ハッシュ照合は `plan_now` の読取り時だけ。`execute_revert` は控え用に同じファイルを読み直すが、その内容が計画時の内容と同じかを確かめずに、計画時の内容から作った `content` で上書きする。再計画と書換えの間（`spawn_blocking` の待ち・io_guard の待ち）にエディタ保存や、検出外のチャット（M3）が書いた変更は失われる（控えには残るが、利用者は気づかない）。
- 修正案: `rules::revert::Write` に計画時の現内容のハッシュ（`expect: Option<String>`、なければ不在）を持たせ、`execute_revert` で控え用に読んだバイト列のハッシュと比較し、違えばそのファイルは書かずに `failed`（「計画後に変更されました」）にする。

### M2. 書換えに失敗したファイルも「戻し済み」に記録され、再試行できない
- 場所: `host/changes.rs:690`（書込みの前に `touched.push`）、`:580-582`
- 症状: Windowsではエディタ・ウイルス対策がファイルを開いていると `write_atomic` の置換・`remove_file` が一時的に失敗しやすい。失敗したファイルも `touched` に入るため Reverted 記録で消化され、一覧は「戻し済み」、以後の計画から消える。実際には何も戻っていない（または移動の片側だけ）。
- 修正案: 成功した書込みだけ `touched` に入れる。移動（2手順）の途中失敗は、成功した側だけを記録し、失敗理由に「一部だけ実行」を残す（現行の文言は維持）。H2の識別子化と同時に直すと楽。

### M3. 戻す・クラウド取込みの「同じ作業フォルダの作業中」判定が狭く、実行中にキューも止まらない
- 場所: `host/changes.rs:470-475`（cwd の完全一致だけ。PendingOp を見ない）、`host/cloud.rs:225-236`（PendingOp・送信待ちを見ない）
- 症状:
  - 戻す: 別チャットの作業フォルダが親・子フォルダ（例 `C:\repo` と `C:\repo\app`）だと作業中でも止めない。PendingOp（受理不明のレビュー・圧縮。turnが動いている可能性）があっても止めない（worktree削除は `pending_ops` を見ている）。
  - 両方: 判定は開始時の1回だけで、実行中（cloud apply は最大120秒）にそのフォルダのチャットのキューが自動送信を始められる。エージェントの編集と取込み・戻しが同時に走る。
- 修正案: 判定を `cloud::paths_overlap` に揃え、`pending_ops` 非空も止める条件に加える。実行中は対象フォルダと重なるチャットのキューを保留する（`QueueRuntime` に「フォルダ操作中」の集合を持ち、`gate_input` で `Hold`。終了時に `kick_queue`）。

### M4. worktree削除で、無視ファイル（.env・ビルド成果物等）が確認なしに消える
- 場所: `host/worktree.rs:354-360`・`gitops/worktree.rs:105-108`（`GitOp::Status` は無視ファイルを数えない）、`host/worktree.rs:402`
- 症状: `git worktree remove`（`--force` なし）は未追跡・変更があれば拒否するが、`.gitignore` 対象のファイルは黙って削除する。dirty=0 で「通常の削除」として確認画面を出すため、worktree内で作った `.env`・ローカル設定・生成物が、2段目の確認なしに消える。
- 修正案: 削除の判定材料に無視ファイルの件数を足す（`status --porcelain=v2 -z --ignored=matching` を読取りの `GitOp` として追加し `!` 行を数える）。1件以上なら `NeedsDiscard` と同じ2段目の確認（文言「無視ファイルN件も削除されます」）にする。

---

## 低

### L1. side相談: 分岐の通知が応答より先に処理されると、一時の会話が通常の会話として一瞬扱われる
- 場所: `host/side.rs:136-166`（`side_threads` への登録は応答の後）、`codex/events.rs:107-120`、`host/state.rs:794-812`
- 症状: `thread/started` が `open_side` の再開より先に処理されると、ルートのエージェントとして `AgentUpdated` が出て、`AgentSeen` の活動行が作られる（保存は `d.chat` で弾かれる）。UIが一時的に一覧に出しうる。
- 修正案: `fork_chat` を呼ぶ前に「side作成中」の印を置き、分岐元 `forkedFrom == main` かつ作成中の間に発見されたルートを保留する、または `side_threads` 登録時に `AgentUpdated` の取消し（一覧から外す）イベントを出す。

### L2. クラウド取込みの「変わったパス」に、前から変更中で取込みでも変わったファイルが出ない
- 場所: `host/cloud.rs:82-87`（`status_delta` は status の行の比較だけ）
- 症状: 取込み前から `M` だったファイルを apply が更に変えても、行が同じなので一覧に出ない。「変わったファイルなし」に見える。
- 修正案: 前後で `M` のままのファイルは内容のハッシュ（`git hash-object` は書込みなしの読取り）を比べるか、「前から変更中のN件は判定対象外」と明記する。

### L3. 読取りエラーをEOFとして扱い、途中までの出力を完全な結果として解析する
- 場所: `exec.rs:54`・`:69`（`read(..).await.unwrap_or(0)`）
- 症状: パイプの読取りエラーで出力が途中で切れても成功扱いになり、`status`・`worktree list` を一部だけで判定する（dirty件数の過小、cloud一覧の欠落）。
- 修正案: 読取りエラーは `RunError::Io` で返す。

### L4. タイムアウトで直接の子だけ終了し、孫プロセスが残る
- 場所: `exec.rs:89`（`kill_on_drop`）
- 症状: `git worktree add` の post-checkout フック、`codex` が起動する補助プロセスは時間切れ後も動き続け、「結果未確認」の後に設定・作業ツリーを変えうる。
- 修正案: 外部コマンドも Job Object（App Server と同じ部品）に入れ、時間切れでジョブごと終了する。

### L5. 削除の報告本文が統一diffに見える場合、別の内容で「復元」する
- 場所: `rules/diff.rs:429-431`（`looks_like_unified` で本文かdiffかを判別）
- 症状: 削除されたファイル自体が `.patch` 等で `@@ -a,b +c,d @@` 行を含むと、削除行だけを集めた別の内容で再作成する。
- 修正案: 削除の記録には本文かdiffかを観測時に確定して保存する（バックエンドの変換で `FileChange` に `diff_format` を持たせる）。不明なら `DiffUnreadable` で止める。

### L6. 戻す処理の間、全保存がio_guardで止まる
- 場所: `host/changes.rs:566`
- 症状: 最大64MiB×件数の読込み・控え書込み・置換をio_guardの中で行うため、その間チャット状態・キュー・活動履歴の保存がすべて待つ。
- 修正案: io_guardは控え（チャット領域への書込み）の間だけ持ち、作業フォルダ側の書換えはロック外で行う（チャット削除との競合は `is_deleting` の確認で足りる）。
