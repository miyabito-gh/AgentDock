# AgentDock idle・所有別停止・Plugins更新の追加検証

2026-10-04。承認済みの隔離検証を継続。codex-cli 0.160.0、試行別CODEX_HOME/work、localhostモデル・MCP・自作pluginのみ。本体実装・HTML変更・既存設定／認証の変更やコピー・実クラウド推論なし。測定手順のpassを製品受入合格とは扱わない。

## 保存取得と購読

根拠: [10:54:59.697 UTC試行](probes/app-server/runs/2026-10-04T10-54-59-697Z/result.json)。接続を破棄した後、同じサーバーのloaded/listとreadを取得。モデル要求2→2。read後のunsubscribeはnotSubscribed。fixtureターンを正常完了させると、保存completedは回収できたが再接続側のturn/completedは0件。

明示的に新しいturn/startを行う比較でも依頼は完了・保存されたが、turn/item通知は戻らず、その後のunsubscribeもnotSubscribedだった。thread/status/changedだけは観測した。状態通知の受信をturn/item購読の復旧と同一視しない。キューなし、短時間断、ロード済みルート1件の限定条件。

監視の無条件resumeは引き続き採用しない。read優先の履歴補完と、live監視未復旧・キュー保留の表示を担当案として維持。ユーザーの明示継続に伴うresumeを、監視専用の再購読と分ける。対象版ThreadResumeParamsにexcludeTurns等はあるが、それを副作用抑止指定とは扱わない。

read-onlyポーリングの代替も [11:09:15.132 UTC試行](probes/app-server/runs/2026-10-04T11-09-15-132Z/result.json) で比較。最初の部分本文chunkは元の購読接続でitem/agentMessage/deltaとして受信し、fixtureのdeltaが実際に処理されたことを確認した。一方、別の読取り接続のreadにはactive/inProgressとユーザー入力があるが部分本文はなく、全元購読を切断した後の次chunkもreadでは取得できなかった。最終completed後は全文を回収。モデル要求2→2、read応答4／4ms。この低負荷1会話の応答時間を2秒・100agentの保証にしない。readポーリングを本文逐次表示の完全代替として採用しない。初回11:01:17.173のoracleなし試行と11:02:06.857の同等測定も保持。選択試行では版確認の起動にも専用HOME/workを先に適用した。

31分idleは [10:43:12.306 UTC試行](probes/app-server/runs/2026-10-04T10-43-12-306Z/idle-recovery.json) で確定。完了済み・キュー空の会話から全WS接続を破棄し、専用サーバーを生かしたまま31分間RPCもresumeも行わず実時間待機。最終観測elapsedMs=1861577、loaded/listは空、readはnotLoadedで完了本文を回収、保存キューは空。毎分のモデル要求は1のまま、読取り前後も1→1、read側通知なし。fixtureの明示resumeはidleで成立し1500ms後も要求1。ただし30分ちょうどのunload時刻は未観測、queue付き・activeターン・実OS sleep・安全な購読専用契約の保証ではない。

このまとまりは新規8測定／8選択試行、累計26測定／24選択試行として統合。初回・不成立・証拠不足の試行も保持し、全suiteや製品受入合格とは扱わない。

## 停止対象の所有と実終了

選択根拠: [10:49:50.044 UTC試行](probes/app-server/runs/2026-10-04T10-49-50-044Z/result.json)。同じcwdの別会話A/Bに、45秒上限のNode親子孫を各3PID生成。さらに同じcwdへharness管理の独立Nodeを起動し、App Serverの管理実行と区別した。

|操作|応答|OS照合|
|---|---|---|
|BのthreadIdとAのprocessIdを組み合わせterminate|result.terminated=false|A/Bの6PIDと独立PIDは生存|
|Bをclean|result={}|Bの3PID不存在、Aの3PIDと独立PIDは生存|
|Aの正しい組合せでterminate|result.terminated=true|Aの3PIDも不存在、独立PIDは生存|

API管理IDとOS PIDは別で、osPid=null。CIMにより実行ファイル・生成時刻・コマンドライン・直接親PIDを限定fixtureだけ記録。停止要求後500ms待って生存とCIM実体を照合し、追加cleanup前の結果を保存した。同一のコマンド／cwdだけでは所属会話を特定できない。

初回10:47:40.634 UTCはCIMがサンドボックスで拒否された。初版harnessは空stdoutを空一覧として扱い、OS識別情報の取得をassertしていなかった。元のpass記録は改変せずevidence-assessment.jsonで所有証拠への不採用を記録。拒否後に必要な昇格許可を得て再実行し、CIM失敗を明示エラーとして記録・assertするよう修正した。

この測定は任意PIDの所有判定、PID再利用、detach、権限の異なるプロセス、遅延生成を保証しない。製品案は起動単位・サーバー・threadId・管理processIdを紐づけ、取得可能ならPIDと生成時刻／実行ファイルも照合すること。cwd・親PID・保存履歴だけを所有証拠にしない。所有不明と実終了未確認を別に保持し、D04/D06と削除保留を維持する。

## Pluginsの設定競合

根拠: [10:45:08.956 UTC試行の同時CLI測定](probes/app-server/runs/2026-10-04T10-45-08-956Z/result.json)、[10:53:30.021 UTC試行](probes/app-server/runs/2026-10-04T10-53-30-021Z/result.json)。

- 異なる2pluginのCLI add／removeを同じconfigへ同時起動。各5組、計20コマンド。起動～終了時間は重なり、全exit 0、add後は両設定／cacheが存在し、remove後は両方が消えた。内部のread/writeが同時になるよう強制した測定ではなく、更新消失を観測しなかったことを排他保証にしない。
- CLI add後に、変更前のexpectedVersionでconfig/value/writeを行うとcode=-32600、config_write_error_code=configVersionConflictで拒否された。検証専用のtui.show_tooltipsだけを変更。
- CLI addと版指定付きAPI表示設定書込みの5重複試行は、両方成功しplugin設定と表示設定が残った。読み出した設定・永続ファイル・応答を別記録。CLIにもexpectedVersionが適用される証拠ではない。

対象タグのplugin_edit.rsでは専用configをread→編集→write_atomicallyする経路を確認。この関数に版比較はなく、読取りから書込み全体の共通排他も確認できない。API側の版競合検出をCLI／外部エディタを含む一つのtransactionへ拡張しない。アプリ内直列化、操作前後の設定再取得、意図した差分と無関係な変更の照合、不一致時の保留を比較案へ追加。旧config全文への自動rollbackを前提にしない。

## marketplace版更新・削除と既存会話

根拠: [10:46:33.494 UTCローカル版更新／削除](probes/app-server/runs/2026-10-04T10-46-33-494Z/result.json)、[10:49:01.537 UTC Git marketplace更新](probes/app-server/runs/2026-10-04T10-49-01-537Z/result.json)。

Git fixtureは試行領域内のsource／bare repositoryとlocalhostのdumb HTTPのみ。v1→v2のfixture commitを作り、CLI marketplace upgradeによる実fetchを測定した。upgradeはexit 0、errors=[]、installed一覧は0.2.0。Git管理のない本体ワークスペースをGit化していない。公開クラウドmarketplace・OAuth・認証情報のコピーなし。

|条件|明示Skillの新しい本文|モデル向けMCP|Hook|
|---|---|---|---|
|Git upgrade後の既存会話|v2が新規注入|reload前はv1、reload＋1500ms後v2|Git測定では対象外|
|Git upgrade後の新規会話|v2が注入|v2|同上|
|local sourceをv2へ変更し明示re-add|既存／新規ともv2が新規注入|既存はreload前v1、後v2。新規はv2|変更定義は稼働中にも新hash／modifiedへ再評価、実行抑止|
|CLI削除直後|既存会話にも追加注入なし|既存はreload前までv2、新規はなし|一覧なし、実行・ログ増加なし|
|削除＋reload／再起動|追加注入なし|既存／新規とも広告なし|実行・ログ増加なし|

local marketplaceへのupgradeはexit 1でGit marketplaceではない旨を返す。localのre-addとGit upgradeを別経路として記録した。前回の「cache内Hook定義を直接編集すると稼働中に再評価されなかった」証拠も維持する。今回の明示re-add経路の結果で、どんなファイル変更も即時反映すると一般化しない。

既存会話では過去に注入されたv1/v2本文が削除・reload・再起動後もモデル入力履歴に残った。モデル要求全体のtoken有無だけでは現在のSkill適用を証明できない。今回harnessはinput位置・内容のopaque hash・token出現数を記録し、既存会話のv2出現数は更新前0→更新後1→reload後2→削除後2のままで、新しい注入が止まることを照合した。既存の本文を消去する操作は行わない。

初回10:45:08.956 UTCの版更新／削除測定も保持。ただし本文全体のboolだけでは履歴残存との区別が不足するため、補強した10:46:33.494を選択。削除で過去本文や既に行った作用が消えると説明しない。MCPツールの実呼出、進行中ツール取消、暗黙Skill、非同期Hook、Apps/OAuthは未確認。

## Windows sleepの残条件

今回の31分idleはWindows上の実時間待機であり、Windows自体をsleepさせる測定ではない。sleepはPC全体と他の作業へ影響するため、承認済みの隔離プロセス内だけの検証へ混ぜない。プロセス停止や時計の人工変更を実sleepの代替合格にもしない。

読取り専用のpowercfg /aは実行し、このPCではS3・休止・高速スタートアップが利用可能、S0 Low Power Idleはfirmware非対応、hybrid sleepはhypervisor非対応と表示された。[sleep-readiness.json](probes/app-server/sleep-readiness.json)に記録。現PCの実測候補はS3／休止とし、Modern Standbyは別構成の残条件。これは実sleepや復旧の成功証拠ではなく、電源設定は変更していない。

後続の実sleep測定は、作業のない時間帯に本人がsleep／wakeを行い、専用サーバーPID／起動時刻、power通知、壁時計・単調時計・観測間隔、接続応答、loaded/read/保存キュー、wake前後のモデル要求と各turnを照合する。sleep中のリアルタイム監視を要求せず、wake後は鮮度を不明にして送信保留→履歴と対象集合再照合→購読可否の別判定を行う案。自動resume／自動再送／自動OS強制終了は採用しない。

実sleep・Modern Standby／休止の差、30分超sleep、ネットワーク・MCPの再起動、保存途中sleep、OS時計補正は未確認として残す。sleep検証だけを根拠にUI技術や取得方式の採用を確定しない。

ホストでsleep/wakeを観測する公開候補として、Microsoftの [RegisterSuspendResumeNotification](https://learn.microsoft.com/en-us/windows/win32/api/winuser/nf-winuser-registersuspendresumenotification) はwindow handle／callback登録を提供する。UIタイマーの遅延だけをOS sleep証拠にしない。対象タグのunload判定はhas-subscriber／activeの両状態とInstantを使い、[Rust Instant文書](https://doc.rust-lang.org/std/time/struct.Instant.html) はsuspend中の経過を数えるかがOS・版依存であることを説明する。壁時計と期限を単純に同一視せず、実Windows条件で検証する。native通知経路も未実装・未実測。

## 責任境界・18操作・技術選定への反映

App Server＋限定CLI補助＋AgentDock管理を比較の中心に維持。保存取得・turn/item購読・明示継続を分離、停止の対象所有／API結果／ターン終端／OS実体／保存を分離する。Pluginsは操作結果・設定版・cache版・会話別能力・Hook信頼・過去本文残存を別状態として記録する案。

18操作の11（Skill適用）、12（MCP/Plugins管理）、18（同時利用）には能力の会話別反映と設定競合を追加。2（安全な変更復元）、14（worktree削除）等にも所有不明時の停止を引き続き適用する。1〜18の希望操作とD01〜D07の合意を変更・再質問しない。

stdio優先評価・Tauri 2第一UI検証候補を維持。画面hide／renderer再読込みで接続を切らないホスト構成を比較するが、stdioでホストごと失われた場合の復旧とWSで接続だけ失われた場合の復旧は別。Tauri／Electron／WPFのどれを選んでもreadをlive購読へ変えることはできない。Windows wake後のホスト状態照合を各候補の受入条件へ追加する設計案。

App Serverの最終採用・UI最終選定・製品実装は未決。B01安全な購読、B02任意構成／生成競合、B03その他能力・更新途中失敗、認証・クラウド・18操作の残る機能成立・V01〜V10の不足は解消していない。

対象版固定ソースは [source-followup/manifest.json](probes/app-server/source-followup/manifest.json) にref・blob SHA・取得ファイルを記録。現在文書の [App Server](https://learn.chatgpt.com/docs/app-server) はreadと購読の区別、無購読・無活動30分のgraceを説明する。[Developer commands](https://learn.chatgpt.com/docs/developer-commands) のmarketplace upgradeはGit marketplaceの更新。現在文書と対象版実測・固定ソースは区別する。

