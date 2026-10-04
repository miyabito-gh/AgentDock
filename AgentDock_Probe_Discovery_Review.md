# AgentDock 未知子孫・再起動・停止完了照合

2026-10-04、正本v0.46・第48節の根拠。codex-cli 0.160.0、試行ごとの専用CODEX_HOME/work、localhost Responses/MCPを使用。既存認証コピー・クラウド推論・本体実装・HTML変更なし。

## 未知子孫と再起動

選択試行: [09:32:48.928 UTC result](probes/app-server/runs/2026-10-04T09-32-48-928Z/result.json)。生成tool応答を750ms遅延させ、生成結果が届く前に唯一の当該WSクライアントのTCP proxyを破棄。子／孫のローカルモデル要求到達をfixture側で確認してから接続を作り直した。再接続クライアントの探索入力はルート親IDのみ。生成結果から子IDを渡していない。

|場面|追加待機指定／実断時間|探索と結果|探索前後のモデル要求|
|---|---|---|---|
|子生成結果未受信|15000ms／15122ms|ancestorThreadId＋sourceKinds=subAgentThreadSpawn、limit=1で子1件、readで直接親照合|3→3|
|孫生成結果未受信|500ms／914ms|同じ祖先検索をcursorで全ページ取得し子・孫2件、readで直接親照合|6→6|
|専用サーバー終了・再起動|同一専用homeで再起動|loaded一覧空、祖先検索2件、親子孫read。子孫runtime=notLoaded、保存turn=interrupted|6→6|

TCP dropに約100ms待機を含む。2回目は500ms経過だけでは生成前になるため生成確認まで断を維持。実断時間は再接続開始前までで初期化時間を含まない。サーバー終了は当該試行の専用プロセスだけへkillを要求しexitを待った。任意クラッシュ・電源断の再現ではない。

探索ではresumeもturn/startも実行しない。listの既定scan/repairがあるため「ディスクへ一切書かない」の保証ではなく、追加推論なしの保存・状態取得の証拠。[公式契約](https://learn.chatgpt.com/docs/app-server)のreadと購読の差を維持する。

走査中の生成・ページ内容増減、大量生成、別sourceKind、archive、長時間idle、sleep、保存失敗は未確認。15秒を長時間断の合格にしない。notLoadedの保存履歴を現在runningと表示しない。

## 再接続後の停止照合

選択試行: [09:32:06.429 UTC result](probes/app-server/runs/2026-10-04T09-32-06-429Z/result.json)。別々の親から生成したキューなしの子をTCP断後の新規接続から停止。RPC timeout=3000ms、応答後500ms待機してread。

|準備|中断応答|readの同じturn|完了通知|モデル要求|
|---|---|---|---|---|
|readのみ、resumeなし|result={}|interrupted|0件|3→3|
|read後に子だけresume|result={}|interrupted|1件|6→6|

要求開始からreadまで543ms／537ms。threadId/turnIdと保存終端を照合し、API成功だけで状態を合成しない。resumeなしでも当該中断を確認できたがlive購読は復旧していない。resumeありはキューなしfixture比較であり監視目的の無条件resume採用ではない。親・別子孫・OS停止、保存全体完了、10秒保証は対象外。

## 不成立と試行の保持

- 09:29:45.580 UTCは未知子探索後のturn/interruptが20秒でtimeout。途中探索JSONの保存が当時なく測定全体の成功根拠に使わない。停止失敗／停止済みとも断定しない。原因未解決、原result・通知・コードsnapshotを保持。
- 09:30:58.914 UTCは手順passだが2回目の断中生成完了を再接続前に確認していなかったため、断中生成の根拠に選択しない。
- 最終09:32:48.928 UTCは両生成が断中だったことをfixture到達で確認、各探索後の途中証拠も保存。前述のtimeoutを修復済みとしない。独立した停止比較2条件の成立で原因解決とはしない。

batch-result.jsonは今回2測定を2試行から追加し、**12測定を11試行から統合**。不成立・選択しなかった全試行をtrialsへ保持。一度の全suite合格・製品受入合格ではない。今回選択2試行の既存config前後hash一致、node --check成立。認証内容は取得・コピーしない。

App Server第一候補・最終採用保留。B03の追加能力実測は今回行っていない。責任境界・18操作・取得方式・UI候補は[AgentDock_Responsibility_Technology_Review.md](AgentDock_Responsibility_Technology_Review.md)。次は走査中生成／長時間idle・購読副作用、停止競合・timeout、Plugins live反映・再信頼・部分失敗をまとめて照合する。
