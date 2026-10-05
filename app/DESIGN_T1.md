# T1 設計メモ: AIバックエンド境界・共通データモデル・IPC契約

対象: 正本 `AgentDock_Requirements.md` §2.1〜2.3、§3.1〜3.6・3.9、§4、§5、§6、§11。区分は正本§0.2に従い、本書の内容はすべて**設計案**。

成果物: `src-tauri/src/backend/{mod,model,backend,ipc}.rs`、`src/ipc/types.ts`。型・traitのみで実装の中身はない。

## 1. 責任分担

| 層 | 持つもの | 持たないもの |
|---|---|---|
| UI（React） | `types.ts` の正規化型の表示、ユーザー操作をコマンド化 | 状態判定・タイマー・再送判断・Codex固有の型 |
| IPC（`ipc.rs`） | コマンド引数/戻り値、`HostEvent`、`UserConfirmed` の発行 | 業務判断 |
| ホスト監視層（T2/T3の上、後続） | 状態と鮮度の保持、後着の破棄、キュー保留、停止照合（10秒判定もここ）、通知集約、保存 | プロトコル詳細 |
| `AiBackend` 実装（T3） | Codex型⇔正規化型の変換、能力宣言、照合の具体手段 | 状態の上書き判断、再送 |
| JSON-RPCクライアント（T2） | stdioの要求/応答対応、サーバー要求の受付、通知の順序付き転送 | 意味解釈 |

## 2. イベントの流れ

```
codex app-server ─stdio─▶ T2 JsonRpc ─(method, params, seq)─▶ T3 Adapter
   ─EventEnvelope{source, seq, receivedAt, BackendEvent}─▶ (mpsc, 単一消費者) ─▶ ホスト監視層
   ─HostEventEnvelope{seq(全体), HostEvent}─▶ Tauri emit "agentdock://host-event" ─▶ UI
UI ─invoke(name, {args})─▶ ホスト（UserConfirmed発行）─▶ AiBackend ─▶ T2 ─▶ app-server
```

- 受け口は有界mpscの単一消費者。アダプターは溢れ・パース失敗を黙って捨てず `BackendEvent::Gap` を送り、ホストは該当範囲を `needsReconcile` にする。
- 未知メソッドは `Unrecognized`（警告表示、M12）。終了しない。
- UIは `get_snapshot` で全体を取り、以後 `seq` 順に差分適用。欠番・renderer再読込み時は再度snapshot。
- `ActivityDelta` は描画専用。状態判定に使わない。承認・質問（`RequestUpdated`）は2秒集約の対象外で即時。

## 3. 状態遷移の要点

- **状態（§4.1）と鮮度（§4.2）は別フィールド**（`AgentView.status` / `AgentView.freshness`）。切断は鮮度だけを `disconnected` にし、状態は最後の明示値のまま。通信断・timeout・notLoaded を `done`/`failed` にしない（`unknown` か据え置き）。
- `AgentStatus.turn` と `TurnKey` で古いturnの後着を捨てる。終端（`TurnEnded`）は明示イベントか履歴の `TurnRecord.end` だけから作る。
- `read` は live を戻さないので結果は `historyOnly`。`live` 以外ではキュー自動送信を保留（`Freshness::allows_auto_send`）。
- 不明値は `Known<T>`（value＋basis{direct/derived/inferred} / notFetched / unsupported / missing）。UIは notFetched を「未確認」と表示。
- 送信: `send()` は `Err` を返さず `Accepted / Rejected / AcceptanceUnknown` に必ず分類。`SendRequest`・`UnconfirmedSend` は非Clone。受理不明→ `reconcile_send` → `NotFound(NotAccepted)` → `SendRequest::retry(NotAccepted, UserConfirmed, 新ID)` の経路以外で再送できない。
- 中断: `interrupt()` は `InterruptAck`（受付のみ）。停止は `StopRecord.targets[].evidence` に「中断要求／turn終端／管理実行終了／OS実体消滅」を別々に記録し、要約 `StopSummary` は機械的導出。`blocks_deletion()` が真の間は削除をBlockedで拒否。
- 状態を変える trait メソッド（resume・manage・respond・interrupt）は `&UserConfirmed` を要求。`UserConfirmed` は `pub(crate)` 生成でIPCコマンド層だけが作る（監視・自動処理から呼べない、M11）。

## 4. 引き継ぎ

**T2（stdio JSON-RPC）**
- 要求IDと応答の対応表。timeout・切断時は保留中の全要求を `BackendError::OutcomeUnknown` で完了させる（`Rejected` にしない）。エラー応答だけが `Rejected`。
- サーバー要求のIDは数値/文字列を可逆に `ExternalId` へ符号化（例 `n:5` / `s:abc`）し、応答時に元の型へ戻す。`RequestKey` には接続ごとの `SourceId` を含める。
- 通知は受信順に `seq` を振って転送。自動再送・自動回答をしない。`initialize` で experimental の有効/無効を `ConnectConfig.enable_experimental` に従って指定。
- 必要な依存（T2で追加）: tokio の `process, io-util, rt, time, macros`。

**T3（Codexアダプター）** 変換の初期案（schema 0.160.0 stable）
- `ThreadStatus`: notLoaded→`unknown`（履歴で終端があれば turn scope で補完）、idle→`idle`、active→`running`、active[waitingOnApproval]→`waiting(approval)`、active[waitingOnUserInput]→`waiting(userInput)`、systemError→`unknown`＋警告（失敗の明示か未確認のため保守的に）。
- `TurnStatus`: completed→`done`、failed→`failed`、interrupted→`interrupted`、inProgress→`running`（いずれも scope=turn）。
- 要求: commandExecution/fileChange/permissions の requestApproval、tool/requestUserInput、mcpServer/elicitation を `RequestKind` へ。選択肢は受信内容から作り（`acceptForSession`→scope=session、execpolicy/network amendment→persistent）、固定一覧にしない。未知要求・`item/tool/call` 等は `Other` で表示し自動回答しない。`serverRequest/resolved`→`RequestResolved`。
- 親子: `parentThreadId`→`ParentLink::Explicit`、`forkedFromId` は別項目、`agentRole`/`agentNickname`→role/displayName。深さから親を推定しない。
- 送信: `client_message_id`→`turn/start.clientUserMessageId`。照合は `thread/read` の userMessage `clientId` で行う。Steer→`turn/steer`。
- 一覧: `thread/list` の `sourceKinds` を明示（appServer・cli・vscode・exec・subAgent系）。子孫再発見は experimental の `ancestorThreadId` を使い、`complete=false` なら再走査。
- 能力宣言: descendantSearch・managedExecControl＝experimental、ピン＝アプリ側（バックエンド能力ではない）。

**T4（画面）**
- `types.ts` だけを使う。状態バッジと鮮度表示を並べて出し、片方から他方を導かない。`Known` の欠け方ごとに「未確認／非対応／欠損／推定」を表示。
- `capabilities` が unsupported の操作は隠すか「このAIでは非対応」。`sendState=acceptanceUnknown` の間は再送ボタンを出さない。
- 停止未確認（10秒）の判定はホストが `stopUpdated` で送る。UIにタイマーを置かない。
- 監視範囲の切替（`set_monitor_scope`）やカード選択で送信先・中断・承認を変えない。

## 5. 設計時に決めた事項（§11の範囲、ユーザー判断不要）

1. 識別子＝`BackendKind`＋不透明 `ExternalId`。要求は監視元 `SourceId` も含む。時刻は Unix ms（i64）。
2. 不明値4区分＋根拠3区分の `Known<T>`。
3. イベント配送＝有界mpsc単一消費者＋`Gap`、UIへは全体seq＋snapshot再同期。
4. 送信・再送・ユーザー操作を型（非Clone・`UserConfirmed`）で制約。
5. IPCはコマンドごとの関数で引数は `args` 1個、失敗は `IpcError{code, blocked}`。
6. ピンはアプリ側保持。モデル選択は送信時に適用し、受理応答までは `accepted=notFetched`。
7. experimental API: `ConnectConfig.enable_experimental` で接続単位に切替。初期値の推奨は有効（子孫再発見に必要）。使えない場合は該当能力を unsupported に降格。
8. systemError は `unknown`＋警告（保守側）。実測で失敗の明示と確認できれば `failed` に変更する。

**ユーザー判断が必要な事項: なし**（上記はすべて§11の設計裁量内）。
