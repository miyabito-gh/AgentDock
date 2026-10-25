# V0 native preview probe（製品未接続）

2026-10-25。状態: **実験コードは実装・ビルド済み。実行試験は厳格な通信遮断に不合格。製品へ組み込まない。**

`src-tauri/examples/visual_preview_probe.rs`はWindows WebView2の限定技術検証。通常のAgentDockのHost、独自command、plugin、保存処理を起動しない。通常の既定featureを変えず、`visual-preview-probe`だけで有効化する。

## 保存済み部分と追加修正

- `fc15894`: 初期probe。独立した非表示1000×700窓とchild WebView、sandbox iframe、native資源/遷移拒否、loopback fixture/canaryを実装。
- 実起動でCargo exampleにCommon-Controls v6 manifestがなく、`TaskDialogIndirect`のエントリポイントエラーを検出した。これはmainより前の失敗でRustのwatchdogも動かない。エラーダイアログを閉じ、終了コード-1073741511を確認した。
- `build.rs`は通常の`tauri_build::build()`を維持し、probe feature＋Windows MSVC時だけ`rustc-link-arg-examples`でmanifestを埋め込む。実PEのresource #1からCommon-Controls v6とasInvoker/uiAccess=falseを抽出確認した。
- 検証helperを追加。既存の完全一致probeまたは同名で実体不明のプロセスがあれば重複起動しない。既存祖先・exe・出力ディレクトリ・ログのreparseを拒否。これは事前検査でありatomicな保護ではなく、ランチャーは逐次実行する。

## 実行と結果の読み方

リポジトリ直下で:

```powershell
cargo build --manifest-path app/src-tauri/Cargo.toml --example visual_preview_probe --features visual-preview-probe --locked --offline
.\app\verification\visual-editor\Run-NativeProbe.ps1 -Mode Inspect
.\app\verification\visual-editor\Run-NativeProbe.ps1 -Mode Run -Policy NativeOnly
.\app\verification\visual-editor\Run-NativeProbe.ps1 -Mode Run -Policy Layered
```

GUI起動はこの環境の権限付き実行を使う。`Inspect`は読取りのみ。`Run`は固定EXEと固定引数だけを使い、非表示起動して自身のPIDを25秒待つ。`Start-Process -Wait`の子孫全体待機は使わない。stdout/stderrをGUID別のlauncherログへ保存し、native終了コードを返す（待機超過5）。プロセスの自動停止はしない。

独立identifierは`com.agentdock.visual-preview-probe`。profile/reportは`app/src-tauri/target/visual-probe/run-<GUID>/`、起動記録は`launcher-<GUID>/`。自動削除しない。通常アプリ領域や`~/.codex`へのアクセスはない。攻撃fixtureのHTTP宛先は別ポートのloopback canaryのみ。実在する秘密ファイル・コマンドを使わず、OS全体の通信を計測した試験ではない。

NativeOnlyはCSPなしでnative層を観測する比較条件。LayeredはHTTP CSPを併用し、token付きfixtureのscript/style/image/connect/frameを限定許可する。fixture用inline scriptは許可し、fontとinline styleは不許可。default/object/worker/base/formsも拒否。policyを変えてもTCP接続を検出したら失敗の判定は緩めない。

通常はsetup後6秒で観測を保存する。native終了0は設定完了・canary接続未観測という限定条件のみで、V0全体の合格ではない。2はcanary接続または設定未完了、3はreport保存失敗、4はdefault拒否設定失敗で即時終了、5は起動/終了の20秒watchdog。mainより前のloader失敗をRust watchdogで止めることはできないためhelper側にも待機上限を持つ。

## 実測（2026-10-25、JST）

|条件|時刻/結果|記録|
|---|---|---|
|NativeOnly|11:15:22〜11:15:30、native終了2|`run-228106001178368224478522710247296842274/report.json`|
|Layered|11:19:35〜11:19:42、native終了2|`run-250425057571520161051585373759972646675/report.json`|

両条件で`CANARY_REACHED`が発生。LayeredではCSP違反（connect/img/frame）と要求拒否を観測したが、別ポートへのTCP接続は届いた。追加計測は`readBytes=0`、`firstHttpLine=""`、`readOutcome=timeout`（150ms）。**その時間内にHTTP要求行を観測していないだけであり、通信遮断成功や送信なしを断定しない。** preconnect等の可能性はあるが原因は未確定。

Layeredの制御fixtureから、相対module=true、CSS=`rgb(23, 71, 125)`、SVG image=true、parent DOM reachable=falseを観測した。これらはページ自己申告の補助証拠。native記録でもHTML/CSS/module/SVGの許可URLを確認。child controller boundsはleft=0/top=0/right=640/bottom=520。これだけで製品中央UIの位置・追従・重なりを検証したとはしない。

`__TAURI_INTERNALS__`と`chrome.webview`はiframe側でobjectとして見えた。globalsを隠しただけの隔離は前提にできない。probeは製品handlerを持たないため、製品独自commandの拒否を証明していない。reportは`productionIpcTested=false`、`centralEmbedTested=false`、`observations-only-not-V0-acceptance`を維持する。

Layered試験のEXE SHA-256: `29BCE754F798B14B48783593AA704C4124CE3A75998AFB45DD8849C09E79E3D7`。起動後はhelperの`processes=[]`でprobeの終了を確認。全reportと起動ログはignoredなtarget配下に保持する。

## 実装境界

最初はabout:blank。すべてのnative handlerを登録してから未信頼navigationへ進む。`ICoreWebView2_22`の全resource context・全request source kindsのfilterを要求し、古いruntimeへ不完全なfallbackをしない。WebResourceRequestedは最初に合成403、URIとGETの完全一致確認後だけ解除する。遷移も最初にCancel、許可条件を確認してから解除する。別窓・downloadを拒否し、default拒否自体の設定が失敗したらprobeを終了する。

COM handlerはWebView破棄・プロセス終了まで保持。製品の長寿命セッションでの解除/再生成は別設計。shellの観測は固定schemaのJSON文字列にして即時転送し、source window・件数・長さを確認する。Wry側が文字列を期待するため、objectの通知から変更した。観測データは未信頼であり操作権限にしない。

canaryはTCP接続の事実を先に失敗条件として扱い、別スレッドで最大512bytes・150msの先頭行を補助記録する。共有イベントmutexを持ったままsocketを待たない。HTTP行なしでも接続失敗判定を消さない。

## 確認と残課題

- Luna Tester: 初期基盤Rust試験7/7、通常cargo check、probe build成功。manifest/build.rs修正後の通常cargo checkも成功（39.04秒）。最後のprobe buildは7.65秒で成功。試験/レビューの成功を通信隔離成功に読み替えない。
- Sol独立レビュー: helperの区切り文字とreparse検査の指摘を修正。限定実験コードの必須修正なし。実reportをLuna/Solとも照合し、V0不合格を確認。
- stderrにはexampleの既定icon resource不足(0x80070715)とChrome_WidgetWin_0解除の1412が残る。これらが出てもreportは生成された。通常アプリの不具合と同一視しない。
- **次の設計判断は通信境界の補強。CSP＋WebResourceRequestedだけでは今回の厳格なTCP遮断条件を満たさなかった。** 未確認のbrowser flagを安全保証として足さず、専用host/OS側の通信制限等を調査する。Windowsの設定やfirewallを無断で変更しない。
- 製品IPC ACL、WebSocket/WebRTC/worker等、全階層frame/redirect、中央UI、選択橋、安全な元ファイル反映は受入未完了。製品へ公開しない。

## ローカル依存ソースの根拠

ロック版: Tauri 2.12.1 / wry 0.57.0 / webview2-com 0.39.1。

- `tauri-build-2.7.1/src/windows-app-manifest.xml`はCommon-Controls v6を宣言するが、`tauri-winres`→`embed-resource-3.0.11/src/lib.rs`は通常bin向けのlink引数を出力し、exampleには適用されなかった。
- `tauri-runtime-wry-2.12.1/src/dialog/windows.rs`はTaskDialogIndirectを使用する。
- `webview2-com-sys-0.39.1/src/bindings.rs`の`ICoreWebView2_22`、frame navigation、download、合成response APIを使う。
- `wry-0.57.0/src/webview2/mod.rs`の既存WebMessageReceivedは文字列として取得するため、probeのshellもJSON文字列を渡す。
