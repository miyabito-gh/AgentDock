# AgentDock 復旧・キュー境界・OS子孫・Skills/Hooks 検証

2026-10-04、正本v0.45の根拠。承認済み小規模検証を継続し、codex-cli 0.160.0・専用CODEX_HOME/work・localhostモデルを使用。今回4測定を4試行から選択。過去分を含むbatch-result.jsonは10測定を9試行から統合する。手順passは製品受入合格ではない。既存config hashは全選択試行で一致。本体・HTML・既存設定／認証・実クラウド推論は変更しない。

## 復旧

09:15:42.486 UTC開始の試行。親→子→孫を生成してTCP proxyを破棄し、切断中に孫を完了。500ms後に再接続、loaded/listと既知IDへのreadで3会話を取得。直接親IDとdepth 1／2が一致し、孫のcompletedを回収した。再接続後・resume前に完了通知の再配信は観測なし。通知だけに頼らず履歴照合が必要。

3会話を個別resumeしてもモデル要求15→15。個別中断後に子・親のturn/completed=interrupted通知を再接続側で確認。キューなし、サーバー生存、既知IDの限定条件。未知子孫の再発見、生成競合、親だけのresumeで子孫も購読できるか、長時間断、再起動は未確認。以前の非保持試行は未解決。

## 内部キューの境界

09:18:06.457 UTC開始の試行。独立した4会話の稼働中ターンへCLIで各1件保存し、終端後1500msを観測。

|終端／状態|キュー|モデル要求|結果|
|---|---|---|---|
|正常完了|1→0|3→4|次ターン開始|
|同じターンのfixture stream切断でfailed|1→0|5→6|失敗後にも次ターン開始|
|明示interrupt|1→1|7→7|観測内では保留|
|子がinProgressのまま親だけ正常完了|1→0|10→11|子の完了を待たず開始|

失敗はローカルstreamを切断し、同じturn.status=failedを確認した。再試行0。09:15:42の初期試行はinterrupt後に別failedターンを開始した条件だったため、この失敗境界の根拠には使わない。原結果は維持。

内部キューを合意済みAgentDockキューの代替としてそのまま採用できない。AgentDock側の保管・状態照合・送信判断を担当案とする。正常な親完了に加え、子孫の稼働／待機なしを要求し、失敗・中断・不明状態では保留。復旧はread優先。保存キュー付きcold resumeの副作用も維持。明示再開・重複防止・クラッシュ回復は未実装。

## Windows管理処理のOS子孫

09:16:58.097 UTC開始の試行。正確なcommand/cwdだけ単発承認し、Node親→Node子→Node孫の45秒上限fixtureを実行。各PIDは処理自身のstdoutから取得し、全3PID生存を確認。別会話のterminateとcleanを各1回行い、500ms後に管理一覧空・各3PID不存在を確認。観測前に追加cleanupなし。

この構成では終了が成立。detach、生成競合、別所有権、PID再利用、一覧ページ増減、通信断、任意のOS子孫、10秒以内停止の一般保証ではない。processIdとOS PIDは別、API osPid=nullも維持。停止未確認時の保存・削除保留を維持。

## Plugin Skill／Hook

09:19:58.962 UTC開始の試行。local fixture pluginを専用環境へCLI追加。skills/listの実名fixture:fixture-probeを用い、freshな別会話へtext指定＋skill入力itemを送信。本文固有tokenのモデル入力への注入を測定し、過去developer履歴の名前残存とは区別した。

|Hook信頼|会話のplugin無効化|Skill本文注入|Hook累積実行件数|
|---|---|---|---|
|未信頼|なし|あり|0|
|未信頼|あり|なし|0|
|信頼済み|なし|あり|1|
|信頼済み|あり|なし|1（増加なし）|

HookはUserPromptSubmitの同期command1件。自作fixtureを確認し、hooks/listのkey/currentHashを専用configのhooks.stateへ記録して再起動した。全Hookのtrust bypass、既存config変更、永続execpolicy追加なし。有効な会話でhook/started・hook/completed=completedとログ1件を確認。無効な会話では両通知なし・ログ増加なし。hooks/listはcwd単位なので、会話の無効化後も一覧のenabled=trueは残った。一覧を会話内実行可否と同一視しない。

09:16:58のSkill検出失敗はprefix付き実名を未考慮だった検証側の誤り。09:19:29はWindows command形式不一致でHook exit 1、開始通知だけを成功と数えない。PowerShell呼出演算子で修正した09:19:58を選択。全試行をtrialsに残した。

当該明示Skillと同期UserPromptSubmit Hookの限定成立。他のSkill解決、変更Hookの再信頼、他イベント／非同期Hook、Apps/OAuth、live追加、部分失敗・同時変更は未確認。[Hooks公式資料](https://learn.chatgpt.com/docs/hooks)、[App Server公式資料](https://learn.chatgpt.com/docs/app-server)。対象版信頼設定の根拠は[config_rules.rs](https://github.com/openai/codex/blob/rust-v0.160.0/codex-rs/hooks/src/config_rules.rs)。

## 採用判断・残項目

App Server第一候補・最終採用保留。App Server＋限定CLI補助＋AgentDock側管理を比較し、内部キューの送信判断をAgentDock側へ分ける案が具体化した。

- B01: 未知子孫の再発見、生成競合、長時間断／再起動、キューを動かさない購読・復旧。
- B02: 生成競合／通信断／ページ増減を含む停止照合、10秒未確認時の保存・削除保留。
- B03: 他Hookイベント・再信頼、Apps/OAuth、live追加、部分失敗・同時変更、能力別管理契約。
- 責任境界と残る18操作の比較、取得方式・UI技術の選定。製品実装は別段階。

合意済み18操作・D01〜D07、personality非推奨、履歴revertと安全なファイル復元の差を維持する。
