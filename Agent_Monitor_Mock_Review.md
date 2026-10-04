# Agent Monitor モック改訂・レビュー記録

更新日: 2026-10-04（日本時間）
対象: Agent_Monitor_Mock.html（上部メニュー版）
状態: 操作モック。Codex本体・App Serverへの接続、実機技術検証なし。

## デザインと配置

青灰色・抑えた青のデスクトップ作業環境として全面再作成。会話を中央、履歴を左、動的監視を右に配置した。上部のチャット／作業／表示／設定／ヘルプへ低頻度操作を集約し、会話には入力・文脈追加・モデル・effort・計画実行・送信方式、必要時の承認と送信待ちを残した。

## 参照資料

- [frontend-design指針](https://github.com/anthropics/claude-code/blob/main/plugins/frontend-design/skills/frontend-design/SKILL.md): 目的に沿った視覚設計、抑制、明確な操作名、自己レビューを適用。
- [Codex IDE extension](https://learn.chatgpt.com/docs/codex/ide): composerからファイル・選択範囲・過去会話を指定する目的を参照。
- [Developer settings](https://learn.chatgpt.com/docs/developer-settings?surface=ide): queue/steerとEnter挙動を確認。初期値は本要件の合意を優先。
- [ChatGPT Windows app](https://help.openai.com/en/articles/9982051-using-the-chatgpt-windows-app): 履歴・モデル選択・ファイル利用の公式説明を参照。
- [Working with files](https://openai.com/academy/working-with-files/): 追加メニューからのファイル添付を参照。

資料による確認であり、ユーザー導入済み拡張26.917.62051の画面を直接観察したものではない。音声・画像生成等のChatGPT固有機能は追加していない。

## solサブエージェントによる再レビュー

指定モデル: gpt-6-sol。静的コードとChromeの操作をレビュー。レビュー担当はファイルを編集せず、主担当が修正した。

|指摘|修正|再確認|
|---|---|---|
|送信待ち登録時に添付・参考会話・Skill指定が消失する|キューごとに文脈情報を保持し、送信待ちに名前を表示。取消で対応情報も除去|evidence.txt添付→登録→表示→取消をChromeで確認|
|追加指示へ切替後の送信ボタン読み上げ名が古い|方式変更時にaria-labelを更新|切替直後に「追加指示を送信」を確認|

主担当による追加修正: 最後のチャット削除後に空の新規チャットへ復帰、承認操作が見える位置へスクロール、チャット別下書き保持、最近利用順と初期名、分岐操作名の誤解を整理。

## 検証結果

- Chromeで実行し、pageerrorなし。
- チャット切替で下書きを保持。
- 送信待ち登録と添付情報保持、送信ラベル更新を確認。
- 差分ダイアログを表示。
- デスクトップ1440pxと狭い画面390pxを確認。390pxで横はみ出しなし。
- solの修正後レビューで新たな重要不具合なし。

## 確認範囲の限界

これはUI確認用のモックであり、全機能の製品実装ではない。データはサンプルで再読込み時にリセットする。ファイル追加は名前のみを表示し、内容を読まない。モデル候補はサンプル。クラウド・Plugins等の技術経路は未確認。OSのトレイ・最前面、実際の中断・削除・権限操作は行わない。Markdown出力はサンプル会話のダウンロードとして動作する。


## ボタン・監視カード改訂の再レビュー（2026-10-04）

- デスクトップの一般操作28px／送信32pxを基準とし、余白・文字サイズ・角丸を統一。狭い画面・タッチでは押下領域を広げる。
- 右ペインは親／子ラベル、接続線、名前、状態、作業内容を備えた常時表示カードに改訂。状態は色と文字で示す。終了済み非表示時の件数と空状態案内を修正。
- 参考: [Linear Board layout](https://linear.app/docs/board-layout)、[Geist Button](https://vercel.com/geist/button)、[Geist Context Card](https://vercel.com/geist/context-card)。現在公開中の公式設計を参照し、本アプリに合わせて構成した。
- gpt-6-solを新規起動して再レビュー。Chromeで1440／1100／390px、カード階層・バッジ、全チャットと終了済み、失敗確認を検証。重大な問題なし。モバイルでボタンが拡大する点は意図した仕様として確認。
- 主担当も同幅と下書き・送信待ち・添付保持・送信ラベル・差分を検証。横はみ出し・JavaScriptエラーなし。
- 最終版は操作モック。Codex実接続・App Serverの技術確認は行っていない。
