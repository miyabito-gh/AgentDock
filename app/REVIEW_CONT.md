# REVIEW_CONT: 継続チャット最小版（G-P）C2・C3 差分レビュー（R-C）

- 対象: `git diff 8fadef8 1bbab66` のうち C2・C3 のホスト側（`app/src-tauri/src/`）。UI は `App.tsx` の `onHandoffSend` と `ui/HandoffNotice.tsx` の安全面だけ見た。
- 基準: `app/DESIGN_CONT.md` §2（I1〜I6）・§5・§6・§9・§10。
- 実行: `cargo test --lib host::continuity`（25件）、`cargo test --lib rules::continuity`（49件）はすべて通過。コードは変更していない。

## 要約

### G-P の修正（C6）で直すべきもの（高・中）

| # | 重大度 | 内容 | 主な場所 |
|---|---|---|---|
| H1 | 高 | 引き継ぎ元 P（一般チャット）を削除すると、先頭 N が使っている作業領域ごと消える | `host/manage.rs:502-516`、`host/continuity_handoff.rs:281-289` |
| M1 | 中 | やり直しの判定が `no_history` だけに頼っている。実際には会話がある N を隠し、つながった表示からも外してしまう | `host/continuity_handoff.rs:68-70, 156, 265-269, 320-321` |
| M2 | 中 | 添付の検査が先頭の切替え（C5）の後にある。予測できる失敗で「作ったが送れない」になり、P の添付が隠れたチャットに残る | `host/continuity_handoff.rs:356-369` |
| M3 | 中 | やり直しの途中で落ちると、起動後に N と N2 が両方先頭として出る（A8 違反）。修復の手掛かりがない | `host/continuity_handoff.rs:320-355`、`rules/continuity.rs:626-686` |
| M4 | 中 | 案内の表示中、使用率が更新されるたびに `preview_handoff` が走り、大きな履歴の全件読取り（`thread/read`）を繰り返す | `ui/HandoffNotice.tsx` `useHandoff` の useEffect、`host/continuity.rs:144-177` |

### 持ち越し（低）

| # | 内容 |
|---|---|
| L1 | 目的メモを削除してから作り直すと、届いていない版番号を再び使うことがある。後から受理が分かった送信で、届いていない文を「届いた」と記録しうる |
| L2 | 後から受理が分かった送信（照合）で、`compacted_since_delivery` を無条件に false へ戻す。`purpose_delivered_rev` も古い版へ戻る |
| L3 | 設計にある「履歴で資料つきの依頼を見たら Delivered」が未実装。受理から保存までの間に落ちると、同じ資料（最大 24,000 字）をもう一度付ける |
| L4 | `start_unknown` が `Io` を含むため、ローカルの失敗も「作れたか確認できません」と表示される |
| L5 | 隠れた P（ドック・通知から開ける）で目的メモを編集できる。直るのは P の写しだけで、先頭 N には反映されない |
| L6 | 引き継ぎ中に P の送信ロックを待っていた別画面の手動送信が、引き継ぎ後にそのまま P へ送られる |
| L7 | 使用量の取込みで、chat.json のないチャット（再開した外部の会話など）にも領域と chat.json を作る |
| L8 | `thread/compacted` 通知を正規化していない（item だけで観測している）。item が出ない圧縮があると取りこぼす |

### 確認して問題がなかった点（合意・不変条件）

- I1: 資料を付けるのは `send_message` の `is_new_turn && steer_entry.is_none()` だけ（`mod.rs:1002`）。キュー送信は `dispatch_send_as` も `send_message` も通らない。組織の送信は `SendAuth::Org` で、届いた記録も付けない（`mod.rs:1078`）。照合では、キュー項目の記録（`entry` あり）を除いている（`queue_driver.rs:1006`）。`retry_send` は送った本文そのまま（資料込み）を送るので、設計どおり。分岐・編集して再送で資料が付くのは、利用者の操作による `send_message` だけ（§9 のとおり）。
- I2: P に対して呼ぶバックエンド操作は `read_history` だけ。resume・send・archive・delete・rename は N にだけ呼んでいる。
- I3: 拒否・受理不明は `HandoffResult` で返すだけで、自動では再送しない。受理不明のときは資料が `Pending` のまま残り、既存の照合に任せている。
- I4・I5・I6: 推定値は使っていない（`last_total` がなければ捨て、上限が 0 や null なら割合を出さない）。子孫と side の値は混ぜていない（`state.rs` `root_chat_of`）。保存型は ChatKey・LocalId・数値・文字列だけ。`~/.codex` には書かない。一般チャットは P と同じパスなので、trust の新しい追記も起きない。
- ロック順: P の送信ロック → N の送信ロックの1方向だけ。保持中に呼ぶ `queue_gate_input`・`carry_attachments`・`start_chat_in`・`rename_best_effort` は P のロックを取らない。キュー駆動は `try_lock` なので待たない。デッドロックの経路は見当たらない。
- 二重実行: `handoff_inflight` と `resend_inflight` を `try_begin` で取り、P のロックを取った後に前提を判定し直している（`superseded` で古い画面を止める）。
- UI: `handoffAndSend` を呼ぶのは `Composer.send()` だけで、利用者の送信操作の中に限られる（`CenterPane.tsx:899`）。`useHandoff` は読取り（`previewHandoff`）しかしない。自動で送る経路はない。

---

## 高

### H1 引き継ぎ元 P を削除すると、先頭 N の作業領域が消える

- 場所: `host/manage.rs:502-516`（`remove_area` → `store.remove_chat_dir(chat)`）、`host/continuity_handoff.rs:281-289`（N は P の `chats\<dirP>\workspace` をそのまま cwd にする）、`host/manage.rs:321-336`（削除の事前表示）。
- 何が起きるか: 一般チャットの作業領域は P のチャット領域の中（`chats\<dirP>\workspace`）にあり、N・N2… はそのパスを作業フォルダとして共有する。P を削除すると、`remove_chat_dir(P)` が領域ごと消すので、先頭の会話のファイルが消える。元に戻せない。削除の確認にも「新しい thread が使用中」とは出ない。単体テスト `deleting_the_new_chat_never_removes_the_sources_workspace` が確かめているのは、N を削除した場合だけ。
- 再現条件（どれでも起きる）:
  1. P をドックや通知から開き、ヘッダーの「…」から削除する（§9 で、隠れていても開ける設計になっている）。
  2. N の chat.json が破損・新しい版で読めず、起動後に P が一覧へ戻る。利用者が古い重複だと思って P を削除する。
  3. 引き継ぎの C3〜C5 の間（P がまだ一覧にある）に別の画面で P を削除する。削除は送信ロックを取らず、`is_deleting` は C0 でしか見ていない。
  4. 鎖 P0→P1→P2 では、領域は P0 にある。隠れた P0 を削除すると、P2 の作業ファイルが消える。
- 修正案（推奨順）:
  - (a) 削除の事前表示と実行の両方で、「この領域の `workspace` を作業フォルダ（`Chat.cwd` か `cached_meta.cwd`）として使う、別の読込み済みチャットがあるか」を調べる。あれば削除を止め、「新しい thread（名前）がこの作業領域を使っています」と理由を出す。`superseded_by` があるチャットも同じ扱いにする。
  - (b) 削除を許す場合は、`workspace` だけ残してチャット領域の残りを消す。残す場所の持ち主（先頭）を記録する必要がある。
  - あわせて、`handoff_and_send` の C5 の直前で `manage_rt.is_deleting(&p)` と `delete_pending` を見直す。削除が始まっていれば、N を CreatedNotSent にして送らない。
  - 単体テストを足す: 「P を削除しようとすると止まる（または workspace が残る）」。

---

## 中

### M1 やり直しの判定が `no_history` だけに頼っており、会話のある N を隠しうる

- 場所: `host/continuity_handoff.rs:68-70`（`is_redo_source`）、`:156`、`:265-269`、`:320-321`。
- 何が起きるか: `no_history` は「Codex の一覧に出ていないので、記録から作った行」という意味（`persist.rs:135-153` `chat_from_record`）で、「発話がない」とは限らない。一覧のページ外にある場合や、一覧を読む前もここに入る。N の送信が受理された直後、`Delivered` を保存する前に落ちると、N の資料は `Pending` のまま残る。この状態で N の `no_history` が真だと、「引き継いで送信」がやり直しとして動く。すると次のことが起きる。
  - P を読み直さずに、同じ資料を N2 へもう一度送る。
  - N に `supersededBy=N2` を付けて一覧から隠す。
  - N2 の列は N の列と同じで N を含まないため、「以前の会話を表示」でも N の会話（AI の応答を含む）が出ない。ドックから開く以外に見る手段がなくなる。
- 再現条件: 起動後、一覧に N が記録由来（`no_history=true`）で出ていて、資料が `Pending`、さらに Codex 側に N の turn がある状態で「引き継いで送信」を押す。
- 修正案: やり直しの前に `read_history(N)` を読む（読取りのみ）。「履歴がない（NotFound）」か「依頼を含む turn が 0」のときだけ、やり直しにする。それ以外は通常の引き継ぎとして N を列に含め、N の履歴から資料を作る。あわせて、ここで履歴に資料つきの依頼が見つかれば `Delivered` に直す（L3 と同じ処理）。

### M2 添付の検査が先頭の切替え後で、予測できる失敗で「作ったが送れない」になる

- 場所: `host/continuity_handoff.rs:356-369`（C6）。
- 何が起きるか: `args.attachments` が P の台帳にない、`Ready` でない（コピー中・失敗・欠損）といった、事前に分かる理由でも、気づくのは C5 で P を隠した後になる。結果は `CreatedNotSent` になる。
  - P の下書きの添付は、隠れた P に残る。
  - 途中までコピーできた分だけが N の入力欄に入り、件数がずれる。
  - 利用者は付け直すしかなくなる。
- 再現条件: 添付のコピー中（UI の `attachBlock` をすり抜ける競合）や、送信直前にコピー元が消えた状態で「引き継いで送信」を押す。
- 修正案: C0 の段階（P のロック内）で、`args.attachments` がすべて P の台帳にあり `Ready` か、コピー元が実在するかを調べる。`prepare_attachments` と同じ条件を読取りだけで確かめ、だめなら `NotStarted` を返して何も作らない。C6 で失敗した場合は、コピーできた分を N から外すか、件数を `attach_note` で正確に出す（現在の文言はそうなっている）。そのうえで「元のチャットの入力欄にも残っています（一覧からは隠れています）」と案内する。

### M3 やり直しの途中で落ちると、起動後に N と N2 が両方先頭になる

- 場所: `host/continuity_handoff.rs:320-355`（やり直しの C4→C5）、`rules/continuity.rs:626-686`（`resolve_chains`）。
- 何が起きるか: やり直しでは、N2 の列を N の列と同じにして N を含めない（設計 §6.6）。N2 の chat.json を保存した後、N に印（C5）を書く前に落ちると、起動時の照合では P の主張者が N と N2 の2つになり、警告だけ出る。N（空・資料 Pending）と N2 が両方一覧に出て、A8「P と N が両方先頭として出ることはない」と K8「一覧に同じ会話が2行出ない」に反する。データは失われないが、同じ資料を2つの先頭が `Pending` で持つ。
- 再現条件: やり直しの C4 保存の直後にプロセスを落とし、再起動する（テスト `chains_redo_of_an_empty_head_is_not_a_conflict` の3例目と同じ状態）。
- 修正案（どちらか）:
  - (a) N2 の記録に「置き換えた空の先頭」を持たせる（例 `ConversationRecord.replaces: Option<ChatKey>`、`#[serde(default)]`）。`resolve_chains` で `replaces` を主張として扱い、N→N2 を修復する。
  - (b) やり直しのときだけ順序を逆にする。N に `supersededBy=N2` を先に書き、その後で N2 を保存する。N2 が読めなければ照合で N が先頭に戻るので、2行にならない。
  - どちらの場合も、`rules::continuity` の照合テストに「途中で落ちたやり直しを修復する」を足す。

### M4 案内の表示中、使用率が更新されるたびに履歴を全件読む

- 場所: `ui/HandoffNotice.tsx` の `useHandoff` の `useEffect`（依存に `usageStamp`・`running`）、`host/continuity.rs:144-177`（`preview_handoff` は毎回 `backend.read(include_turns: true)` と `queue_gate_input`（子孫の終端の再確認）を行う）。
- 何が起きるか: 閾値の案内は 70% 以上で出るので、読む thread は大きい。turn の実行中はモデル呼び出しのたびに `tokenUsage` が届き、`observedAt` が変わる。そのたびに大きい thread の全件読取りと子孫の確認が走る。実行中は `armed` が偽で、結果を使わないのに読んでいる。合意違反ではない（読取りのみ）が、§3.14 #18（負荷）に反する。
- 再現条件: 閾値を超えたチャットで長い turn を実行する。
- 修正案: UI では、`running` の間は取り直さず、`running` が真から偽になったときに1回だけ取り直す。依存から `usageStamp` を外す（割合の変化は表示側で見せれば足りる）。ホストでも、同じチャットの読取り結果を短い時間（例 5 秒）だけ使い回すか、`root_busy` で止まるときは履歴を読まずに理由だけ返す。後者は設計 §8「止める理由があっても資料は返す」の変更になるので、要判断として扱う。

---

## 低

- L1 版番号の再利用（`host/continuity.rs:48`）: `floor = max(現在の版, purpose_delivered_rev)` なので、「版3 を届けた → 版4 に編集（未送付か受理不明）→ 削除 → 作り直し」で、作り直した文も版4 になる。その後、受理不明だった版4 の送信が照合で受理と分かると、新しい版4 を届けたと誤って記録し、添えなくなる。修正: 最大の版番号を記録に残し（例 `ConversationRecord.last_revision`）、削除しても番号を増やし続ける。
- L2 照合での印のリセット（`host/continuity.rs:204-215`、`queue_driver.rs:1006`）: 受理不明の後、照合で受理が分かるまでの間に圧縮を観測していても、`compacted_since_delivery=false` に戻してしまう。`purpose_delivered_rev` も古い版へ戻る（その結果の再添付は害がない）。修正: 届いた記録は「送った時刻より後に圧縮を観測していなければ」だけ印を外す。版は `max` で更新する。
- L3 履歴で `Delivered` にする処理がない（設計 §3.1 の `SeedState::Delivered` の説明）: 受理から保存までの間に落ちると、資料が `Pending` のまま残り、次の手動送信で同じ資料をもう一度付ける。修正: 起動後に最初に履歴を読んだとき（または M1 の確認で）、最新の依頼を `parse_envelope` で読み、引き継ぎを含む資料なら `Delivered` にする。
- L4 `start_unknown` の範囲（`continuity_handoff.rs:63-65`）: `spawn_blocking` の失敗（作業フォルダの確認）も `Io` になり、「新しい thread を作れたか確認できません」と表示される。修正: バックエンドの応答待ちに由来する失敗だけを不明とし、ローカルの検査の失敗は `InvalidArgs` などへ分ける。
- L5 隠れた P での目的メモ編集（`host/continuity.rs:218`）: P の写しだけが変わり、先頭 N の会話には反映されない。修正: `superseded_by` があれば拒否し、「今の thread で編集してください」と案内する。
- L6 ロック待ちの送信（`mod.rs:954-956`）: 引き継ぎ中に別画面で P へ送ると、ロック待ちの後に P へ送られる（P は隠れた状態で turn が走る）。§9 は、隠れた P へ意図して送ることを許しているので、ロックを取った後に `superseded_by` を見て、待っている間に引き継がれたものだけ「引き継ぎ済みです。新しい thread で送ってください」で止めるのが妥当。
- L7 監視がきっかけの書込み（`host/continuity.rs:197-201`、`state.rs` `note_context_usage`）: 再開した外部の会話など chat.json のないチャットにも、`ensure_dir_id` と保存で領域を作る。修正: `hosted` か、既に chat.json があるチャットだけ保存し、それ以外はメモリだけに置く。
- L8 `thread/compacted` の正規化がない（`state.rs` の圧縮の観測は `ContextCompaction` の item が完了したときだけ）: item が出ない圧縮経路があれば、目的メモの再添付と「圧縮後」の表示が漏れる。実機 K2 で、手動の圧縮と自動の圧縮の両方で item が出るか確かめる。出なければ、通知を `BackendEvent` に足す。
