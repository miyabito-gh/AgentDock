# 隔離App Server追加検証の一括結果

確認日: 2026-10-04（日本時間）。正本v0.44。前回の承認と引継ぎ依頼に基づく小規模検証。

## 結論

**App Serverは第一候補を維持し、最終採用は保留。親の中断だけでは子・孫・管理実行が止まらないこと、保存キュー付きcold resumeが待機依頼を開始することを実測した。実fixture pluginのMCP能力は会話単位無効化で除去された。**

比較候補はApp Server＋限定CLI補助＋AgentDock側管理のまま。基本対話等の前回14チェックを再定義せず、今回の6測定手順を5個の試行から選択して統合した。1回の全suite合格・要件受入6項目合格ではない。原本configの前後SHA-256は選択した全試行で一致。製品実装・HTML・既存認証・実クラウド推論は変更・実施していない。

統合結果: [batch-result.json](probes/app-server/batch-result.json)。前回の[Probe Review](AgentDock_Probe_Review.md)と各runsの不成立も保持する。

## 検証環境と限定範囲

- 対象exe・Node.jsは前回と同じ。0.160.0以外は拒否。専用CODEX_HOME、専用work、loopback Responses模擬モデル、ローカルMCPのみ。
- gpt-5.4は模擬要求用識別子。子・孫も実App Server内で生成するが応答は固定fixtureで、実モデル推論ではない。
- 子・孫生成を明示した検証要求のみを発行。孫用に専用configのagents.max_depthを2へ設定。一般タスクの自律委任は行わない。
- 管理実行は、PIDを標準出力へ出し45秒で終わる検証専用Node待機処理だけ。read-only＋on-requestで、正確なfixture commandとcwdに一致する承認要求だけ単発accept。永続execpolicy追加や一般要求への承認代行はしない。
- local marketplaceにSkillとMCPを持つ実fixture pluginを作り、専用CODEX_HOMEへCLIで追加・削除。既存pluginや設定は操作しない。Hooks、Apps、OAuthはfixtureに含めない。
- WebSocketはloopback・experimental・認証なし。TCP proxyを使った強制切断と通常のclose handshakeを別測定。製品transport選定ではない。

## 1. 全接続断と保存キュー

|条件|観測|限界・採用への影響|
|---|---|---|
|TCP切断、1接続／2接続を全切断、追加待機0／500／2500ms、各2回|12行すべてloaded維持。保存turnはinProgress、runtimeはactive。resumeによるモデル要求増加なし|proxy破棄の処理に約100ms待機を含むため0msは絶対ゼロ時間断ではない。長時間断・負荷・通知欠落・親子孫復旧の保証ではない|
|通常WebSocket close、2秒上限|closeAcknowledged=false、約2013msで待機を打切り。その後loaded・active・inProgress|close要求を送ったこととTCP切断成立を同一視しない。これを全接続断耐性の成功根拠に数えない|
|通常完了の履歴へCLI保存キュー、App Server再起動後read|1件を保持、モデル要求3→3|readは閲覧のみ。ライブ購読は別|
|上記のcold resume|モデル要求3→4、turn/started、キュー1→0|再購読だけのつもりでも待機依頼開始の副作用が実在。返却thread.status=idleだけから直後もidleと断定しない|
|active＋保存キュー、TCP全切断500ms、warm resume|loadedとキュー1件を保持、モデル要求1→1|単一会話・当該active条件のみ|
|上記をinterrupt後、warm idle resume|各500ms観測ではキュー1件保持、モデル要求1→1|cold resumeとの条件差を維持。長期watcher・自然完了・子孫失敗の条件は未網羅|

前回08:20:46 UTC試行の「最後の接続断後loadedに残らない」は取り消さない。今回TCP切断条件では再現しなかった。過去の状態差の根本原因は未確定。通常closeが実際には完了していない可能性は、切断条件を明示すべき新しい根拠であり、過去不成立の原因確定ではない。

AgentDockのキューは正常完了・子孫非稼働・不明時保留・失敗中断時停止・明示再開という合意を維持。App Server内部キューをそのまま製品キューへ置換しない。監視復旧時は保存キュー有無と状態を照合し、無条件resume／再送を避ける設計が必要。

## 2. 親・子・孫と管理実行の停止

|対象|実測|採用判断|
|---|---|---|
|新規親→子→孫|source.subAgent.thread_spawn.parent_thread_idとdepth 1／2を照合。子孫それぞれのturn・item・status通知を同じクライアントで取得|spawn完了だけの監視ではなく、子孫内部通知の限定成立。役割がnullなら未取得を維持|
|親turn interrupt|親interrupted／idle、子と孫はinProgress／activeのまま|親中断を再帰停止と扱わないことを実測で補強|
|子・孫への個別interrupt|各対象の保存turn.status=interruptedを再取得して照合|生成途中・切断・競合・未知の子孫を含む全体停止保証ではない|
|yield済み管理実行でturn interrupt|turnはinterrupted。管理一覧に残り、fixture OS PIDは生存|ターン終了と管理実行停止は別。追加の管理停止が必要|
|独立した管理実行へterminate|terminated=true、管理一覧空、fixture PID不存在を300ms後に確認|当該待機処理の成立。任意のOS子孫・Job登録競合・fallbackの保証ではない|
|独立した管理実行へclean|空応答、管理一覧空、fixture PID不存在を300ms後に確認|clean応答だけで完了判定しない。ターンは別途中断して後始末|

管理IDとOS PIDは異なる。今回もAPIのosPidはnull。PIDは専用fixture出力から独立取得し、生存確認に利用した。初回管理試行では前の管理一覧項目を誤って次のterminate対象に選ぶ検証バグがあった。最終試行は操作ごとに独立threadを用い、当該管理IDとPIDの対応を分離。古い結果を全停止成功に数えない。

終了後のプロセス照合で、検証用App Server・MCP・待機fixtureの残存は見つからなかった。これは今回の後始末の確認で、製品の全OS停止保証ではない。

## 3. 実fixture pluginの能力反映

1. CLIでfixture@agentdock-probeを追加。runtimeのpluginIdとconnected、モデル要求のmcp__plugin_fixtureを確認。
2. 同じ会話のdisabledPluginIdsへ実IDを設定。次のモデル要求から当該namespaceが消え、thread指定MCP一覧から当該plugin serverも消えた。
3. disabledPluginIdsを空に戻すと、次要求へnamespaceが再出現した。
4. CLIでfixtureを削除しconfig/mcpServer/reloadを行うと、次要求とruntime一覧から再び消えた。

したがって、対象版の古い公開型注記「保存値であり能力filter未実施」を実物のMCP挙動全体へ適用しない。**このfixtureのMCPではfilterが成立したという実測を併記する。**注記が消えた、全Plugins能力が成立した、という判断には広げない。

無効化後の要求文脈には過去のdeveloperメッセージ由来のSkill名が残ったが、現行instructionsと最後のdeveloperメッセージにはなかった。これを現在のSkillが有効な証明にしない。skills/listはcwd単位の一覧であり、その結果から会話単位filterを確定しない。Skillsの明示呼出、Hooks、Apps、実OAuth、複数会話、CLI操作の部分失敗・同時変更競合は未確認。

途中のチャット報告でcapabilityStillAdvertised=falseを逆に読み、MCPが残ると述べた点は訂正済み。JSONは最初の試行からfalseで、原結果を変更していない。

## 4. 選択した証拠と不成立試行

|測定|採用したruns開始時刻（UTC、すべて2026-10-04）|
|---|---|
|実plugin無効化・再有効化・CLI削除反映|08:51:20.222|
|保存キューcold resume／TCP断12条件|08:42:30.412（同runの他の不成立チェックを合格扱いしない）|
|warm保存キュー／通常close|08:48:28.110|
|親子孫通知・個別中断後status再取得|08:49:44.364|
|管理実行interrupt／terminate／clean|08:47:29.135|

08:34:37は要求ツール型を観測する準備試行で、追加検証suiteの合格には数えない。08:37:28は無期限close待ちで停止し、所有確認した専用WSサーバーだけ外から終了した。介入後の行は切断保持の根拠に使わない。[intervention.json](probes/app-server/runs/2026-10-04T08-37-28-175Z/intervention.json)を追加し原resultは維持。

子試験の不成立はfixtureのnamespace形式、生成検出にthread/startedを期待した検証側の誤り、孫の深さ制限・要求到着競合の切り分けを含む。最終はspawn完了itemからIDを取得し、直接親と子内部通知を別確認。管理試験の初期拒否はread-only＋neverのexec policyによるもの。fixtureに一致する単発承認だけを許可して再試行し、既存設定には反映していない。

全試行をbatch-result.jsonのtrialsへ列挙。passは測定手順の完了であり、親再帰停止やキュー安全性という製品条件の成功ではない。

## 5. 担当範囲・未解決と次の検証

|条件|前進した証拠|残る条件・担当候補|
|---|---|---|
|B01 安全な復旧|TCP断の反復、read／warm／coldと保存キューの差|切断状態の確定、親子孫の再発見・再購読、通知欠落、resume前後のキュー副作用抑止。状態照合・キュー保留はAgentDock担当候補|
|B02 全対象停止|親中断後の子孫継続、個別turn停止、管理実行の個別終了|子孫生成競合、ページ増減、通信断、Windows OS子孫、保存失敗。各対象照合・10秒停止未確認・削除保留はAgentDock担当候補|
|B03 Plugins管理|実MCP filter、再有効化、ローカルCLI削除＋reload|Skills/Hooks/Apps、認証、部分失敗、同時変更、live追加、複数会話。限定CLIと能力別照合を比較。管理APIの本番利用制約は維持|

次はこの3条件をまとめ、親子孫の切断復旧と生成競合、キューの正常完了・失敗・中断境界、管理実行がOS子孫を生成した場合、実pluginのSkills/Hooksを小さく検証する。実クラウド/auth・製品実装・HTML・既存設定変更は今回の範囲に含めない。personality非推奨、履歴revertとファイル復元の差、その他18操作の未確認も維持する。

公式参考: [App Server](https://learn.chatgpt.com/docs/app-server)。対象版公開ソースの保存URLとSHA-256はbatch-result.jsonのsourceCache。ソース注記・現在の公式仕様・実測を別の証拠として扱う。

## 6. 後続更新（正本v0.45）

本書はv0.44時点の履歴。最新はAgentDock_Probe_Recovery_Review.md・正本第47節を優先。既知親子孫の復旧、内部キューの失敗後／子稼働中送信、限定OS子孫終了、実pluginの明示Skill／信頼済みHook抑止を確認。未知子孫・生成競合・長時間断／再起動・他能力は残る。batch-result.jsonは10測定を9試行から統合する。
