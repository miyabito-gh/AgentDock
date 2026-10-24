# 通常チャット改善の実装・検証記録

2026-10-24。feature/subagent-migration の0a136a1として保存済み。フォルダ切替後に再開し、通常機能U1〜U5の実装・関連テスト・型生成・フロントエンドビルドが完了。独立したLuna確認担当の静的レビューでは回帰指摘なし。**実機受入は未実施**。

## ソースに入れた変更

- U1: Composer が初回だけ保持していた model/effort/speed を、送信・キューと同じホストの selected 値から表示する。同一チャットの再開や設定更新、保存失敗時に表示だけ残る経路を除去。effort=null は「既定」と表示。一覧にない保存済みのモデル・effortも選択値を明示する。
- U2: clipboardData.files に露出する画像を先頭1枚ではなく順番に読む。PNG/JPEG/GIF/WebP/BMP の MIME と対応する拡張子を保持し、UUID付き表示名で区別。不対応形式・読込失敗は通知。1枚25 MiBの既存画像上限をUIとホストで確認。複数画像が OS/WebView から露出しない形式は取得できず、対応保証していない。モデルの画像理解も保証しない。
- U3: 取得済みdiffを4000行ずつ描画し、前へ・次へ・末尾へと表示範囲を追加。解析は本文ごとに memoize、本文変更時は先頭へ戻す。ホストの16 MiB超エラーは維持。巨大diffの実機負荷測定は未実施。
- U4: 設定に未指定の既定値を持つ externalTools を追加。VS Code/Cursor のファイル・行（--goto）、Windows Terminal/PowerShell のフォルダ起動を固定引数配列で用意。実行ファイルはローカル .exe 絶対パスを指定し、起動時に存在確認。任意の追加引数・シェル文字列補間は使わない。既存 open_path の作業フォルダ外・実行形式の確認、会話リンクの workspaceOnly 制限を通す。UIは会話ファイルメニューとチャット上部「ターミナル」、設定「外部ツール」。プロセス生成までの確認で、外部アプリの準備完了・行移動成功は観測しない。既存設定は serde default で読む設計で、外部ツール設定の空オブジェクト既定値テストは成功。実際の保存データには一切触れていない。
- U5: 読込済みチャットを名前・作業フォルダ・親の実状態で絞る。フォルダは現在/次回/受理済みフォルダを対象とする。状態はルート status.state を直接使い、未取得は unknown。アーカイブ・一覧除外・既存入れ子分類は保持する。会話横断本文索引は未実装。

## 検証結果と残作業

- 初回のlock/tsc不足は、親担当が旧基準lockを復元して `npm.cmd ci --offline` を実施し解消。lockの保存コミットは40f3ae4。
- `cargo test --manifest-path app/src-tauri/Cargo.toml --lib external_tools --locked`: 成功、6件（固定引数/設定検証2件、ExternalToolsSettings / EditorKind / TerminalKind / OpenTargetのts-rs出力4件）。
- `cargo test --manifest-path app/src-tauri/Cargo.toml --lib export_bindings_appsettings --locked`: 成功、1件。AppSettingsを再生成。
- `cargo test --manifest-path app/src-tauri/Cargo.toml --lib export_bindings_openpathargs --locked`: 成功、1件。OpenPathArgsを再生成。
- `app/` の `npm.cmd run build`: 成功（tsc + Vite、427 modules）。既存のdynamic/static import混在とchunkサイズ警告あり。型生成前に行った `npm.cmd run type-check` の新型未生成エラー5件は、このbuildで解消確認済み。
- Rustテストビルドで既存のts-rs serde transparent属性警告あり。
- 独立したLuna確認担当の静的レビューでは回帰指摘なし。非testのcargo checkは本担当では未実施。フロントエンドの実機操作と外部エディタ/ターミナルの起動試験は未実施。
- 新版アプリ・開発サーバー・実Codexは起動していない。旧アプリPID54056が稼働中と親が確認しているため、同じ保存領域へ書く試験は利用者が停止・切替を確認するまで行わない。~/.codex・認証・通常会話・旧組織履歴は変更していない。

## 変更ファイル

通常UI: src/ui/CenterPane.tsx, DiffParts.tsx, LeftPane.tsx, RefParts.tsx, Dialogs.tsx, 新規 ExternalToolsSettings.tsx。
接続: src/App.tsx, src/ipc/client.ts, live.ts, types.ts。
Rust: src-tauri/src/backend/local.rs, pathref.rs, commands.rs, lib.rs, host/attachments.rs, host/persist.rs, 新規 external_tools.rs。
共有ファイルの編集は終了し、新方式担当へ引き渡し済み。生成型はsrc/ipc/gen/配下6型。新方式担当のsubagentsファイルは本担当の変更に含めない。通常機能のコミットなし。
