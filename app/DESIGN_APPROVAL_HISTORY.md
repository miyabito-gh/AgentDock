# 項目51 設計メモ: 応答した承認・質問・MCP許可の記録

## 1. 実測（調査）
- 静的確認（schema 0.160.0 `ThreadItem`）: 項目の種類は userMessage / agentMessage / reasoning / plan / commandExecution / fileChange / mcpToolCall / dynamicToolCall / collabAgentToolCall / webSearch / imageView / imageGeneration / contextCompaction / enteredReviewMode / exitedReviewMode / hookPrompt / functionCallOutput / sleep / subAgentActivity。**承認・質問・elicitation の項目は無い**。
- 履歴に残るのは結果だけ: `commandExecution`・`fileChange` の status に `declined` があり、拒否された事実は読めることがある。許可（承認）自体、質問への回答、MCP elicitation の応答は履歴に出ない。要求元の子・時刻・選んだ範囲（今回のみ等）も出ない。
- 実PCの `thread/read` による実測は**未実施**（本タスクでは静的確認のみ）。実機で承認後の履歴に承認系の項目が無いことの確認は、実機確認の手順に入れる。

## 2. 設計
- 履歴から復元できないので、**ユーザーが押した結果（事実）だけ**を AgentDock のチャット別領域 `chats\<dirId>\decisions.jsonl`（追記、1行=`DecisionEntry{schemaVersion, record}`）へ残す。会話本文は保存しない方針（DESIGN_P2 §1.3）に合わせ、内容は80文字までの要約だけ（コマンド全文・長いパス・URLは切る、質問の答えの本文と秘密入力は保存しない）。
- 記録する状態: 応答済み（許可・辞退・取りやめ・回答）、受理未確認（応答を送ったが確認できない。再送しない）、別の場所で解決（AgentDockの応答ではないと明記）。保留・期限切れ（切断）・取消は記録しない。
- 取りこぼし防止: 記録は `HostData::note_decision`（`respond_request` と `RequestResolved` の2か所）で積み、`mutate` の直後に書込みtaskへ渡す（応答の代行・再送には使わない）。書込みは `io_lock` の中で、削除中のチャットには書かない（activity と同じ）。
- 保存失敗: 容量不足・I/O失敗は `DecisionRecorded{save: failed}` で画面に出し、行に「保存できていません。再起動後には残りません」を付ける。自動削除・自動再試行はしない（`decisions.jsonl` は使用量ではメタデータに数える。チャット削除で一緒に消える）。
- 表示: App が `list_decision_records`（読取りのみ）でチャット選択時に読み込み、`decisionRecorded` 通知を合わせる（`ui/decisions.ts`）。同じ(agent, turn)の末尾に小さな行として差し込み、turn不明は時刻で最後の開始済みturnへ（履歴の各項目は時刻を持たないので、turn内の前後は決めない）。AgentDock導入前・外部で応答したものは記録がないだけで、「記録なし」とは表示しない（行を出さない）。
- 制限: 履歴にも保存にも無い過去の応答は表示できない。他のクライアント（Codex CLI等）で応答したものは、AgentDockが `RequestResolved` を受けたときだけ「別の場所で解決」として残る。

## 3. 変更ファイル
Rust: `backend/decision.rs`（型）、`rules/decision.rs`（組立て・テスト）、`store/{layout,mod}.rs`（追記・読込）、`host/{state,mod,persist}.rs`、`backend/ipc.rs`（`DecisionRecorded`）、`commands.rs`・`lib.rs`（`list_decision_records`）。TS: `ui/decisions.ts`（テスト付き）、`App.tsx`、`ui/CenterPane.tsx`、`ipc/{types,client,live}.ts`。
