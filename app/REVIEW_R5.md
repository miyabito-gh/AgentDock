# REVIEW_R5 — 段階⑤ 編集して再送・再生成（M50）と履歴で再確認（M52）

対象: P5-H（`394d3d7..0f7b081`、master）／P5-4u（worktree `agent-afd0bc91488ada6f6` の `908e4e3..c05d380`、未マージ）。
設計: DESIGN_P5 §5・§7A、正本 M50（1.3）、CLAUDE.md の合意。コードは変更していない。

## 結論

- 致命的（合意違反の自動再送・監視目的のresume・通信断を完了扱い）は**なし**。
  - resend の送信は `send_message` 1回だけ。分岐の受理不明・拒否・下書き保存失敗では送らない（`resend_next` で固定）。
  - 送信の受理不明の後は、ホストの `check_sendable`（`unknown_attempt`）が新しい送信を止めるため、照合が済むまで入力欄からの二重送信は起きない。
  - recheck は `read_history` だけ（resume・turn/start・interrupt なし）。読めないときは `Unreadable` で状態を変えず、判定は `tail_confirms_idle` で `unknown_confirmed_idle` と同じ。
- 不具合は**高1・中5・低8**。

## 不具合一覧

### 高

**H1 分岐先に元のチャットの権限・モデル・作業モードを渡さないまま、すぐ送る**
- 場所: `app/src-tauri/src/host/thread_ops.rs:306`（send）、`:363-381`（`register_forked_chat`/`adopt_forked` は `hosted`・`fork_of`・`cached_meta` だけを書く）、`app/src-tauri/src/codex/parity.rs:436`（`fork_params_json` は model・cwd・sandbox・approvalPolicy を渡さない）。
- 失敗シナリオ: 元のチャットで AgentDock の権限を「読取り専用」、モデルを X、作業モードを Plan にしていた。「もう一度」を押すと、分岐先の local には何もないので、`send_overrides` と `model_for_send` は既定を使う。Codex の `thread/fork` は上書きのない項目を config の既定から作るので、分岐先の最初のturnが**元より広い権限**・別のモデル・通常モードで走る可能性がある。継承されるかどうかは未確認。確認画面を経ずにすぐ送るため、通常の分岐より影響が大きい。
- 修正方針: `fork_inner` の受理後（purpose が EditResend・Regenerate のとき。できれば Fork でも）に、元の local から `permission`・モデル選択（`model_settings`）・`work_mode`・`worktree`（＋`next_cwd` があればそれ）を分岐先の local へ写して保存する。保存を待ってから送る。最初のturnの経路（`startFirst`）はすでに model・permission を写しているので、それとそろえる。写せなかった場合は送らず、`DraftOnly` 相当で止める。あわせて、実機で `thread/fork` の後の sandbox・approval・model・cwd を `thread/read` で確かめる。

### 中

**M1 分岐を受理不明から照合で採用すると、`purpose`・`through_turn` が失われる（すでに登録済みの記録も上書きされる）**
- 場所: `thread_ops.rs:420`（`ForkPurpose::Fork`・`through_turn: None` の固定値）、`:377`（`adopt_forked` が `fork_of` を無条件で上書き）、`backend/ipc.rs:651`（`ReconcileForkArgs` に目的がない）。
- 失敗シナリオ: (1) resend の分岐が受理不明 → 「分岐を確認」で採用 → ヘッダーのタグが「分岐」になり、名前にも「（編集）」が付かない（照合の経路では `rename_best_effort` を呼ばない）。(2) M2 の経路では、`fork_inner` が EditResend で登録した記録を、照合が Fork・終点なしで上書きする。
- 修正方針: `ReconcileForkArgs` に `#[serde(default)] purpose: ForkPurpose` と `#[serde(default)] through_turn: Option<ExternalId>` を追加する（古い呼び出しとの互換は default で保つ）。UI は `reconcileFork(chat, at, purpose, throughTurn)` で渡す。`adopt_forked` は、既存の `fork_of` が同じ分岐元を指しているときは上書きしない（`f.fork_of.get_or_insert(origin)`。別の分岐元を指しているときは記録を変えず補足を返す）。照合で採用したときも、目的が Fork 以外なら `rename_best_effort` で「（編集）」「（再生成）」を付ける。保存形式は追加フィールドだけなので、後方互換は保てる。

**M2 分岐の後に送信の前提検査で止まると、`?` で Err になり分岐先の情報が UI に届かない**
- 場所: `thread_ops.rs:306`（`send_message(...).await?`）、`app/src/ui/ResendDialog.tsx:158-160`（catch → 「結果を確認できませんでした」＋`unknownAt`）。
- 失敗シナリオ: 分岐は受理され、下書きも保存済み。その後 `ensure_live`（resume）の失敗や `check_sendable` で Err になる。この Err は送る前に止まったもので、送信は起きていない。UI は分岐が不明になったと案内し、「分岐を確認」で照合 → M1 の上書き → `saveDraft` で同じ文をもう一度書く。分岐先はできていて送信もしていないのに、利用者には不明と表示される。
- 修正方針: 送信前の Err は `Ok(ResendAsForkResult { fork, draft_saved: true, send: None, send_error: Some(msg) })` で返す（`send_error: Option<String>` を追加）。UI は `forkedLines` で「分岐して入力欄に入れましたが、送れませんでした（理由）。送信は入力欄から行ってください」と出し、`h.forked(key, text, null)` を呼ぶ。今の `!send` の分岐（`ResendDialog.tsx:95`）はこの用途に使える。

**M3 実行中にダイアログを閉じられ、ホストに同じチャットの実行中の印がないため、二重に分岐・送信できる**
- 場所: `app/src/ui/Dialogs.tsx:55,57,62`（scrim・×・キャンセルはいつでも `onClose`）、`ResendDialog.tsx:139-162`、`thread_ops.rs:317-345`（`busy` に進行中の resend を含めない）。
- 失敗シナリオ: 「分岐して送信」を押す → 応答待ちの間に閉じる → 同じ吹き出しから開き直して、もう一度押す。分岐元は idle のままなので、ホストは2本目も通す。分岐が2つでき、同じ依頼が2回送られる。1回目の結果（受理不明を含む）は画面から消え、完了後に `h.forked` が選択を勝手に移す。
- 修正方針: (1) ホストで分岐元のチャットごとに実行中の印を持つ（`resend_inflight: Mutex<HashSet<ChatKey>>` と RAII ガード）。`resend_ctx.busy` にも含め、2本目は `ChatBusy` にする。(2) UI は `running` の間は閉じられないようにする（`onClose` を無視し、×とキャンセルを無効にする）。(3) アンマウント後は `h.forked` を呼ばない。

**M4 送信が受理不明のとき依頼の文を入力欄に残すと、照合で受理が確定した後に二重送信しやすい（設計との差ではなく、設計自体の問題）**
- 場所: `rules/thread_ops.rs:221`（`clear_draft_after` が Accepted だけで真）、`ResendDialog.tsx:155`（受理不明でも text を入力欄へ）。
- 失敗シナリオ: 送信が受理不明 → 入力欄に文が残る（照合が済むまでは `check_sendable` が止める）→ 照合で「受理済み」と確定 → 送信の止めが外れ、入力欄には同じ文が残ったまま → 利用者が送ってしまい二重送信になる。通常の送信（`App.tsx` の `onSend`）では、受理不明でも入力欄を空にしているので、扱いが食い違う。
- 修正方針: 受理不明でも下書きを消す（`clear_draft_after` を `Accepted | AcceptanceUnknown` にする）。文は送信試行の記録（照合・再送の帯）で見せる。拒否のときは今どおり残す。DESIGN_P5 §5 手順5の記述もこれに合わせて直す。

**M5 最初のturnの経路で下書き保存に失敗すると例外になり、押し直すたびに空のチャットが増える**
- 場所: `app/src/App.tsx:818-829`、`ResendDialog.tsx:164-173`。
- 失敗シナリオ: `startChat` は成功（空のチャットができ、選択も移る）→ `setDraft` が失敗して throw → ダイアログが `finished:false` のまま「新しいチャットへ入れられませんでした」と出す → もう一度押すと、2つ目の空チャットができる。
- 修正方針: `startFirst` は、チャットを作った後の保存失敗では throw しない。`{ key, draftSaved: false, message }` を返し、UI は `finished:true` にして「作りましたが入力欄への保存に失敗しました（文は入力欄に表示）」と出す。`startChat` 自体が IPC エラーのときは、チャットの作成が不明として「一覧を更新して確認」と案内し、押し直しの前に一覧の確認を促す。

### 低

- **L1** ホストが依頼0件のturn（レビュー・圧縮など）を通す。`thread_ops.rs:321` は `target_request_count == 0` でも止めない（UI は `noText` で止める）。`resend_blocker` に「依頼の文がないturnは対象外」（`count == 0`）を加える。
- **L2** UI とホストで、受理不明の送信の扱いが違う。ホストは分岐元の `unknown_attempt` を busy に含める（`thread_ops.rs:332`）が、UI の `resendCtxOf`（`ResendDialog.tsx:32`）は見ない。UI では押せるのに、ホストは「実行中は使えません」と返し、理由が合わない。UI に `attempts[chat]` の受理不明を入れるか、ホストの理由を「前の送信の受理を確認できていません」に分ける。
- **L3** 最初のturnかどうか（`resend.ts:79` の `i === 0`）を UI の手元のturnだけで決め、ホストが検査しない。`openChat` に失敗した後、live 通知だけで積まれたturn列は途中からなので、2番目以降のturnでも「新しいチャットに入れる」経路になり、文脈が失われる（送信はしない）。`bundle.turns` が `openChat` 由来であることを確かめてから first を許すか、ホストで最初のturnを照合する（読取り）。
- **L4** `startFirst` が worktree を引き継がない（`App.tsx:825` の `worktree=null`）。元が AgentDock の worktree のチャットだと、新しいチャットは同じパスで作られるが記録に結び付かず、worktree の削除確認に出てこない。元の `local.worktree` を渡す。M50 の「同じ種類」も、`src.kind` を見ず cwd の有無で決めているので、あわせて確認する。
- **L5** 照合で採用した分岐先の下書きを、確認なしで上書きする（`ResendDialog.tsx:182`、`App.tsx:814`）。候補がちょうど1件でも、利用者が別の経路で作った分岐の可能性がある。既存の下書きが空でなければ上書きせず、文はダイアログに残して案内する。
- **L6** IME の変換中に Ctrl+Enter で送れる（`ResendDialog.tsx:208`）。`e.nativeEvent.isComposing` のときは無視する。文の長さに上限がない（ホスト・UI とも）。通常の入力欄と同じ上限があればそろえる。
- **L7** ダイアログの外枠と本文で、判定の基になる値が違う。外枠の閉じるボタンの文言（`Dialogs.tsx:207` は現在の `st.plan` で決める）と、本文（`useState(s.plan)` で開いた時点に固定）が食い違う。開いた後に状態が変わると、本文は実行可能なのに外枠は「閉じる」と表示される。表示だけの問題。
- **L8** recheck で読めなかったとき、`read_tail_fact` が `queue_rt.terminals` を空の事実で上書きする（`queue_driver.rs:461`）。前に確認できていた終端が消え、キューが保留に戻ることがある。安全側に倒れる挙動で合意違反ではないが、Err のときは既存の事実を残す（insert しない）ほうが「何も変えない」という説明と合う。

## 確認して問題なしとした点

- resend の順序（検査 → 分岐 → 名前 → 下書き保存を待つ → 1回送る）と、`resend_next`／`clear_draft_after` のテストは、設計 §5 どおり。
- 添付は引き継がない（`attachments: Vec::new()`）。分岐先の local に元の添付・キューは入らない。
- ホストの再検査（TOCTOU）: `resend_ctx` は実行時に履歴を読み直して照合する（`target_follows_through` で表示が古いものを止める）。`fork_inner` も分岐元が実行中かを再度検査する。子孫が作業中なら `busy`。キューに送信予定があっても、分岐元は変えないので可とした（設計どおり）。
- 9条件の順と文言は、UI とホストでそろっている。UI だけにあるのは followUp と noText。
- recheck の対象の選別（メイン・live・作業中・他チャット・Unsupported を除く。重複は1件にまとめる）。終わったら `kick_queue` だけを呼び、キューを直接送らない。
- `ForkOrigin.purpose` は `#[serde(default)]` で、古い chat.json が読める。webview_cache は `Known` で区別し、0で代用しない。

## 実機確認で見てほしい点

1. `thread/fork` の直後の分岐先で、model・sandbox・approval・cwd が元と同じかどうか（H1）。元を「読取り専用」にして「もう一度」を押し、分岐先の turn がファイルを書けないこと。
2. 「分岐して送信」→ 受理 → 分岐先が選ばれ、名前が「…（再生成）」、タグが「再生成」、元の会話には何も追加されないこと。
3. 分岐の受理不明は再現しにくい。App Server の応答を遅らせられれば、「分岐を確認」の後のタグと名前を見る（M1）。
4. 応答待ちの間にダイアログを閉じて、開き直せるかどうか（M3）。
5. 最初のturnの「新しいチャットの入力欄へ」で、作業フォルダ・モデル・権限が元と同じこと。worktree のチャットの場合の扱い（L4）。
6. 状態不明の子の「履歴で再確認」で、トーストの件数と、Codex 側で `thread/resume` が出ていないこと（App Server のログ）。
