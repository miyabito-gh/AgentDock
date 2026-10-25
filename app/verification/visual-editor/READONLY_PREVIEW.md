# Visual Editor 表示・選択スライス

2026-10-25。状態: **読取りプレビュー・要素選択・既存チャット入力への挿入を実装。専用WebViewの限定実機試験に合格。全体MVPは未完成。**

## 現在できる操作

- 上部のVisual Editorで、中央プレビュー・右既存チャットへ切り替える。左の既存情報は維持する。
- HTMLを開く。表示ルートはHTMLの親フォルダーを初期値とし、必要なら親rootへ変更できる。
- 再読み込み、選択/操作切替、hover、クリック単一、Ctrl＋クリック複数、主選択の親要素、Esc/解除。
- 色・サイズ・配置・テキスト・レイアウトのひな形と要素情報を、既存の下書きを残して入力欄へ追記する。追記だけでは送信しない。

この段階の送信は通常のチャット操作。Visual専用の編集コピー・transaction・自動更新・採用・復元はまだ付かない。元ファイルの編集を指示した場合は通常チャットの送信経路に進み、Visual専用の編集コピーや採用制御は適用されない。画面にも採用・復元未対応を表示する。ソース位置は未確認で、選択情報は未信頼の参照情報として扱う。

## 通信と権限

HTTP(S)の外部資源/API/別loopbackを許可。CORS等の通常ブラウザー制約は維持する。data/blob資源も許可する。完全なTCP遮断やOS通信隔離を製品受入条件にしない。

プレビューはmainと別labelのchild WebView。全application command入口をmain/monitorのexact labelに制限し、preview制御はmainだけ。plugin capabilityもwindowではなくexact webviewを対象にする。HTTP remote originのTauri ACLも適用される。host objectsを許可せず、file/app独自scheme・app内部hostを拒否する。通知は固定schemaの選択情報だけ、main宛に送る。

静的配信はtoken必須GET/HEAD、Host完全一致、明示root内の許可表示形式のsnapshotだけ。秘密/cache/実行ファイル/reparseは除外。URLのencode/traversal・未知パスを拒否する。root相対資源は同じWebView内でtoken経路へ対応付ける。JSによるファイル書込みや任意パスの読取りAPIは提供しない。

## 現在の制限

- root走査4096entry、512asset、各8MiB・合計64MiB。大きい単体assetは除外する。大量の無関係ファイルがあるrootでは開けない場合がある。
- DOM生成要素・shadow DOM・別frameのソース位置対応は未完成。selectorは編集対象の一意性を保証しない。
- topの別ページ移動、popup、downloadはこの表示スライスの対象外。同一entryのquery/hashとページ内操作、資源/API通信は利用可能。
- native通知は20件/秒、64KiB、最大64選択。UIは最大50件を表示し、省略があれば挿入を止めて選択縮小を案内する。
- native childはDOMメニュー等より上に描画されるため、overlay/ダイアログ/チャット切替時は退避・終了する。close未確認は成功にせず再試行を表示する。
- React本画面の実マウス操作、DPI/狭幅、通常/監視画面の包括的なGUI回帰、全file-scheme/全plugin/HTTPS/CDN/LANは今回の専用実測では未確認。

## 検証

Luna Testerが通常cargo checkと関連Rust13件、画面build（tsc含む）、payload/Windows親path/draftの一時実行を確認。Sol Workerの追加Windows canonical path回帰1件成功。親がURL生成→配信解決の空白/日本語回帰を追加し、旧二重slashで失敗・修正後成功を確認。さらに専用harnessの拒否理由分類回帰1件成功。ビルドは既存ts-rs/hard-link/import分割/chunk容量の警告を残す。

専用exampleは製品のPreviewManager・4commands・invoke guardを再利用する。通常Host/store/pluginの起動やCodex接続はしない。identifier `com.agentdock.visual-editor-boundary-probe`、fixture/main profile/reportはignoredなtarget/visual-editor-probe/run-GUIDへ保存。preview profileは同identifier専用のAppLocalData配下。実行担当は親のみ、固定helperで逐次実行し、自身の終了結果を回収する。

```powershell
cargo build --manifest-path app/src-tauri/Cargo.toml --example visual_editor_boundary_probe --features visual-preview-probe --locked --offline
.\app\verification\visual-editor\Run-NativeProbe.ps1 -Mode Run -Policy Editor
```

### 実測履歴（JST）

|run|結果|内容|
|---|---|---|
|e72132c73c6d4480b6a10aed80692e4f / 12:27|終了5|設定basenameをTauriが無視して旧probe設定を読込み、controller未到達。previewProfileは算出metadataのみで実利用なし|
|7300bb1bd35f42fa8e837d69f95b5009 / 12:30|終了2|API/native表示・非表示/root変更は成功。生成URLの二重slashでHTML404、選択失敗|
|4d9bb3b60c494b57b443cb62fb0ae718 / 13:29|終了2|URL修正後、表示/選択等成功。既知commandのTauri ACL拒否文言をharnessが未分類|
|0dc4138a231b4e8fac930bf1b43b3d69 / 13:31:49〜53|**終了0**|全限定受入check成功|

最後のEXE SHA-256: `5BD471278C0A840AB73805FC73D8C72AB629B235A731F42780472E3D9995755C`。reportは`target/visual-editor-probe/run-0dc4138a231b4e8fac930bf1b43b3d69/report.json`、launcherは`071e64522ccf44c7824f410c1d9c667a`。helperのprocesses=[]で自身の終了を確認。全失敗記録も保持する。

相対HTML/CSS/module/画像、root相対画像、data/blob画像、loopback APIのJSON取得、[one]→[one,two]→[one,group]→[]、native visible/width切替、close後childなし、未許可/秘密pathの404、悪意extra fieldの通知拒否を確認。主選択parentでは他選択を保持する。通常形式のroot変更は実測、verbatim形式の比較はRust回帰で確認した（harnessの2入力は最終的に同一通常形式）。

アプリ操作の拒否はTauri 2.12.1のremote origin ACLによる実測。`probe_privileged_sentinel not allowed. Plugin not found`はapp ACL manifestが無い際の正確な診断であり、mainから同commandを1回実行できることと、previewからcounterが増えないことを同時に条件化した。plugin:event|listenはwebview/URLを含む明示ACL拒否。timeout/ReferenceError/曖昧なnot foundは合格にしない。製品invoke wrapperを通る動的経路自体は未実測で、label回帰と静的レビューを根拠に分ける。

最終独立レビューはGPT-6 Sol。前任GPT-6.1 Solの利用上限により交代し、最後の限定修正と実行は親が引き継いだ。追加必須修正なし。専用DOMイベントによる検証であり、実マウス/React本画面/AI編集/採用の受入完了とはしない。

実測後の変更はUIの未対応案内とselection scriptの空行末尾の空白除去のみ。最終type-checkと通常cargo check（11.20秒）は成功。字句間空白の整形後は実機試験を反復していない。
