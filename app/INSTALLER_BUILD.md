# Windowsインストーラーの作成記録

2026-10-24。ソースHEAD d7a4c28（実装3ac8b9f）からNSIS x64を作成。ビルド中の変更は引き継ぎ文書の文字化け修復だけで、アプリソースは変更していない。

- コマンド: app/で `npm.cmd run tauri -- build --bundles nsis`
- 結果: 終了コード0、release最適化成功、NSIS生成成功。
- 通常実行はesbuildの親ディレクトリアクセス拒否で失敗。許可付き再実行で成功。
- 出力: `app/src-tauri/target/release/bundle/nsis/AgentDock_0.0.1_x64-setup.exe`
- サイズ: 7024126 bytes
- SHA-256: `CCC9FD54BFC2E954D042564ADF80A726F96853D3CDC03053D5FC5FAFD95AE75A`
- 詳細なハッシュと時刻は同じ場所の `.exe.build.json`。
- インストーラーとアプリは起動していない。実Codex受入も未実施。
- アプリ識別子/保存先/版番号0.0.1は維持。既存インストールを更新するため、実行前に旧AgentDockの作業を終了する。
- 会話の手動継続を設定オフでも使えるようにする案は未実装で、この配布物にも含めていない。

PowerShellで実行:

```powershell
Start-Process -FilePath "C:\Users\wmasa\Documents\Rust\AgentDock_next\app\src-tauri\target\release\bundle\nsis\AgentDock_0.0.1_x64-setup.exe" -Wait
```
