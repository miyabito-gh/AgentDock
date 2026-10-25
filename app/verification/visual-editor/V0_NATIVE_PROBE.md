# V0 native preview probe（製品未接続）

`examples/visual_preview_probe.rs` は Windows WebView2 の限定技術検証。AgentDockのライブラリ、Host、独自コマンド、プラグインを起動しない。通常アプリの起動方法・既定featureは変更しない。

## 確認状態

- 2026-10-25: `cargo check --manifest-path app/src-tauri/Cargo.toml --example visual_preview_probe --features visual-preview-probe --locked --offline` 成功。
- 最終固定ソースをLuna Testerが `cargo build --manifest-path app/src-tauri/Cargo.toml --example visual_preview_probe --features visual-preview-probe --locked --offline` で再ビルドし成功（59.94秒）。Solの独立レビューは初期基盤とprobeソースの限定範囲で必須修正0件。
- GUI起動の権限確認が中断され、probeの実行結果JSONは得られていない。別経路での起動は行っていない。
- 実WebViewでの実行・安全性・相対資源の表示は未検証。V0合格ではない。
- 独立した非表示1000×700窓の(180,80)へ640×520のchild WebViewを配置。`tauri/unstable`はprobe featureでだけ有効化する。native controllerのBoundsを`child-native-bounds`へ記録する。これだけで製品中央UI・モーダルの重なりや製品IPCの拒否を証明したとはしない。

## ビルドと任意実行

リポジトリ直下で:

```powershell
cargo build --manifest-path app/src-tauri/Cargo.toml --example visual_preview_probe --features visual-preview-probe --locked --offline
# 次はWindows GUIプロセスの起動。試験実行を認めた場面だけで実行する。
& .\app\src-tauri\target\debug\examples\visual_preview_probe.exe
```

独立identifierは `com.agentdock.visual-preview-probe`。プロファイルと結果は `app/src-tauri/target/visual-probe/run-<GUID>/`。既存アプリ領域・`~/.codex`へアクセスするコードはない。毎回新規プロファイルを使い、既存試験結果を自動削除しない。二つのephemeralポートへ127.0.0.1限定でbindする。攻撃HTTP先は別ポートのloopback canaryだけ。`file:`は存在しない固定パス、`tauri:`は固定の無効パス。任意コマンドや実在ファイルを試験に使わない。

通常はsetup後6秒でJSONを保存して終了する。終了0は「native設定完了、canary接続の観測なし」のみ。表示・全攻撃拒否の合格ではない。終了2はcanary到達またはnative設定未完了、3は結果保存失敗、4はnative拒否設定失敗による即時終了（この場合はstderrに理由）。5は起動／終了が20秒を超えたwatchdog強制終了。setup自体が失敗した場合もエラー終了する。

## 実装境界と観測方法

最初は `about:blank`。native handlerをすべて登録してからshellをNavigateする。設定失敗時は未信頼ページへ進まない。WebResourceRequestedでは最初に合成403を設定し、URI・method取得が成功して**完全一致のトークンURLかつGET**の場合だけ応答を解除する。default拒否の設定自体が失敗したらprobeプロセスを終了する。

`ICoreWebView2_22::AddWebResourceRequestedFilterWithRequestSourceKinds("*", ALL context, ALL source kinds)` が必要。古いruntimeでは限定版へフォールバックせず起動を失敗させる。top navigationとframe navigationは初回のshell／fixture URLだけ許可し、先にCancelを設定する。別窓はHandled、downloadはCancelで拒否する。登録handlerはCOMのadd_*が保持し、WebView破棄・probeプロセス終了まで存続する。製品の長寿命セッションでの解除／再生成設計は別途必要。

childはsetupで作成する（同期command/eventからのadd_child呼出しはWindowsでdeadlockの既知制約がある）。CSPは意図的に無効。sandboxは `allow-scripts` だけ。sandboxがpopupやfile等を先に拒否する場合があるため、イベントがない攻撃をnative遮断成功と数えない。`native-resource`はnative filterで実際に観測したURIと許可判定。tokenを含むfixture URLは相対表記にする。`CANARY_REACHED`は別ポートへのTCP接続が起きた強い失敗証拠。fixture由来の`untrusted-page-report`は未信頼の補助観測に過ぎない。

fixtureは相対CSS・JS module・SVG、別loopback fetch/image/nested iframe、許可されたredirect元からcanaryへの302、iframe自身の再navigation、file/app scheme、popup、親DOMと偽コマンド名を試す。親shellがiframeのsource windowを照合して最大40件・各512文字へ制限するが、製品用のsession/revision/nonce/rate検証付き選択橋は実装していない。

## まだ証明していないこと

- 製品main DOM、独自コマンド・承認・ファイル操作への到達拒否。製品Hostが存在しないため、このprobeだけでは検証不能。
- 製品中央領域との追従、モーダルとの重なり、選択橋、HTMLソース対応、編集コピー、安全な元ファイル置換。
- WebSocket/WebRTC、worker/serviceworker、download、外部scheme全般の完全な遮断。ALL source kindsを登録した事実と実測を区別する。
- nested frameの全階層やすべてのredirectのnavigationイベント保証。resource観測とcanary結果で個別確認する。
- ページ自己申告の資源成功が信頼できること。相対資源fixtureの結果は確認の手掛かりで、製品受入では画面・native記録との照合が必要。

## ローカル依存ソースの根拠

ロック版: Tauri 2.12.1 / wry 0.57.0 / webview2-com 0.39.1。

- `tauri-2.12.1/src/webview/mod.rs` の `PlatformWebview::controller()` / `environment()` がnative COMへの入口。
- `webview2-com-sys-0.39.1/src/bindings.rs` の `ICoreWebView2_22` と `AddWebResourceRequestedFilterWithRequestSourceKinds` が全source-kind filter。
- 同bindingsの `ICoreWebView2::add_FrameNavigationStarting`、`ICoreWebView2_4::add_DownloadStarting`、`ICoreWebView2Environment::CreateWebResourceResponse` を使用。
- `webview2-com-0.39.1/src/callback.rs` のCOM callbackラッパーを使用。独自unsafe COM vtableを実装しない。

依存追加はoptionalなwebview2-comとwindows、およびprobe限定のtauri/unstableだけ。既存lock版を使い、新規バージョン解決を行っていない。
