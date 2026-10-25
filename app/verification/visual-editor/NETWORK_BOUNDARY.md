# Visual Editor 通信境界の再設計

2026-10-25。状態: **旧方針の参考資料。利用者が通信許可を明示承認したため、OS通信隔離は初期版の対象外。実装しない。** 以下は当時の提案であり、現在の作業計画ではない。

## 判明したこと

現行WebView2 155.0.4283.62では、CSP＋資源要求拒否＋frame遷移取消でも、子iframeの禁止先へTCP接続を観測した。許可資源を全てメモリ配信し、読込み前にCDP offline設定を完了しても再現した。HTTP要求は観測していない。ブラウザー内部の原因は未確定。[実測記録](V0_NATIVE_PROBE.md)を参照。

このため、資源要求の拒否をネットワーク接続そのものの拒否として扱わない。browser flagsやCDPの追加調整を製品の隔離保証にしない。初期要件の外部通信遮断は維持する。

## 次の候補と成立条件

第一の調査候補は、AgentDockとは別のpreview hostを、ネットワーク権限を付与しないOS実行境界に入れる方式。Windows AppContainer等の対応可否から確認する。MicrosoftはAppContainerのネットワーク資源制限を説明しているが、**その中で現行Tauri/WebView2が起動・表示でき、全子プロセスも閉じ込められることは確認していない**。

- AgentDock本体の通信・Codex・承認・元ファイル操作は通常hostへ残す。
- previewのbrowser/network serviceを含む全子プロセスが同じ制限を受けることを確認する。rendererだけの制限では不足。
- 許可した表示資源は、サイズと種類を制限した専用brokerで渡し、preview側でメモリ応答する。任意パスや任意コマンド、HTTP proxy機能を持たせない。
- 独立profile以外の書込みを許可しない。設定・認証・アプリ保存領域・元プロジェクトをpreviewへ渡さない。ただし、AppContainer単体がすべてのユーザーファイルの読取りを拒否するとは仮定せず、ACLとbrokerを別検証する。
- UIへの戻りは未信頼の選択・描画情報だけ。別プロセスの中央埋込み・入力・DPI・重なり・フォーカスも検証する。別窓の起動だけで仕様01を満たしたとしない。
- OS境界の起動・状態照合が失敗したら表示を開始しない。通常WebViewへ自動fallbackしない。

## 最初の検証単位

1. 固定された小さいpreview hostを独立して起動し、OS token/capabilityとbrowser/network serviceの帰属を読取り確認する。まだユーザーHTMLは読まない。
2. 無害な固定fixtureをメモリ配信し、HTML・CSS・module・SVGと中央配置の可否を確認する。
3. 失敗したnested-frameに加え、直接fetch/image/redirect/preconnect、WebSocket、WebRTCのloopback TCP/UDP宛て試験を行う。利用不可・未発火の経路は遮断成功とせず未試験とする。
4. 未許可ファイルと製品commandを拒否する。停止・異常終了後にも既存アプリの設定/通信/保存が変わらないことを確認する。
5. 専用データ・GUID・PID・SID・実行ファイルhash・OS/Runtime版・起動/終了結果を記録し、実装と別担当で検証/レビューする。

受入は「許可資源を表示できる」「試行が実際に発生したことを確認できる」「未許可通信・ファイル・製品IPCを拒否する」の組合せ。起動失敗や白紙を通信遮断成功にしない。

## システム変更の扱い

現時点ではAppContainer profileの作成、ACL変更、パッケージ登録、firewall設定は行っていない。AppContainer profile作成はユーザー別フォルダ/registryの作成を伴う。検証を実行する前に作成物・アクセス範囲・影響先を具体化する。共用WebView2実行ファイルを一括遮断するfirewall規則や、普段のブラウザー設定を変更する案は採らない。停止確認なしのprofile/履歴自動削除もしない。

OS方式が成立しない場合は、JS動作と厳格な通信遮断を両立できる別の実行基盤を検討する。JavaScriptを無効にする等の機能変更は現行合意から変わるため、勝手に初期仕様へ置き換えない。

## 一次資料

- [Microsoft: AppContainer isolation](https://learn.microsoft.com/en-us/windows/win32/secauthz/appcontainer-isolation)
- [Microsoft: CreateAppContainerProfile](https://learn.microsoft.com/en-us/windows/win32/api/userenv/nf-userenv-createappcontainerprofile)
- [Microsoft: WebView2 browser flags](https://learn.microsoft.com/en-us/microsoft-edge/webview2/concepts/webview-features-flags)
- [Chrome DevTools Protocol Network](https://chromedevtools.github.io/devtools-protocol/tot/Network/)