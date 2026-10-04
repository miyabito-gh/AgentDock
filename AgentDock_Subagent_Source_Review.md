# CLI 0.160.0 子孫監視の公開ソース照合

確認日: 2026-10-04（日本時間）  
対象リポジトリ: openai/codex  
取得ref: rust-v0.160.0  
方法: GitHub接続のfetch_fileでタグを明示した読取り。検索はファイル位置の発見のみ。

## 結論

**子・孫の識別、直接親、生成後のイベント購読に、対象タグのソース根拠がある。** 従来の「最新文書に候補がある」から「対象版公開型・実装経路を確認」へ進んだ。インストール済みCLIの実行成功や、全生成経路・再接続・欠落回復の保証には進めない。

## 確認した3点

|問い|対象タグで確認したこと|判定・限界|
|---|---|---|
|子・孫のIDと直接親を取得できるか|Threadの公開型にid・parentThreadId・agentNickname・agentRole・statusがある。保存スレッドから返却型への変換にもparent_thread_idがある|公開型・返却処理の根拠あり。親不明や役割未取得はnullを保持する。全種類の内部workerに直接親が付くとは断定しない|
|各エージェントのイベントを購読できるか|AgentControlのspawnがthread生成通知を出す。App Serverの受信ループが初期化済み接続へlistenerを付け、listenerが各conversationのイベントをその購読接続へ送る|同じApp Server内で生成されたスレッドの通常経路に根拠あり。外部CLIへの後付け監視の証明ではない。孫も同じspawn経路を通るなら同じ機構に乗るという設計推論であり、孫の実行記録は未取得|
|購読のために作業を再開・変更せずに済むか|生成通知→try_attach_thread_listener→ensure_conversation_listenerは、既存threadを取得して購読とlistenerを登録する経路。ここでthread/resumeや新しい依頼を送る処理は確認されない|初期化済み接続に対する新規生成時の観測は、resumeを監視の標準経路にせず設計できる見込み。既存threadへの再接続を同じ結論で保証しない|

公開型のsessionIdやforkedFromIdは、直接親を示すparentThreadIdと別に保持する。分岐関係やセッションの一致だけでspawn親子へ変換しない。

## 根拠ファイル

リンクはすべて取得時に指定した対象タグ。行番号は取得本文に対する番号で、検索結果のdefault branchを引用したものではない。blob SHAはファイル内容の識別子であり、リリースcommit SHAではない。

|ID|ファイル・箇所|blob SHA|
|---|---|---|
|C01|[Thread.ts](https://github.com/openai/codex/blob/rust-v0.160.0/codex-rs/app-server-protocol/schema/typescript/v2/Thread.ts#L14) 14〜26、65〜92行|9a0f865d7b42a489063b07324d6f5c6b0c4a29c3|
|C02|[thread_processor.rs](https://github.com/openai/codex/blob/rust-v0.160.0/codex-rs/app-server/src/request_processors/thread_processor.rs#L3607) 3607〜3639、6078〜6084行|289cb63953662eb5fba00222ec49432dd976b944|
|C03|[spawn.rs](https://github.com/openai/codex/blob/rust-v0.160.0/codex-rs/core/src/agent/control/spawn.rs#L872) 872〜875行|277ce112bd847022c9abd8622fe8af539b37c718|
|C04|[lib.rs](https://github.com/openai/codex/blob/rust-v0.160.0/codex-rs/app-server/src/lib.rs#L1282) 1282〜1311行|d1a622ca8cc94ab626e8e377333c9d875cd18451|
|C05|[thread_lifecycle.rs](https://github.com/openai/codex/blob/rust-v0.160.0/codex-rs/app-server/src/request_processors/thread_lifecycle.rs#L140) 140〜187、302〜350行|b0fa49b4babcc5802a9968ea26bc230add7cba17|
|C06|[bespoke_event_handling.rs](https://github.com/openai/codex/blob/rust-v0.160.0/codex-rs/app-server/src/bespoke_event_handling.rs#L973) 973〜1010行|d1b0ab89756d38579d0d976ee72a5ee80cc216f2|
|C07|[AgentMessageDeltaNotification.ts](https://github.com/openai/codex/blob/rust-v0.160.0/codex-rs/app-server-protocol/schema/typescript/v2/AgentMessageDeltaNotification.ts) threadId・turnId・itemId・delta|b47985e5b7c312fad05615afcaad84567e7dc543|
|C08|[thread.rs](https://github.com/openai/codex/blob/rust-v0.160.0/codex-rs/app-server-protocol/src/protocol/v2/thread.rs#L1441) 1441〜1449行|2e166d5506b10996e2da286feb6082d6b8711340|
|C09|[ThreadListParams.ts](https://github.com/openai/codex/blob/rust-v0.160.0/codex-rs/app-server-protocol/schema/typescript/v2/ThreadListParams.ts) チェックインされた標準生成型|854b6ef92cb69a91abaf2dce09165745ec1ab4eb|

C06では旧Collab/SubAgentActivityイベントをそのままv2通知へ変換せず、TurnItem lifecycleを使う旨がある。親の協調イベントや内部EventMsgだけを見て、クライアントが受信するv2通知を決め打ちしない。C05・C07により所属thread/turn/itemの識別と発言差分の経路を確認した。全ツール・全活動種別の網羅性は未確認。

初回に推測した旧配置のcodex_message_processor.rsとprotocol/v2.rsは404。検索で現在の配置を発見し、その後に対象タグで取り直した。404を機能欠如の証拠にはしない。

## 残る制約

1. **通知欠落の回復**: C04の生成通知受信がLaggedの場合、ログを残し再同期を省略する。過負荷時に全子を捕捉できる保証がない。生成数・接続・読み取り速度による実際の発生条件は未測定。
2. **購読失敗**: C02のattachはbest-effort。接続終了・thread閉鎖中等で失敗する経路がC05にある。アプリ側が失敗を識別し、見落としを表示・回復できるかは未確認。
3. **再接続**: C04が対象にするのは生成時点の初期化済み接続。後から接続したクライアントや再接続先へ全既存子孫が自動購読されるとは、この経路だけでは証明できない。warm resumeには既存live状態を使う処理があるが、監視のみで許容できる副作用範囲をまだ確認していない。
4. **experimentalの差**: C08に親・祖先検索フィールドとexperimental注記が存在する一方、C09の標準生成型には両フィールドがない。標準生成物に載らないことを不存在と判定せず、生成オプション・opt-in・対象版の実際の受理を別途確認する。parentThreadIdの返却と、親IDで検索する要求は別契約。
5. **状態変換**: ThreadStatusの公開型はnotLoaded／idle／systemError／activeで、製品のdone・failed・interrupted等をすべて直接含むわけではない。turn・itemの結果と合わせて変換する設計が必要。無出力やnotLoadedをdoneと扱わない。

## 要件と採用判定への反映

- V02: 対象タグのチェックイン生成型とRust型を一部確認。ユーザー環境でのschema生成は未実施。
- V04: 識別・直接親・生成後購読の公開実装根拠ありへ更新。孫実行・全経路・欠落回復・再接続は未確認。
- M11: 新規生成の自動attachを優先する設計候補。監視目的のresumeを標準にしない規則は維持。
- M09・M12: 通知欠落と再接続時の回復経路が成立するまで達成済みとしない。
- 最終採用: 依然保留。今回のソース確認は第一候補を維持する根拠を強めるが、実停止・Plugins・他の同等性・公式サポート範囲を解決していない。

次の文書照合では、生成通知欠落・再接続後に既存子孫へ安全に購読を戻す経路を追う。その後に親・子孫・ツールの中断と停止証拠を確認する。実装、App Server起動、schema生成、テスト実行、設定変更は行っていない。
