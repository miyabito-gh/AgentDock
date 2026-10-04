# AgentDock 責任境界・18操作・取得方式・UI技術の比較

v0.47追加: [復旧・停止競合の実測](AgentDock_Probe_Concurrency_Review.md)でページ途中生成の欠落と再走査回収、無接続時生成の子の中断応答timeoutと保存interrupted、親停止中の子孫継続、120秒断とidle内部キューの自律開始を確認。以下の責任境界・技術候補は維持。正本第51節を現在の最新証拠として適用し、過去の未確認を全面成立へ広げない。

2026-10-04。要件定義・採用検証の継続資料。第32〜36節、18操作、D01〜D07は維持し再選択しない。方式・UI技術の最終採用と製品実装は未決定。証拠は正本第47〜48節と各試行JSONを参照。

## 1. 候補構成と責任境界

比較の中心は **ローカルApp Server＋限定CLI補助＋AgentDock側管理**。App Server単独では合意済みキューや安全なファイル復元を満たしたと扱えない。

|責任|App Server|限定CLI／OS・Git経路|AgentDock側の設計候補|
|---|---|---|---|
|会話・要求応答|thread/turn、承認・質問、逐次イベント|通常CLI画面を必須にしない|thread/turn/request ID対応、取消・期限切れ・受理不明の表示|
|本文・活動保存|保存会話を正本候補、read/list|内部DBを直接編集しない|表示キャッシュ、取得済み活動、未保存差分、下書き・添付・所有情報を保持|
|監視復旧|loaded、祖先検索、read。検索と購読を分離|Hooks・JSONLは不足時の補助候補|既知IDと新規発見IDを照合、鮮度を別管理、未復旧時キュー保留|
|次依頼のキュー|turn/startへ明示送信。内部保存キューの自動dispatchには委ねない案|常時queue CLI投入を主経路にしない|永続保管、親子孫の通常完了確認、送信判断、受理不明の照合、明示再開|
|中断・全体終了|各turn/interrupt、管理実行list/terminate/clean|所有を確認できる実行のOS停止照合。強制終了は合意済み別操作|停止要求対象集合、再発見、終端状態・保存照合、10秒未確認案内、削除保留|
|Plugins|MCP状態・OAuth・reload、Skills/Hooks等の公開取得経路|plugin CLI管理は明示操作時の候補。既存設定への実操作は今回対象外|installed/enabled/trusted/authenticated/connected/callable/反映済みを分離、部分成功と競合を表示|
|ファイル・worktree|変更イベントは補助証拠|GitとOSの実体確認・所有に基づく操作候補|変更前状態と他会話変更を照合、添付コピー、成果物実在、退避・削除対象管理|
|Windows/UI|画面寿命に依存しない接続をホスト側へ置く案|トレイ・通知・起動・窓のネイティブ経路|通知2秒集約、フォーカス抑制、通常窓と監視窓、容量・復元・入力設定|

キューの送信記録には待機ID、送信試行、thread/turn対応、設定確認時点を持つ案。通信断中の「送信したが応答不明」を未送信へ戻さず、履歴との一致が曖昧なら保留する。本文hashだけでは同じ文章の別依頼を区別できない。正式な冪等性契約は未確認であり、exactly-onceを保証しない。

専用CODEX_HOMEは検証の隔離手段。製品の認証・履歴配置をこれで決定していない。既存履歴を使う要件と認証方針を維持し、認証のコピーや共有config変更を今回行わない。

## 2. 復旧・停止・Pluginsの残条件

|区分|今回までの前進|次の検証・設計条件|
|---|---|---|
|B01 未知子孫|生成結果未受信でも祖先検索＋readで直接親を照合する限定測定|一度の一覧は完全snapshotではない。走査中生成・ページ増減・archive・保存遅延・深さ違い、繰返し照合と取得不能時の保留|
|B01 長時間／再起動|15秒程度のTCP断と専用サーバー再起動を限定測定|30分等の無購読idle境界、Windows sleep、UIのみの再起動、サーバー異常終了、保存中断、受理不明・キュー付き。今回15秒を長時間保証としない|
|B01 live復旧|既知ID個別resume、キューなしで要求増加なしの実測|監視だけのresumeはM11上無条件採用しない。read優先でlive未復旧を表示。副作用を抑える契約とloaded判定競合が残る|
|B02 停止照合|親子孫個別中断、限定OS3PID終了、再接続後の履歴照合|新規送信を止め対象を再列挙、各turn終端・管理実行・OS所有・保存を別照合。対象集合が同じでも将来生成不能の証明にはならない|
|B02 取得不能|中断応答timeoutの試行を保持|通知がない／APIが失敗した時はreadで再確認。通信不能、PID所有不明、保存失敗は停止未確認のまま。完了・interrupted・failedを相互変換しない|
|B03 能力filter|fixture MCP、明示Skill、信頼済み同期UserPromptSubmit Hook|暗黙Skill解決、他イベント／非同期Hook、変更hashの再信頼、Apps/OAuthは能力別に判定|
|B03 管理反映|専用pluginの追加・削除と限定reload実測|起動中追加、既存会話とfresh会話、複数接続、store成功/config失敗、同時編集、失敗後再照合。rollback未保証|

停止の設計候補は「送信保留→親への中断→対象再発見→各子孫・管理実行の停止→履歴・実体・保存再照合」。子先行と親先行の競合は未比較であり、固定順の採用ではない。終了中も生成通知と発見を継続する案。履歴中断だけでOS全停止・保存完了を合成しない。

Plugins管理では、CLI成功と能力反映を別の結果にする。失敗後は現在のstore/config/runtimeを再取得して残処理を案内し、成功済みinstall/removeを無条件再実行しない。アプリ内の直列化だけでは外部CLIとの競合を防げず、atomic writeだけでも更新消失を保証しない。

## 3. 合意済み18操作の担当比較

全行は実現候補。入口の存在・fixture成立・製品受入は別に記録する。操作の除外や延期は決めていない。

|番号・操作|主経路の候補|AgentDock担当と残る成立条件|
|---|---|---|
|1 差分画面|差分イベント＋Git／ファイル取得|実在・基準・会話との対応を確認して表示。イベントの差分だけを現在ファイルと断定しない|
|2 変更を戻す|アプリの変更所有・基準保存＋ファイル別復元|元からの変更と他会話変更を識別不能なら停止。thread/revertはファイル復元の代替にならない|
|3 レビュー|review/start|未コミット／基準ブランチの指定、受理・終了・保存を検証|
|4 レビュー保存先|review/startの同一／別thread候補|初期は現在。別チャットの対応と結果保持、forkとの違いを確認|
|5 Plan|collaborationMode等|計画／実行設定と移行、計画中の制約を確認。権限モードを勝手に変更しない|
|6 会話分岐|thread/fork|分岐元、設定・添付・保存の引継ぎ。spawn親子と別に保持|
|7 圧縮|thread/compact/start|受付と完了、圧縮前保存本文の閲覧、失敗時の文脈を確認|
|8 過去会話参考|read＋明示的な参照入力候補|送信前対象、範囲・長さ・欠損・更新時点を示す。専用参照入力契約と同等性は未確認。自動全文添付を採用しない|
|9 Goal|goal set/get/clear|自動継続、解除・停止・予算・保存の境界。単なる目標欄で機能成立にしない|
|10 side相談|一時fork＋相談境界＋専用表示|主作業維持、保持、Goal・権限継承、明示移送。read-only指示を強制sandboxとして扱わない|
|11 Skills/AGENTS.md|skills/list・skill入力＋ファイル確認|明示Skillの限定実測あり。階層適用・既存AGENTS保護・initのpreviewと書込み結果は残る|
|12 MCP/Plugins|MCP公開経路＋限定plugin CLI|B03を能力別評価。管理APIの本番制約を維持。OAuth・Appsは今回未実行|
|13 クラウド委任|experimental cloud CLI補助候補|環境、資格、進捗、停止、結果反映競合。実クラウド測定は今回範囲外、ローカルfixtureで成立にしない|
|14 worktree|AgentDockによるGit管理候補|作成・既存選択・所有・未コミット/ignored退避・確認付き削除。内部managerは公開API保証ではない|
|15 status等|モデル情報・状態・Fast/memory関連API|資格・実効値・保存scope。personality非推奨を維持、指示調整への置換は同等性未合意|
|16 操作導線|主要ボタン＋メニュー|公開経路未成立も台帳に残す。未対応の操作を成功表示しない|
|17 狭い画面|会話優先＋左右一時表示|D07維持。幅境界・フォーカス・読み上げ・IMEを実UIで検証|
|18 同時利用|ホストの複数thread管理＋表示差分|普段1〜5、監視上限とは別。暫定20ルート/100agent、2秒更新を負荷評価。未測定|

## 4. 取得方式と通信方式の比較

|方式|合意済み双方向操作への適合|復旧・影響|現時点の位置付け|
|---|---|---|---|
|App Server stdio|会話・要求応答・履歴・イベントをまとめる候補|UIの非表示とプロセス寿命を分離する。ホスト再起動時に旧プロセスへの再接続を当然とはしない|製品主経路の優先評価。App Server自体のサポート条件と機能充足は残る|
|App Server WS|同じ操作群、接続断の比較が可能|既存試験はlocalhost・無認証experimental。製品では接続認証・所有・再接続を別評価|隔離検証の道具。今回の成功で製品transportへ採用しない|
|JSONL追尾|会話入力・承認は別経路が必要|保存無効、形式変化、遅延、部分行、欠損。監視だけでruntimeを保証できない|履歴回復の補助比較。全履歴追尾を主方式にしない|
|Hooks|操作は別経路が必要|信頼設定追加、順序・イベント網羅・背景取消等の条件|不足時の補助。製品の常時追加を決定しない|
|exec --json／端末埋込み|非対話または端末の操作・表示へ依存|通常チャットと承認・追加指示・子内部活動の対応が未確認|代替比較として保持。UI要件を縮小しない|
|内部DB／OSプロセス|会話操作はできない|schema、ロック、所有、PID再利用。生存だけでrunning/doneを決められない|診断・停止照合の補助。直接DB編集を採用しない|

通常はイベント、起動・再接続・欠落時は対象メタデータと保存履歴、本文は選択時に取得する案。未知子孫検索は既定cli/vscode一覧に依存せず、必要なsourceKindsと祖先filterを明示。filterはexperimental。loaded一覧・保存検索・readを組み合わせても購読完了にはならない。

今回のthread/listは既定のscan/repairを使うため「完全にディスク書込みがない読取り」とは主張しない。公式資料のuseStateDbOnlyはscan/repairを抑える候補だが、その条件で欠損回復と未知子孫取得が成立するかは追加確認。

## 5. UI技術の比較と暫定推奨

公式資料を2026-10-04に取得。以下は設計評価であり、本PCで各UIを動かした性能比較ではない。

|候補|要件への対応根拠・構成案|主な残条件|評価|
|---|---|---|---|
|Tauri 2＋Web UI＋Rustホスト|tray、複数窓、alwaysOnTop、自動起動、通知の公式経路。ホスト側にstdio・保存・キューを置く案|Windows WebView2、通知から会話復帰、インストール後の通知、隠れた窓でも収集継続、IME/DPI、RustとUI間の型境界|**第一のUI検証候補として推奨**。配置フォルダの名前ではなく必須窓機能とホスト分離からの設計判断。採用未確定|
|Electron＋Web UI＋main/worker|BrowserWindow、Tray、最前面等の公式経路。Node検証コードの知見を移しやすいという設計推論|rendererのNode無効化・contextIsolation等、IPC検証、配布と更新、常駐資源を実測|比較第二候補。Tauriより必ず重い／遅いとは未測定のまま断定しない|
|WPF＋WebView2またはnative UI＋.NETホスト|Microsoft公式にWPF/WebView2統合経路あり。Windows専用ホスト案|トレイ・通知起動・複数窓の全導線照合、Web/ネイティブ境界、IME、runtime配布、UI移植工数|Windows運用を優先した比較候補。今回の資料だけで全要件対応保証にしない|
|ブラウザ単独|会話・Markdown表示に適合する候補|OSトレイ・最前面・自動起動・常駐管理には別ホストが必要|単独では確定Windows要件を満たさない。別ホスト付きなら別構成として比較|

Tauriの通知資料はWindowsではインストール済みアプリでの利用、開発時のPowerShell名・アイコンを記載している。開発表示だけを受入証拠にしない。通知のクリック／会話IDの復帰と音の制御は追加契約照合が必要。[通知資料](https://v2.tauri.app/plugin/notification/)

共通の設計条件として、取得・保存・キュー判断・2秒通知集約はUIのタイマーや窓の表示寿命へ置かずホスト側に置く案。Markdown・コード・取得本文はデータとして表示し、本文由来の任意コマンドをホストIPCへ直結させない。Tauri capabilityやElectron renderer sandboxはCodexの作業権限とは別の境界。[Tauri Capabilities](https://v2.tauri.app/security/capabilities/)、[Electron Security](https://www.electronjs.org/docs/latest/tutorial/security)

最終選定前の共通比較は、通常窓hide/監視窓close時の継続、renderer再読込み時の下書き・接続維持、通知から正しい会話へ復帰、最前面の非フォーカス動作、複数画面とDPI、1〜5チャットと100agent負荷。UIプロジェクト作成・配布物作成は今回行っていない。

## 6. 根拠と次のまとまり

既存の対象版証拠: AgentDock_Adoption_Batch_Review.md、AgentDock_Probe_Recovery_Review.md、probes/app-server/batch-result.json。最新の隔離測定はAgentDock_Idle_Stop_Plugin_Followup_Review.md・正本第51節。現在公開文書と0.160.0実測を混同しない。

- [App Server](https://learn.chatgpt.com/docs/app-server): readと購読の差、sourceKinds、祖先filter、listのscan/repair条件を再確認。
- [Tauri configuration](https://v2.tauri.app/reference/config/)、[System Tray](https://v2.tauri.app/learn/system-tray/)、[Autostart](https://v2.tauri.app/plugin/autostart/): 窓・トレイ・起動の文書根拠。
- [Electron BrowserWindow](https://www.electronjs.org/docs/latest/api/browser-window)、[Tray](https://www.electronjs.org/docs/latest/tutorial/tray): 窓・最前面・トレイの文書根拠。
- [Microsoft WPF/WebView2](https://learn.microsoft.com/en-us/microsoft-edge/webview2/get-started/wpf): Web UI統合の文書根拠。

次のまとまりは、B01の走査中生成・idle長時間断と購読副作用、B02の応答timeout・自然完了競合と停止対象再発見、B03のlive追加・Hook再信頼・部分失敗。クラウド/authは承認済みローカルfixtureで保証できない未確認として残す。18操作・V01〜V10の受入不足を維持し、最終採用と製品実装へ進んだことにはしない。

## 7. v0.48で具体化したPlugins責任境界

AgentDock_Probe_Plugin_Lifecycle_Review.md、第50節を追加。CLI操作完了、設定の有効化、cache配置、会話に広告される能力、Hook信頼は異なる状態。起動中追加の一覧掲載と既存会話の能力反映には差があり、CLI失敗時に設定とcacheが不一致となった。AgentDock側が各状態を保持して照合し、反映完了・削除完了を一つの終了コードで確定しない案を18操作比較に適用する。定義ハッシュは呼出し先スクリプト内容の完全性保証ではなく、自動再信頼を前提としない。

18操作・D01〜D07、stdio優先評価、Tauri 2第一UI検証候補は維持。最終選定・本体実装・HTML変更なし。次のまとまりは安全な購読とidle/sleep、停止対象の所有照合、Plugins同時変更・marketplace更新・削除の既存会話反映。この第50節時点の集約は18測定／16選択試行。

## 8. idle・所有・Plugins更新の追加比較

詳細は [追加検証レビュー](AgentDock_Idle_Stop_Plugin_Followup_Review.md)。31分の無購読・完了済み・キュー空の測定ではloaded空／read notLoadedとなり、保存履歴を回収してモデル要求増加なし。累計26測定／24選択試行。以下は観測に基づく設計比較であり、製品実装・最終採用ではない。第1〜7節の古い残条件は当時の記録とし、この第8節と正本第51節を優先する。

|責任／18操作との対応|候補の具体化|残る条件|
|---|---|---|
|監視復旧・18 同時利用|readはモデル要求を増やさず履歴と状態を補完できたが、turn/item購読は戻らず、partial本文も取得できない条件を実測。イベント収集／履歴補完／明示継続を分ける|安全な購読専用経路。readの常時ポーリングで全逐次表示を置換しない。100agent・2秒条件は未測定|
|中断・終了、2 復元／14 worktree等の所有条件|別会話の管理IDはterminated=false、正しい管理対象のOS3PIDは消失。独立同cwd実行は残る。起動単位・サーバー・thread・管理IDとPID生成時刻を対応付ける|任意のOS子孫・PID再利用・detach・遅延生成。管理IDやcwdだけで所有と終了を合成しない|
|11 Skill適用、12 Plugins更新／削除|Git marketplace v2への更新、明示Skill新規注入、MCP reload前後、変更Hook抑止、削除後の過去本文残存を能力別に照合|暗黙Skill、進行中ツール、非同期Hook、Apps/OAuth。削除後に過去の指示や副作用が消えるとは扱わない|
|12 設定管理、18 並行利用|版指定APIはCLI変更後のstale書込みを拒否した。CLI同時操作とCLI/API重複は今回変更が残った。CLI実行前後の設定・cache・会話適用を別照合|API expectedVersionをCLI全体のtransaction保証にしない。アプリ内直列化だけで外部操作競合を解消しない|
|16 操作導線、17 狭い画面|履歴取得済み／live未復旧／要照合／過去本文残存を識別できる導線をUI受入条件へ加える案|画面案・HTMLは変更なし。取得不能を成功表示しないという既存要件の具体化|
|取得方式・UIホスト|stdio優先、Tauri 2第一検証候補を維持。renderer・画面寿命と収集接続を分離し、ホスト側にwake検出と状態再照合を置く比較案|stdioホスト喪失とWS接続のみの断は別。どのUIでも購読不足は残る。native sleep通知・実wake・DPI/IME/通知復帰は未実測|

その他の18操作は第3節の主経路・残条件を維持する。クラウド認証、side、Goal、Fast/memories/personality、安全なファイル復元等は今回測定で成立していない。Windowsの実sleepはPC全体への操作として隔離測定と分け、専用fixtureと本人のsleep/wakeで検証する残条件を具体化した。

停止要求と保存・削除保留、AgentDock側キューの永続保管・状態照合・送信判断、D01〜D07は維持。App Server第一候補・最終採用保留。取得方式・UI最終選定も未決。

