# AgentDock 復旧・停止の競合検証

2026-10-04。codex-cli 0.160.0、試行別専用CODEX_HOME/work、localhostモデル/MCP。承認済み隔離検証のみ。既存設定・認証の変更／コピー、クラウド推論、本体実装・HTML変更なし。各passは測定手順成立で、望ましい製品挙動や全suite合格ではない。

## ページ取得中の生成

根拠: [10:15:30.823 UTC result](probes/app-server/runs/2026-10-04T10-15-30-823Z/result.json)。親から子2件を生成。祖先filter・sourceKinds=subAgentThreadSpawn・limit=1で1ページ取得後、既存子に新ターンを送り孫を生成し、その後古いcursorから残りページを取得。

- 最初の走査は既存子2件のみ。途中生成した孫1件を取り逃がした。
- cursorを捨て先頭から再走査すると3件すべて取得。readで孫の直接親を照合。探索中のモデル要求9→9。

再走査には孫IDを検索入力として渡さない。fixtureで知っている3IDは結果検証用oracleとしてのみ使用。一度のnextCursor=nullを完全な子孫集合の証明にしない。1回の新規生成・保存済み条件の成立であり、大量／継続生成・archive・保存遅延等の全保証ではない。[公式App Server資料](https://learn.chatgpt.com/docs/app-server)のページ取得経路と今回の測定を分離する。

## 未知子の中断timeoutと自然完了競合

選択根拠: [10:17:08.956 UTC result](probes/app-server/runs/2026-10-04T10-17-08-956Z/result.json)。子生成のtool応答を750ms遅延し、生成前に唯一の当該WS接続を破棄。fixture側で子要求到達を待ち、新接続からルートIDの祖先検索・readで未知子を取得。子のturnIdへ中断。

|条件|件数|RPC応答|保存turn|新接続の完了通知|追加モデル要求|
|---|---|---|---|---|---|
|readのみ、resumeなし|4|全件3秒timeout、実測3013〜3026ms|全件interrupted|全件0|なし|
|read後に当該子だけresume|2|result={}、43／48ms|全件interrupted|各1|なし|

この限定条件で前回の応答timeoutを再現した。停止失敗とは限らず、対象threadId/turnIdの保存終端を応答とは別に照合する必要がある。個別resumeはキューなしfixture比較であり、監視の無条件resumeを採用しない。前回の「生成を既存接続で観測してから断した子」ではresumeなしでも応答成功したため、既知／未知という表示ラベルだけで決めず、生成時の接続・購読条件を記録する。内部原因は未確定。

自然完了との比較は3条件。先にfixtureを正常完了させreadでcompletedを確認した場合、中断応答はcode=-32600・no active turn to interrupt、保存状態はcompletedのまま。中断とfixture完了を同じtickで開始した場合と、中断先行条件（JSONのtiming=after）はresult={}・interrupted。今回「成功応答かつcompleted」の競合は未観測であり、対象版ソースにある可能性を否定しない。中断エラーを会話全体failedへ変換しない。

生成競合は、親が作業中に既存子へ孫生成turnを開始し、750msの生成遅延中に親をinterruptした1条件。停止前祖先一覧1件、停止後再走査2件。親のturnはinterrupted、子・孫はactive/inProgress。親の停止成功や停止前一覧だけで全体停止を確定できない。

初回10:15:45.226 UTCも4件全て3秒timeoutとinterruptedを観測。後続のresume比較2件を追加した選択試行と区別し原結果を保持。合計8回の未購読条件を2試行で観測したが独立した環境・任意構成の頻度保証ではない。

## 120秒断の測定条件

初回[10:15:09.848 UTC result](probes/app-server/runs/2026-10-04T10-15-09-848Z/result.json)は不成立。fixtureのholdはSSEコメントを200ms毎に送るが、Codexのstream idle timeout=60000msによりactive turnがfailedとなった。内部保存キューが開始され、2会話のキューはいずれも空、モデル要求2→4。60秒制限はTCP切断時間やidleアンロードとは別である。

初回のassert文「Read after gap caused inference」は原因を正しく特定していない。切断前とread後のカウンタだけを比較しており、切断中の自律dispatchとread起因を区別できない。原コードsnapshotとfail記録は維持。後続測定では専用fixtureのstream idle timeoutを300000msに設定し、再接続後・読取り直前と直後の要求数を別に記録。既存ユーザー設定へは反映しない。

120秒でも[公式資料](https://learn.chatgpt.com/docs/app-server)が説明する30分無購読・無活動のアンロード境界を越えず、その受入証拠にはしない。Windows sleep・通信復帰・任意クラッシュ・保存失敗も別条件。

選択根拠: [10:18:22.565 UTC result](probes/app-server/runs/2026-10-04T10-18-22-565Z/result.json)。実断から再接続・読取りまで120131ms、stream idle timeout=300000ms。切断前の実行中会話とidle会話へ、専用queue CLIで各1件を保存した。

|対象|切断直前|復帰後read／queue|個別resume後1500ms|
|---|---|---|---|
|実行中会話|active/inProgress、キュー1|active/inProgress、キュー1|同じturn、キュー1、要求増加なし|
|idle会話|idle、既存completed、キュー1|active、追加inProgress、キュー0|開始済みturn継続、キュー0、要求増加なし|

モデル要求は切断前2→読取り直前3→読取り直後3。3件目は切断中のidle会話の保存キュー。fixtureモデルの到達時刻を記録し、read/resumeによる開始とは区別した。resume段階では両会話ともactiveであり、idle状態のwarm resume成立を追加主張しない。今回はサーバー自体は生存し、各会話もloadedに残った。120秒断全般の保証ではない。

初回混在試行は、idle timeoutと失敗後dispatchの診断証拠として保持し、安定したactive復旧の成功根拠には使わない。タイムアウト延長でCodexの任意長時間実行や製品設定を変更したことにしない。

今回のaggregateは3測定を3試行から追加、**15測定／14選択試行**。不成立・選択しなかった試行はtrialsへ保持。全suite・製品受入合格ではない。今回選択3試行とも既存config前後hash一致、検証スクリプトのnode --check成立。

## 採用判断への反映

設計候補は、再接続・停止中に先頭から子孫集合を再照合し、生成イベントとcursor結果を統合すること。集合の一致が続いても今後生成しない保証とはせず、不明な対象・終端未照合・管理実行未確認・保存失敗があればキューと削除を保留する。

中断要求はthreadId/turnId単位で保持し、timeout時にreadで同じturnを再確認する。interruptedが得られてもOS全停止・保存全体完了まで合成しない。終端を確認済みのturnへ中断を無条件再送しない。D04の10秒案内・D06の保存区分を維持。製品実装は未着手。

次は30分idle境界の扱いと安全な購読契約、停止対象の管理実行・所有照合、Plugins live追加・Hook変更再信頼・部分失敗。UI技術候補と18操作を変更せず、App Server第一候補・最終採用保留。
