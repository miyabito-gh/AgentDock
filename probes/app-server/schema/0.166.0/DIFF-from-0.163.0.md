# スキーマ差分 0.163.0 → 0.166.0
## stable
- ClientRequest: +1 / -0
    + feedback/save
- ServerNotification: +0 / -0
- ServerRequest: +0 / -0
- 型(v2): 追加 3 / 削除 0 / 変更 34
    + FeedbackSaveParams.json
    + FeedbackSaveResponse.json
    + ModelProviderError.json
    ~ AccountRateLimitsUpdatedNotification（中身の変更）
    ~ AccountUpdatedNotification（中身の変更）
    ~ ConfigRequirementsReadResponse（中身の変更）
    ~ ErrorNotification（中身の変更）
    ~ GetAccountRateLimitsResponse（中身の変更）
    ~ GetAccountResponse（中身の変更）
    ~ GetAccountTokenUsageResponse（中身の変更）
    ~ HookCompletedNotification（中身の変更）
    ~ HooksListParams（項目追加: threadId）
    ~ HooksListResponse（中身の変更）
    ~ HookStartedNotification（中身の変更）
    ~ ItemCompletedNotification（中身の変更）
    ~ ItemStartedNotification（中身の変更）
    ~ ListMcpServerStatusResponse（中身の変更）
    ~ PluginReadResponse（中身の変更）
    ~ ProcessExitedNotification（項目追加: outputDrainTimedOut）
    ~ ReviewStartParams（中身の変更）
    ~ ReviewStartResponse（中身の変更）
    ~ ThreadForkResponse（中身の変更）
    ~ ThreadItemsListResponse（中身の変更）
    ~ ThreadListResponse（中身の変更）
    ~ ThreadMetadataUpdateResponse（中身の変更）
    ~ ThreadReadResponse（中身の変更）
    ~ ThreadResumeResponse（中身の変更）
    ~ ThreadRevertResponse（中身の変更）
    ~ ThreadStartedNotification（中身の変更）
    ~ ThreadStartResponse（中身の変更）
    ~ ThreadTurnsListResponse（中身の変更）
    ~ ThreadUnarchiveResponse（中身の変更）
    ~ TurnCompletedNotification（中身の変更）
    ~ TurnStartedNotification（中身の変更）
    ~ TurnStartParams（中身の変更）
    ~ TurnStartResponse（中身の変更）
    ~ TurnSteerParams（中身の変更）

## experimental
- ClientRequest: +7 / -0
    + account/gcp/setup
    + accountSession/list
    + accountSession/switch
    + feedback/save
    + thread/memory/read
    + thread/sample
    + thread/sample/cancel
- ServerNotification: +0 / -0
- ServerRequest: +0 / -0
- 型(v2): 追加 15 / 削除 0 / 変更 41
    + AccountSessionsListParams.json
    + AccountSessionsResponse.json
    + AccountSessionsSwitchParams.json
    + AccountSessionsSwitchResponse.json
    + FeedbackSaveParams.json
    + FeedbackSaveResponse.json
    + GcpSetupParams.json
    + GcpSetupResponse.json
    + ModelProviderError.json
    + ThreadMemoryReadParams.json
    + ThreadMemoryReadResponse.json
    + ThreadSampleCancelParams.json
    + ThreadSampleCancelResponse.json
    + ThreadSampleParams.json
    + ThreadSampleResponse.json
    ~ AccountRateLimitsUpdatedNotification（中身の変更）
    ~ AccountUpdatedNotification（中身の変更）
    ~ ConfigRequirementsReadResponse（中身の変更）
    ~ ErrorNotification（中身の変更）
    ~ GetAccountRateLimitsResponse（中身の変更）
    ~ GetAccountResponse（中身の変更）
    ~ GetAccountTokenUsageResponse（中身の変更）
    ~ HookCompletedNotification（中身の変更）
    ~ HooksListParams（項目追加: threadId）
    ~ HooksListResponse（中身の変更）
    ~ HookStartedNotification（中身の変更）
    ~ ItemCompletedNotification（中身の変更）
    ~ ItemStartedNotification（中身の変更）
    ~ ListMcpServerStatusResponse（中身の変更）
    ~ PluginReadResponse（中身の変更）
    ~ ProcessExitedNotification（項目追加: outputDrainTimedOut）
    ~ ReviewStartParams（中身の変更）
    ~ ReviewStartResponse（中身の変更）
    ~ ThreadForkParams（項目追加: attachToSource, experimentalDisablePredictionMcp）
    ~ ThreadForkResponse（中身の変更）
    ~ ThreadItemsListResponse（中身の変更）
    ~ ThreadItemsReadResponse（中身の変更）
    ~ ThreadListParams（項目追加: attachedToThreadId）
    ~ ThreadListResponse（中身の変更）
    ~ ThreadMetadataUpdateResponse（中身の変更）
    ~ ThreadQueueStartResponse（中身の変更）
    ~ ThreadReadResponse（中身の変更）
    ~ ThreadRealtimeStartParams（項目追加: voiceSessionId）
    ~ ThreadResumeResponse（中身の変更）
    ~ ThreadRevertResponse（中身の変更）
    ~ ThreadSearchResponse（中身の変更）
    ~ ThreadStartedNotification（中身の変更）
    ~ ThreadStartResponse（中身の変更）
    ~ ThreadTimelineListResponse（中身の変更）
    ~ ThreadTurnsListResponse（中身の変更）
    ~ ThreadUnarchiveResponse（中身の変更）
    ~ TurnCompletedNotification（中身の変更）
    ~ TurnStartedNotification（中身の変更）
    ~ TurnStartParams（項目追加: responsesapiClientMetadataPatch）
    ~ TurnStartResponse（中身の変更）
    ~ TurnSteerParams（項目追加: responsesapiClientMetadataPatch）

## 互換を壊しうる変更（深い比較。項目の削除・必須化・列挙値の削除）
- /definitions/ThreadItem/oneOf[15]: 項目削除 result（21か所: ItemCompletedNotification, ItemStartedNotification, ReviewStartResponse ほか）
- /definitions/ReviewDelivery: 列挙値の削除 detached（1か所: ReviewStartParams）

## 判定
- AgentDockが使っているメソッドで、新版から無くなったもの: なし
- 項目の削除・必須化があった型（使用箇所は要確認）: なし
- 結論: 互換を壊しうる変更あり。AgentDockが使っているかを、一覧で確認してから、確認済みにする。

## 使用箇所の確認（2026-11-03、AgentDockのソースを確認）
- `ReviewDelivery` の `detached` 削除: AgentDockは `review/start` を常に `delivery: inline` で送る（app/src-tauri/src/codex/parity.rs の `review_params_are_always_inline_and_never_detached`）。影響なし。
- `ThreadItem`（imageGeneration）の `result` 削除（`generationId`・`inputImages` が追加）: AgentDockは `imageGeneration` を `status` だけ読む（app/src-tauri/src/codex/convert.rs）。影響なし。
- 安定版の追加メソッドは `feedback/save` のみ。使っているメソッドの削除なし。実験版の追加（`thread/sample`、`thread/memory/read`、`accountSession/*`、`account/gcp/setup`）は未使用。
- 結論: スキーマ上は、0.163.0 → 0.166.0 は AgentDock にとって後方互換。実Codex 0.166.0 での接続・送信・承認の実機スモークは未実施（実機確認後に「実機確認済み」にする）。
