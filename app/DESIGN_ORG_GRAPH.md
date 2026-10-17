# DESIGN_ORG_GRAPH: 組織実行のグラフ定義と「案件」（設計書 v0.1）

- 作成: 2026-10-17（設計担当）。ブランチ `feature/org-engine`。
- 入力: `app/DESIGN_ORG.md` v0.4（§0.2・§1.3・§4・§5.4・§5.5）、`app/DESIGN_ORG_P4.md`、`app/REVIEW_ORG.md`、`app/DEFERRED.md`、正本 §3.15・§3.6（プロジェクト表示）、`app/org-engine/src/*`、`app/src-tauri/src/host/org/*`、`app/src/ui/OrgPane.tsx`。司令塔経由のユーザー追加依頼（Claude の再設計版 Projects の考え方、要約のみ）。
- 状態: **設計のみ・実装未着手**。正本 §3.15 の改訂案は §13。ユーザー判断は §15。
- 表記: 【決定】＝既存の確定事項・合意から直接導けるもの。【設計】＝設計担当の裁量。【要判断】＝§15。
- 構成: **第1部 全体像（ロードマップ）**、**第2部 最初のMVP（周回で品質を上げる＋テンプレート1の回帰なし）の実装指示**、第3部 安全・確定事項への影響・判断。

---

## 0. 結論（要点）

1. 流れはコードではなく**グラフ定義ファイル**（JSON、`org\graphs\<id>.json`）で決める。ノードは `agent`（役割＝モデル呼び出し）・`gate`（人の判断）・`router`（決定的な分岐）・`plannedTasks`（Plannerが出したタスクDAGを今のやり方で実行する）。辺は通常・条件つき・**フィードバック（最大周回数が必須）**。
2. **前向きの辺だけで循環なし（DAG）**、循環はフィードバックの辺だけで作れる、周回の数え方はワークフロー全体で1つの辺ごと（リセットしない）。この条件で、ノードが動く回数の上限は**開始前に計算できる**。上限を超える定義は読み込み時に無効にする。turn上限（Q11）は従来どおり最後の歯止めとして必ず効く。
3. 既存の業務状態×確認状態・`OrgEvent`・送信権・照合はそのまま使う。**1ノード＝1タスク＝1チャット**。周回は新しいタスクを作らず、今の Worker⇄Reviewer と同じ `reviewing`／`rework` の遷移で回す。状態機械の表は変えない。
4. テンプレート1（今の Planner→Worker→Reviewer）は `planner`（agent）→`tasks`（plannedTasks）の2ノードで表す。plannedTasks の中身は今のコードを**ほぼそのまま**別モジュールへ移すだけなので、既存テストは期待値を変えずに通る。旧 `events.jsonl`／`workflow.json`（グラフの記録がないもの）はテンプレート1として読む。
5. 共有状態は**スロット**（型つきの成果物。版ごとに保存）。今は成果物の要約がメモリにしかなく、再起動で消える。これを保存するように変える（再起動後もプロンプトに前の版と指摘が入る）。
6. 成果物の置き場は `git`（今の worktree＋コミット）と `folder`（AgentDock 専用の作業フォルダ＋**スナップショット**でレビュー対象を固定）。MVPは `git` だけ。`folder` は業務向けの第2段階。
7. 口を出す入口は全部**ワークフロー画面からの利用者操作**（`UserConfirmed` 付き）で、エンジンの入力イベントになる。役割チャットへ直接打ち込む経路は作らない（Q9 のまま）。実行中の追加指示は**承認カードを通して**、以後の指示に追記する。
8. Claude の再設計版 Projects に当たるものは「**案件**」（ゴール・共有メモリ・ライブラリ・使用量）と「**窓口チャット**」（コーディネーター）として第3〜4段階に置く。既存の停止中の「プロジェクト」（Codex 由来の作業フォルダの束ね）とは名前で分ける。
9. 「目的別の継続チャット」は**案件の窓口チャットだけを使う形**として自然に収まる。引き継ぎは利用者が送信したときだけ行い、自動の送信はしない。

---

# 第1部 全体像（ロードマップ）

## 1. 段階（全体）

|段階|内容|利用者から見えること|
|---|---|---|
|**G-A（MVP-1）**|グラフ定義・検証・テンプレート、グラフを解釈するエンジン（agent・router・gate・フィードバック）、スロットの保存、標準以外の役割、git の置き場、開始ダイアログでテンプレートを選ぶ、読み取り専用のグラフ表示と周回の表示、理由を書いて差し戻す（入口①）|「品質向上周回」テンプレートで、計画→実装→検証を最大N周回せる。テンプレート1は今と同じ|
|**G-B（MVP-2）**|folder の置き場（スナップショット）、業務テンプレート（議事録・調査メモ・メール下書き）、入口②（実行中の追記は承認カード経由）、入口③（完了後の続き＝同じ系統の新しいワークフロー）、成果物の保存|Git なしで文書の周回ができる。続きを頼める|
|**G-C（案件）**|案件＝ゴール＋作業フォルダ／入力＋共有メモリ＋ライブラリ＋使用量。ワークフローは案件に属する。メモリの更新は提案→利用者の承認|複数のワークフローで決定事項・成果物を引き継げる。案件ごとの使用量が見える|
|**G-D（窓口チャット）**|案件の窓口チャット（利用者が普通に話す会話）。AIは「ワークフローの開始・追記・メモリ更新」を**提案**し、承認カードで実行。長くなったら利用者の次の送信で裏の thread を引き継ぐ（継続チャット）|1つの会話で指示し、裏で複数のワークフローが動く|
|**G-E（拡張）**|扇状の分岐と集約（複数の検証の比較）、動的な展開、入れ子の周回の上限の緩和、グラフ図の描画、エディタ|大きな作業の並列化|

- 依存: G-A → G-B → G-C → G-D。G-E は G-A の後ならいつでも。
- 順序の判断（継続チャットを G-C より先に出すか）は §15 の U5。

## 2. Claude の再設計版 Projects との対応（ユーザー追加依頼 (a)）

一次情報は読んでいない（司令塔の要約に基づく）。

|Claude Projects|AgentDock での対応|段階|
|---|---|---|
|ゴール駆動のワークスペース（ゴールとリポジトリ・文脈）|**案件**: ゴール（文章）、作業フォルダ／Git リポジトリ（任意）、入力ファイル、既定のテンプレート|G-C|
|コーディネーター・チャット（分解・振分け・レビュー・まとめ）|**窓口チャット**。分解・振分けはグラフ（テンプレート）とエンジンが決める（制御をモデル任せにしない）。窓口のAIは**提案**だけをし、実行は承認カードを通す|G-D|
|スレッド（並列の作業セッション、独自のブランチ・コピー）|**役割チャット**（グラフのノードの実行、1ノード1チャット）。ブランチ・コピー＝worktree（git）またはスナップショット付きの作業フォルダ（folder）|G-A／G-B|
|スレッド内のサブエージェント・ループ・ワークフロー|グラフの周回（フィードバックの辺）。Codex 側の子エージェントは今までどおり子孫として監視|G-A|
|共有メモリ（決定事項・スケジュール変更の引継ぎ）|2層。①**スロット**（1ワークフロー内の受渡し。機械的に保存）②**案件メモ**（ワークフローをまたぐ決定事項。役割は提案だけ、利用者が承認）|①G-A ②G-C|
|ライブラリ（利用者のファイル＋成果物の集約）|**案件ライブラリ**（利用者が追加したファイルのコピー＋ワークフローの成果物から利用者が「ライブラリへ入れる」を選んだもの）|G-C|
|進捗はメインで見て、個々のスレッドを開ける|ワークフロー画面（グラフ表示）。ノードを選ぶと役割チャットを開く（閲覧だけ）。窓口チャットの上部に進捗の帯|G-A（画面）・G-D（窓口）|
|スレッドが増えると利用上限に早く達する。使用量表示、役割ごとのモデル・effort|ワークフロー・案件ごとの使用量（turn 数と取得できたトークン数。未取得は未取得と表示）。役割定義のモデル・effort、開始ダイアログで最大 turn の見積り|G-A（見積り）・G-C（案件ごと）|

取り入れないもの【設計】: コーディネーターAIが自分の判断でスレッドを作って送ること。AgentDock では送れるのはエンジンが ready にしたタスクだけ（正本 §3.15.2）で、窓口のAIの出力は提案（承認カード）にしかならない。

## 3. 名前の整理（ユーザー追加依頼 (b)）

- 既存の「プロジェクト」（正本 §3.6）は **Codex の project/list に由来する作業フォルダの束ね**で、表示を停止中（`PROJECTS_ENABLED=false`）。中身は「Codex が持っているフォルダの分類」で、ゴールもメモリも持たない。
- 新しい概念は性質が違う（AgentDock が持つ、ゴール・メモリ・ライブラリ・ワークフローの入れ物）。同じ「プロジェクト」と呼ぶと、停止中の機能を戻したときに見出しが2種類になり、所属の推定（§3.6 の「推定」印）と混ざる。
- 推奨【要判断 U1】: 新しい概念の呼び名は「**案件**」。既存のものは、戻すときに「**Codexのプロジェクト**」と呼ぶ。内部の型名は `CaseId`／`CaseRecord`（Codex 固有の型を持たない。§2.3）。
- 関係: 案件は作業フォルダを持てるが、Codex の projectId とは結び付けない（推定もしない）。停止中のプロジェクト表示は案件と独立のまま。戻すかどうかは案件の仕様とは別に判断する。

## 4. 案件・共有メモリ・ライブラリ（G-C の設計。ユーザー追加依頼 (c)(d)）

### 4.1 保存場所と形式【設計】

```
%LOCALAPPDATA%\com.agentdock.app\cases\<caseId>\
  case.json            // ゴール・作業フォルダ・既定テンプレート・作成時刻（store/atomic.rs で原子的に書換え）
  memory.jsonl         // メモの追記ログ（提案・承認・却下・取下げ）。1行1イベント、schemaVersion付き
  library\manifest.json// ライブラリの項目（由来・sha256・サイズ・追加時刻・利用者のラベル）
  library\files\<id>\<元の名前>   // コピー（元ファイルは動かさない・変えない）
  usage.json           // 集計の写し（正本は各ワークフローの usage.jsonl）
```

- ワークフローは今の `org\<workflowId>\` のまま。`workflow.json` に `case: Option<CaseId>` を足す（`#[serde(default)]`）。
- `~/.codex` には何も書かない。Codex のメモリ機能・AGENTS.md も使わない（案件メモは AgentDock の中だけで、渡すときはプロンプトの資料として渡す）。
- 期限なし・容量不足で自動削除しない。削除は案件の削除操作だけで、既存の削除確認・削除保留（停止を確認できないワークフローがあれば止める）に従う。ライブラリの元ファイル・保存先のファイルは消さない。
- 外へ出すのは利用者の「保存…」「エクスポート」だけ。案件メモ・ライブラリを自動でどこかへ送らない・コピーしない。

### 4.2 メモの更新は誰が承認するか【設計】

- 役割（ノード）は出力の任意項目 `memoryProposals[]`（`{kind: decision|fact|schedule|todo, text}`）で**提案だけ**できる。エンジンは `proposed` として記録する。
- 利用者がワークフロー画面か案件画面で、まとめて「採用／直して採用／却下」する（`UserConfirmed`）。採用されたものだけが次の作業に渡る。
- 自動採用はしない（既定）。秘密情報らしい文字列（APIキー・トークン・パスワードの形）を含む提案には「機微の可能性」の印を付け、そのままでは採用できないようにする（直して採用は可）。検出は簡単な形の照合で、完全ではないと画面に書く。
- 利用者は案件画面からメモを直接書ける（利用者の操作なので承認不要）。

### 4.3 メモリの量とコンテキスト（利用枠）【設計】

- ノードは `memory: "none" | "pinned" | "all"` を持つ（既定 `pinned`）。渡すのは採用済みのメモだけで、ピン留め→新しい順に、合計の上限（既定 4,000字）まで。超えた分は「ほか N件は省略」と書く。
- ライブラリは**一覧（名前・ラベル・パス）だけ**を資料として渡し、本文は渡さない。役割が必要ならパスから読む（読取りは sandbox の範囲）。
- 開始ダイアログに「1 turn あたりに足される資料の目安（字数）」を出す。1 turn あたり約1.7万トークンの土台（§5.4）があるので、メモの量は土台より小さく抑えるのが目安だと説明する。

### 4.4 使用量の見える化【設計】

- 既存の `usage.jsonl`（turn ごと、`last.totalTokens`、未取得は None）をワークフロー→案件で集計する。表示は「turn 数／取得できたトークン数／未取得の turn 数」。未取得を0にしない。
- 役割ごとの表（役割・モデル・effort・turn 数・トークン）をワークフロー画面に出す。モデル・effort は役割定義の値と、受理された値（取れたとき）を並べる。
- 開始ダイアログに「最大 turn（定義から計算）× 約1.7万トークン（目安）」を出す（§7.4）。

## 5. 窓口チャットと継続チャット（G-D。ユーザー追加依頼 (e)）

### 5.1 窓口チャット【設計】

- 案件に1つ。利用者が普通に送信できる**利用者のチャット**（組織の役割チャットではない。Q9 の対象外）。送信は既存の利用者送信（`SendAuth::User`）。
- 利用者が送るとき、AgentDock は送信の中に「案件の現状」（ゴール・採用済みメモ・ライブラリ一覧・動いているワークフローの状態を決定的に組み立てた文）を資料として添える。**添えるのは利用者の送信の中だけ**。進捗が変わっても AgentDock から窓口へ勝手に送らない（監視目的のプロンプト追加をしない、M11）。
- 窓口のAIが返答の末尾に提案ブロック（決められたJSON、`agentdock-proposal`）を書いたら、AgentDock が解析して**承認カード**にする（ワークフローの開始・追記・メモの採用）。解析できなければ普通の文として表示するだけ。カードを押さない限り何も起きない。
- 窓口のAIはワークフローの役割チャットへ送れない。役割チャットの出力を読むのは、利用者が送るときの資料（決定的な要約とスロットの抜粋）としてだけ。

### 5.2 継続チャット（目的別の継続チャット）【設計】

- 「議事録チャット」のように、**窓口チャットだけを使う案件**として表す。会話を続け、必要なら「この議事録を整える」をテンプレートの開始提案として出す。
- 引き継ぎ: 裏の thread のトークン使用量が文脈の上限（`modelContextWindow`、取得できたとき）の 70% を超えたら、窓口に「次の送信で新しい会話へ引き継ぎます」と表示する。**利用者が次に送信したとき**、その送信の中で新しい thread を作り、引継ぎ資料（採用済みメモ、ライブラリ一覧、直近の数往復の原文、案件のゴール）と利用者の文を送る。古い thread は残す（削除・アーカイブしない）。
- 利用者からは1つの会話に見える（窓口チャットの表示は、案件が持つ thread の列を順につなげて出す）。thread ごとの区切りは薄い区切り線で示す。
- 使用量が取れないときは自動で引き継がず、「引き継ぐ」ボタンだけを出す（推定で決めない）。
- 引継ぎ資料をAIに要約させるかは U6（既定は要約しない＝追加のモデル呼び出しなし）。

### 5.3 今回の設計が邪魔にならないための約束（G-A／G-B で守ること）

- `Task.chat` は「今のチャット」として扱い、UI と保存形式で「タスク＝チャット1つ」を前提にしない（将来 `ChatRebound { task, chat, reason }` を足して、タスクが複数の thread を順に持てるようにする余地を残す）。
- スロット・メモ・ライブラリは thread の外にあるので、thread が替わっても受け渡しが途切れない。
- ワークフローの「系統」（入口③）は `lineage` で持ち、G-C で案件に移すときは系統→案件に読み替える。

---

# 第2部 MVP（G-A）とG-Bの実装指示

## 6. グラフ定義（データ）

### 6.1 形式と保存場所【設計】

- JSON（serde_json は既に使っている。TOML は crate がない、DESIGN_ORG_P4 D14）。人が書く前提なので、各要素に `description` を書ける。
- 場所: `%LOCALAPPDATA%\com.agentdock.app\org\graphs\<id>.json`。初回に既定テンプレートを `create_new` で作る（既存は上書きしない、役割定義と同じ）。読めないファイルは退避せず、無効として理由を出す（REVIEW_ORG L9 の教訓）。
- **開始時に定義の写しを `org\<workflowId>\graph.json` に保存**し、そのワークフローは最後まで写しで動く（途中の編集・再起動で流れが変わらない）。使った役割定義の写しと権限の指紋も `roles.json` として残す。再開時に今の役割定義と指紋が違えば、写しのまま動かし「定義が変わっています（このワークフローは開始時の定義で続けます）」と出す。

### 6.2 定義の例（品質向上周回、テンプレート2）

```json
{
  "schemaVersion": 1,
  "id": "quality-loop",
  "displayName": "計画→実装→検証を周回（品質向上）",
  "workspace": "git",
  "entry": "plan",
  "slots": {
    "spec":    { "schema": "spec" },
    "report":  { "schema": "workerReport" },
    "verdict": { "schema": "verdict3" }
  },
  "nodes": [
    { "id": "plan",   "kind": "agent", "role": "planner",  "output": "spec",
      "instructions": "依頼を、実装方針と受入条件に整理する。" },
    { "id": "build",  "kind": "agent", "role": "worker",   "output": "report", "capture": "commit",
      "reads": ["spec"] },
    { "id": "verify", "kind": "agent", "role": "verifier", "output": "verdict", "target": "build",
      "reads": ["spec", "report"] }
  ],
  "edges": [
    { "from": "plan",   "to": "build" },
    { "from": "build",  "to": "verify" },
    { "from": "verify", "to": "build", "when": { "slot": "verdict", "path": "verdict", "eq": "rework" },
      "feedback": { "maxRounds": 3, "onExhausted": "ask" } },
    { "from": "verify", "to": "plan",  "when": { "slot": "verdict", "path": "verdict", "eq": "replan" },
      "feedback": { "maxRounds": 1, "onExhausted": "ask" } }
  ]
}
```

- `verify` で `pass` のときは、出る辺（条件に合う辺）がないので終わる。`router` で明示もできる。

### 6.3 型（`org-engine/src/graph/def.rs`、骨格）

```rust
/// グラフ定義ファイル（`org\graphs\<id>.json`）。Codex の型を持たない。sandbox・承認方針・ネットワークの項目は
/// 意図して持たない（権限は役割定義だけが決める。グラフから権限を広げられない）。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct GraphDef {
    pub schema_version: u32,              // 1
    pub id: String,                        // ファイル名と一致。小文字英数字・-・_、1〜40字
    pub display_name: String,
    #[serde(default)] pub description: String,
    pub workspace: WorkspaceKind,          // git | folder
    pub entry: NodeId,
    #[serde(default)] pub slots: BTreeMap<SlotId, SlotDef>,
    pub nodes: Vec<NodeDef>,
    pub edges: Vec<EdgeDef>,
    /// 開始ダイアログの初期値の提案だけ（Q11 の既定30より上は利用者が開始時に選ぶ）。
    #[serde(default)] pub suggested_max_turns: Option<u32>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum WorkspaceKind {
    /// Git リポジトリが必要。書込みノードは専用 worktree、確定は AgentDock のコミット（今の方式）。
    Git,
    /// Git 不要。書込みノードは AgentDock 専用の作業フォルダ、確定はスナップショット（G-B）。
    Folder,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SlotDef {
    /// 出力スキーマID（組込み、または `org\schemas\<id>.json`。§6.6）。
    pub schema: String,
    #[serde(default)] pub description: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct NodeDef {
    pub id: NodeId,                        // 小文字英数字・-・_。タスクIDは `<id>-<n>`（§8.3）
    #[serde(default)] pub label: Option<String>,
    #[serde(flatten)] pub kind: NodeKind,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase", rename_all_fields = "camelCase")]
pub enum NodeKind {
    /// 役割＝モデル呼び出し1回（＋形式修正1回まで）。出力は `output` のスロットへ。
    Agent {
        role: RoleId,
        /// 書き込むスロット。スロットのスキーマが outputSchema になる。
        output: SlotId,
        /// この役割へ資料として渡すスロット（最新の版）。
        #[serde(default)] reads: Vec<SlotId>,
        /// ノード固有の指示（役割の指示の後に置く）。利用者が書いた定義なので指示として扱う。
        #[serde(default)] instructions: String,
        /// 書込み役の確定方法。書込み役（sandbox workspaceWrite）では必須。
        #[serde(default)] capture: Option<CaptureKind>,
        /// 読取り役が検証するノード（その確定物＝コミット／スナップショットに固定した場所で読む）。
        #[serde(default)] target: Option<NodeId>,
        /// 案件メモの渡し方（G-C。G-A では `none` 固定）。
        #[serde(default)] memory: MemoryUse,
    },
    /// 人の判断。選択肢ごとに出る辺を条件で選ぶ（出力スロットは {choice, note}）。自動で決めない。
    Gate { prompt: String, choices: Vec<GateChoice>, #[serde(default)] shows: Vec<SlotId> },
    /// 決定的な分岐。何も実行しない。出る辺は全部条件つき＋既定1本。
    Router,
    /// Planner の計画（`plan` スキーマのスロット）を今の方式で実行する（テンプレート1用）。葉に限る。
    PlannedTasks { from: SlotId, allowed_roles: Vec<RoleId> },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum CaptureKind { Commit, Snapshot }

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum MemoryUse { #[default] None, Pinned, All }

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct GateChoice { pub id: String, pub label: String, #[serde(default)] pub needs_note: bool }

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct EdgeDef {
    pub from: NodeId,
    pub to: NodeId,
    /// None＝無条件（router の既定、または1本だけの通常の辺）。
    #[serde(default)] pub when: Option<Condition>,
    /// Some＝フィードバックの辺（後ろ向き）。これがない辺で循環を作れない。
    #[serde(default)] pub feedback: Option<Feedback>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Feedback {
    /// 1〜5（`MAX_ROUNDS_PER_EDGE`）。必須。ワークフロー全体での回数で、外側の周回でも戻さない。
    pub max_rounds: u32,
    #[serde(default)] pub on_exhausted: OnExhausted,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum OnExhausted {
    /// 戻り先を `failed{上限到達}` にする（テンプレート1の差し戻し上限と同じ）。
    Fail,
    /// 戻り先を `blocked{周回上限}` にして利用者に「理由を書いてもう1周／この結果で受け入れる／取り消す」を出す。
    #[default] Ask,
}

/// 条件。式は書けない（評価器を持たない）。参照先はスロットのスキーマで enum・boolean・integer の項目に限る（検証で確認）。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Condition {
    pub slot: SlotId,
    /// "verdict" や "checks.tests"（ドット区切り、配列の添字なし）。
    pub path: String,
    #[serde(default)] pub eq: Option<serde_json::Value>,
    #[serde(default, rename = "in")] pub in_: Option<Vec<serde_json::Value>>,
    #[serde(default)] pub ne: Option<serde_json::Value>,
}
```

### 6.4 検証（`graph/validate.rs`、純関数）【設計】

`validate(def, roles, schemas, file_stem) -> Result<CompiledGraph, Vec<GraphError>>`。全部の誤りを返す。

|#|規則|理由|
|---|---|---|
|V1|`schemaVersion`=1、id がファイル名と一致、id・ノードID・スロットIDの文字種と長さ、ノード数 ≤ 16、辺数 ≤ 32|定義の暴走と表示の破綻を防ぐ|
|V2|`entry` が存在。全ノードが entry から**前向きの辺で到達可能**（到達不能ノードは誤り）|動かないノードを残さない|
|V3|フィードバックでない辺だけで**循環なし**|循環はフィードバックの辺だけ|
|V4|フィードバックの辺 S→T: T から S へ前向きの辺で到達できる（後ろ向き）、`maxRounds` 1〜5、`when` が必須（gate の選択肢でも可）|無条件の戻りは毎回上限まで回るため禁止|
|V5|前向きの入る辺はノードごとに最大1本（MVP。前向きの部分は entry からの木になる）。G-E で集約を入れるまで|周回の範囲（本体）が1本の道に決まる|
|V6|出る辺: 「無条件1本まで」または「全部条件つき＋任意で既定（無条件）1本」。無条件2本以上（扇状の分岐）は G-E まで誤り。同じ from→to の辺は1本だけ|分岐の意味を1つにする|
|V7|周回の本体（T から S までの道）は、互いに入れ子か交わらない。一部だけ重なるのは誤り。本体の途中のノードから本体の外へ出る辺は誤り（出口は S だけ）|周回の終わり方を1つにする|
|V8|`agent` の `role` が役割表にある。`output`・`reads` のスロットが定義にある。書込み役（役割の sandbox が workspaceWrite）は `capture` 必須で、`workspace` と合う（git＝commit、folder＝snapshot）。`target` は前向きにさかのぼれる書込み役ノード|検証対象を必ず固定する|
|V9|条件の `path` がスロットのスキーマで enum・boolean・integer の項目。`eq`・`in` の値がその型・enum の中|条件が型の外を参照しない|
|V10|`plannedTasks` は葉（出る辺なし）で、`from` が `plan` スキーマ。`allowedRoles` は役割表にある。グラフに1つまで|テンプレート1の範囲に限る|
|V11|**静的な上限**: 本体のノード数×maxRounds の総和＋ノード数（＋plannedTasks は `maxTasks×(1+maxRework)`）＝最大活動数 ≤ 100。超える定義は誤り|暴走しないことを開始前に保証|
|V12|gate の選択肢IDが一意で、各選択肢に対応する辺（`when: {slot:<gateの出力>, path:"choice", eq:<id>}`）がある|選べるのに進めない状態を作らない|

- 有効化されていない役割（権限を広げる定義）を使うグラフは**誤りではない**。読み込めるが、開始ダイアログで「役割 X が有効化されていません」を出し開始できない（有効化は既存の確認ダイアログ）。

```rust
/// 検証済みのグラフ（不変）。エンジンはこれだけを見る。
pub struct CompiledGraph {
    pub def: GraphDef,
    /// 定義本文の指紋（FNV-1a 64、役割定義と同じ方式）。開始時に記録し、写しの取り違えを検出する。
    pub fingerprint: String,
    /// ノード → 前向きの親（V5 により最大1）。
    pub parent: HashMap<NodeId, Option<NodeId>>,
    /// フィードバックの辺ごとの本体（T..S の道、T と S を含む）。
    pub loops: Vec<LoopBody>,
    /// V11 の計算値。開始ダイアログの見積りに使う（通常＝形式修正なし、最大＝各 turn に形式修正1回）。
    pub bound: ActivationBound,
}
pub struct LoopBody { pub edge: EdgeId, pub target: NodeId, pub source: NodeId, pub members: Vec<NodeId>, pub max_rounds: u32 }
pub struct ActivationBound { pub activations: u32, pub turns_typical: u32, pub turns_max: u32 }
/// `EdgeId` は `"<from>-><to>"`（V6 で同じ組の辺は1本に限るので一意）。
```

### 6.5 既定テンプレート【設計】

|id|段階|ノード|周回|置き場|
|---|---|---|---|---|
|`standard`（テンプレート1）|G-A|`planner`(agent, planner, output `plan`) → `tasks`(plannedTasks, from `plan`, allowed `worker`・`reviewer`)|plannedTasks 内の差し戻し（`Limits.max_rework`=2、`fail`）|git|
|`quality-loop`（テンプレート2）|G-A|§6.2|build⇄verify 最大3（ask）、verify→plan 最大1（ask）。最大活動数 3＋3×2＋1×3＝12、最大 turn 24|git|
|`minutes`（議事録）|G-B|`shape`(文書作成, output `document`, capture snapshot) → `summarize`(文書作成, output `document`, capture snapshot) → `check`(検証担当, output `verdict`, target summarize) →(rework)→ `summarize` 最大2 →(pass)→ `confirm`(gate: 「保存する／直してほしい（理由）」、直す→summarize 最大2)|2重|folder|
|`research-memo`（調査メモ）|G-B|`scope`(調査, output `spec`) → `research`(調査, output `document`, capture snapshot) → `check`(検証担当) ⇄ research 最大2|1|folder|
|`mail-draft`（メール下書き）|G-B|`draft`(文書作成) → `check`(検証担当) ⇄ draft 最大2 → `confirm`(gate)|1|folder|

- テンプレート1のファイルも作る（内容を見るため）。編集すれば別の流れになり得るが、それは利用者の定義として扱う。既存のワークフロー（グラフの記録なし）は、ファイルではなく**組込みの `standard`** で復元する（§8.5）。
- 新しい標準役割を2つ追加する（`org\roles\` に初回だけ作る）: `verifier`（検証担当。readOnly、detachedAtCommit、出力 `verdict3`）、`writer`（文書作成。workspaceWrite、ownWorktree＝専用の作業場所、出力 `document`）。`researcher`（調査。readOnly、sharedReadOnly、出力 `document`）は G-B。

### 6.6 出力スキーマ（`org-engine/src/outschema.rs`）【設計】

- 新しい依存（JSON Schema の検証 crate）は入れない。**小さな部分集合**を自前で持つ: `object`（`properties`・`required`・`additionalProperties:false` 固定）・`array`（`items`・`maxItems`）・`string`（`maxLength`・`enum`）・`integer`（`minimum`・`maximum`）・`boolean`。深さ ≤ 4、項目数 ≤ 32。
- 1つの定義から、`outputSchema` に送るJSONと、受け取った値の検証の両方を作る（ずれない）。
- 組込み: `plan`（今のもの）・`workerReport`（今）・`reviewerVerdict`（今。pass／rework）・`verdict3`（pass／rework／replan、findings[]、任意 `score` 0〜10）・`spec`（goal、steps[]、acceptance[]）・`document`（title、body（Markdown、≤ 60,000字）、notes[]）。利用者定義は `org\schemas\<id>.json`（G-B）。
- G-C 以降、全スキーマに任意項目 `memoryProposals` を足せる（スキーマ定義で `allowMemoryProposals: true`）。

## 7. エンジンの変更

### 7.1 方針【設計】

- `OrgCore` は1つのまま。固定の `RoleKind`（`core.rs:198-212`）を、**ノードの種類と定義から決まる振る舞い**に置き換える。sans-IO・`Persist` が先・送信は `send_turn` だけ、の規則は変えない。
- 今の `core.rs` の中で Planner→タスク表の処理（計画の解析・ポリシー判定・タスク追加・Worker⇄Reviewer の差し戻し・再計画）は `planned.rs` へ**移すだけ**（ロジックを書き換えない）。テンプレート1はこの経路で動くので、回帰しない。
- 新しいノード（agent・router・gate とフィードバック）は `graphrun.rs` で動かす。状態の表（`state.rs` の ALLOWED）は変えない。

### 7.2 ノードとタスクの対応【設計】

- **1ノード＝1タスク（活動）＝1チャット**。タスクIDは `<nodeId>-<n>`（テンプレート1の `planner-1` と同じ形）。n は利用者の操作で新しく作り直したときだけ増える（再計画など）。フィードバックによる周回は**同じタスク**で回す（今の Worker の差し戻しと同じ。チャットと作業場所も同じものを使う）。
- タスクは**辺を通ったときに作る**（使われない分岐のノードはタスクを持たない）。`TaskAdded` の代わりに `ActivationAdded` を記録する（§7.6）。
- 依存（`deps`）は「その活動を起こした前向きの親タスク」1つ（V5）。

### 7.3 1回の実行と周回の遷移【設計】

agent ノードのタスク X の turn が live で完了し、最終メッセージがスキーマに合い、子孫に作業中がない（今の `WaitDesc` の条件のまま）とき:

1. `SlotWritten { slot, version, task }` を記録し、値を保存する（§7.5）。
2. 書込み役なら確定（`capture`）: git は今の `Commit`、folder は `Snapshot`（G-B）。確定できなければ今と同じ `blocked{要対応}`。
3. **周回の本体に入っているが出口（S）でない**ノード → `reviewing`（「後の判定待ち」）。どの本体にも入っていない → `completed`。
4. 出る辺を選ぶ（`route.rs`、純関数。条件を宣言順に評価し、最初に合った辺。合わなければ既定。どれもなければ出口なし）。
   - 前向きの辺 → 行き先のタスクを作る（まだなければ）か、`rework` で待っている行き先を ready にする。
   - フィードバックの辺 S→T → 回数が残っていれば `FeedbackCounted{edge}` を記録し、T: `reviewing`→`rework`→`ready`（今の Worker と同じ）、本体の途中のノード: `reviewing`→`rework`（待ち）、S: `rework`（待ち）。残っていなければ `onExhausted`（`fail`＝T を `failed{上限到達}`、`ask`＝T を `blocked{周回上限}`、S は `completed`）。
   - S が前向きの辺を選んだ（または出口なし）→ 本体を閉じる: 本体の `reviewing` のノードを `completed`、S を `completed`。ただし**外側の本体がまだ開いている**ノードは `reviewing` のまま残す。
5. `rework` で待っているノード M は、本体の中の親 D について「D の最新のスロットの版が M の最後の出力より新しい（イベントの seq で比べる）」かつ D が `reviewing`／`completed` で確定、になったら ready に戻る。今の「reviewer は対象の worker が再び reviewing になったら ready」と同じ規則を一般化したもの。
6. 読取り役（`target` あり）が再び動くときは、今の `RefreshDetached` で対象の新しいコミットに作業場所を合わせてから送る（R-ORG M1 の修正と同じ）。

- router: 実行も turn もなし。親の完了で即座に 4. を行い `completed`。
- gate: タスクを `blocked{承認待ち}`（既存の `AwaitingApproval`）にして通知（即時）。利用者の選択で `GateDecided` を記録し、出力スロット `{choice, note}` を書いて 4. へ。**エンジンは gate を決めない**。gate はワークフロー全体を止めない（他の分岐は進む）。
- ワークフローの完了: 作られた全タスクが `completed`・確定で、選ばれていない辺が残っていない。今の `check_done` を一般化する。

### 7.4 上限と暴走しない保証【決定 Q4・Q11＋設計】

|歯止め|内容|
|---|---|
|静的|V3〜V7・V11。周回はフィードバックの辺の回数でしか起きず、その回数はワークフロー全体で数え、戻さない。よって活動数は開始前に計算した上限以下|
|turn|今の turn 上限（初期30）は全ノードに効く。形式修正も1 turn。到達したら `blocked{上限}`、「上限を増やして再開／終了」（今のまま）。**周回の途中でも必ず止まる**|
|開始ダイアログ|「通常 N turn、最大 M turn（形式修正込み）。1 turn あたり約1.7万トークンの土台があるため、最大でおよそ M×1.7万トークン（目安）」。M が turn 上限を超えるときは「途中で turn 上限に達して止まる可能性があります」|
|利用者の「もう1周」|`ask` の上限到達カードで利用者が選んだときだけ、その辺の許容回数を1増やす（`FeedbackAllowanceRaised`、`UserConfirmed`）。自動では増えない|
|並列|Worker同時2・全体3（Q4）。書込み役＝役割の sandbox が workspaceWrite（今の `Role.writes`）。G-B の folder では、同じ作業フォルダに書く役割は同時1|
|受理不明・再送|今のまま。周回は「新しい指示」（判定に基づく）であって再送ではない。受理不明・要照合・停止未確認のタスクは辺を選ばない・本体を閉じない（確認状態≠確定なら何もしない）|

### 7.5 スロットの保存【設計】

- 値は `org\<workflowId>\slots\<slotId>.<version>.json`（`store/atomic.rs`）。上限 256KB／版。超えた出力は形式違反扱い（形式修正1回→`failed{成果物不正}`）。
- 順序: エンジンは `OrgAction::PutSlot { slot, version, value }` を `Persist(SlotWritten)` の**直前**に出す。ホストは書けたら次の `Persist` へ進み、書けなければ以後の副作用を止める（`persist_failed` と同じ扱い）。これは外へ出る副作用ではないので、「Persist が先」の規則の例外として明記する（孤立したファイルが残っても害はない）。
- 再起動: ホストはワークフローを復元するとき `SlotWritten` が指すファイルを読み、`OrgCore::restored(state, slots)` に渡す。読めない版は「未取得」とし、プロンプトには「未取得」と書く。その版で辺を選ぶ必要があるタスクは `blocked{要対応}`（推測で選ばない）。
- テンプレート1の成果物要約（今はメモリだけ）もスロットに保存する。再起動後のプロンプトに前の報告と指摘が入るようになる（改善。状態遷移は変わらない）。

### 7.6 イベントと保存形式【設計】

`SCHEMA_VERSION` を 2 にする。v1 の行はそのまま読める（`parse_events` は `1..=SCHEMA_VERSION` を受ける）。新しい種類（すべて `apply` で検査）:

```rust
// OrgEventKind に追加
/// 開始時のグラフ（写しの指紋）。これがないワークフローは組込みの `standard` で動く。
GraphBound { graph_id: String, fingerprint: String, roles_fingerprint: String },
/// 辺を通って活動（タスク）を作った。PlanTask に node と起こした辺を足したもの。
ActivationAdded { task: PlanTask, node: NodeId, via: Option<EdgeId> },
/// スロットの新しい版（値は slots\ のファイル）。
SlotWritten { task: TaskId, slot: SlotId, version: u32, bytes: u32 },
/// 選んだ辺（記録。復元時に辺の選択をやり直さない）。
EdgeTaken { from: TaskId, edge: EdgeId, to: Option<TaskId> },
FeedbackCounted { edge: EdgeId },
FeedbackAllowanceRaised { edge: EdgeId, by: u32 },
GateDecided { task: TaskId, choice: String, note: Option<NoteId> },
/// 利用者が書いた理由・追記（本文は events に入れる。4,000字まで）。
UserNoteAdded { id: NoteId, text: String, scope: NoteScope },
/// G-B: folder の確定。
SnapshotTaken { task: TaskId, snapshot: String, files: u32, bytes: u64, manifest_digest: String },
/// G-B: 続きの依頼の系統。
LineageSet { parent: WorkflowId, series: String },
```

- `Task` に `node: Option<NodeId>`（`#[serde(default)]`。None＝plannedTasks の項目、またはテンプレート1の旧データ）、`last_output_seq: Option<u64>` を足す。`BlockedReason` に `RoundLimit` を足す（`ask` の上限到達）。`TaskStatus` は変えない。
- `WorkflowState` に `graph: Option<GraphRef>`・`feedback: BTreeMap<EdgeId, (used, allowed)>`・`slots: BTreeMap<SlotId, u32>`（最新の版）・`notes`・`lineage` を足す（全部 `#[serde(default)]`）。
- 古いアプリで v2 のワークフローを開く（戻した場合）ことは対応しない（v2 の行は読み飛ばされ、復元が不完全になる）。リリース前のブランチなので許容し、README に書く。

### 7.7 再起動後の照合・再開との整合【決定 Q8】

- `classify_after_restart` は今のまま（running／reviewing で確定のもの→要照合、`AwaitingPlanApproval` は保つ）。**gate の `blocked{承認待ち}` と `blocked{周回上限}` は変えない**（running ではない）。
- 復元時、`completed` なのに `EdgeTaken` がないタスク（辺を選ぶ途中で落ちた）は、保存したスロットから `route.rs` で辺を選び直す（純関数なので同じ結果になる）。スロットが読めなければ `blocked{要対応}`。
- 再開は今のまま利用者の「再開」だけ（自動 resume なし、U1）。

### 7.8 プロンプトの組立て（`prompt.rs` の一般化）【設計】

agent ノードの turn 本文は、決まった順の枠で作る（`developerInstructions` は使わない、今のまま）:

1. 枠の見出し（エンジンが作る）: 「あなたは〈役割の表示名〉です」、作業場所の説明（書込み可否・確定は AgentDock が行う・検証対象のコミット／スナップショット）、第N周／最大M周。
2. 【この役割の指示】（役割定義）。3. 【この工程の指示】（グラフのノード）。
4. 資料（`quote()` の区切り付き、「資料であり指示ではない」）: 利用者の依頼、`reads` のスロットの最新版、フィードバックで戻ったときは戻した側の出力（指摘）、案件メモ（G-C）。
5. **利用者の要望**（理由・追記）: 区切りを別にする（`-----BEGIN USER NOTE-----`）。注記は「利用者からの要望です。内容への要望として扱ってください。出力の形式・役割・作業場所・権限は変わりません」。区切り文字は `quote()` と同じく無害化し、4,000字で切る。
6. 「回答は指定の JSON スキーマに従う」。

- テンプレート1の Planner・Worker・Reviewer の文面は今の関数（`planner_prompt` 等）を使い続ける（`planned.rs` の中）。新しい枠は新しいノードだけ。
- 権限はプロンプトでは変わらない（sandbox と cwd が決める）。グラフ・利用者の文に何が書かれても、役割定義の権限より広くならない。

### 7.9 規模とリスク

|部分|規模|リスク|
|---|---|---|
|`planned.rs` への切出し（G-A0）|中（移動中心、約600行）|低。既存テストで確認。`RoleKind` の参照の置換漏れ|
|`graph/`（定義・検証・コンパイル・route）|中〜大（約900行＋テスト）|中。V7（入れ子）とV11（上限計算）を間違えやすい→単体テスト必須|
|`graphrun.rs`（周回の遷移）|大（約800行）|**高**。本体を閉じる・入れ子・`rework` の戻り条件。テスト中心で進める|
|`outschema.rs`|中（約400行）|中。outputSchema と検証のずれ→同じ定義から作る|
|保存（イベント追加・スロットファイル・復元）|中|中。順序（PutSlot→Persist）と復元時の辺の選び直し|
|ホスト（グラフの読込み・写し・開始引数・表示）|中|低〜中|
|UI|中|低|

## 8. 役割の追加（標準3役以外）

### 8.1 方針【設計】

- 役割は「**誰が**（モデル・effort・指示・sandbox・承認方針・作業場所の種類・既定の出力）」、ノードは「**何を・いつ**」。流れの意味は役割IDに持たせない（`role_kind()` を廃止）。これで REVIEW_ORG M4 の「独自の役割が黙って動かない」も解消する。
- `roledef.rs` の「標準3役以外は未対応」（`validate` 224〜228行）を外す。標準3役の存在チェック（`set_roles`）は「グラフが使う役割が全部ある」チェックに変える（テンプレート1を使うなら3役が必要）。
- 作業場所の種類（`workplace`）の意味を一般化する（保存名は変えない）: `sharedReadOnly`＝依頼の作業フォルダを読むだけ、`detachedAtCommit`＝**確定物（コミット／スナップショット）に固定した場所**を読むだけ、`ownWorktree`＝**専用の作業場所**（git の worktree／folder の作業フォルダ）。画面の表示名もこの意味にする。
- 役割とノードの組合せは V8 で検査する（書込み役は capture 必須、`detachedAtCommit` の役割は `target` 必須、`sharedReadOnly` は readOnly のみ）。

### 8.2 権限の安全【決定 §3.15.3】

- 「既定（workspace-write＋on-request）より広い定義は無効のまま理由を示し、利用者が明示的に有効化したときだけ使う」は変えない。既定は役割IDごとではなく、`workplace` の種類ごとに決める（今の `baseline()` を `workplace` で引く形に直す）。
- 承認方針 `untrusted`（狭める側）を実際に効かせる。schema 0.162.1 の `AskForApproval` に `"untrusted"` がある（`ts-stable/v2/AskForApproval.ts` で確認済み）。役割チャット作成時の承認方針に写す（今は sandbox からの固定値、`ports.rs`）。`never` は広げる側なので従来どおり無効＋確認。
- `extraWritableDirs`・`allowNetwork` は今回も未対応のまま（読込みで問題として出す）。
- グラフ定義には権限の項目がない（§6.3）。グラフの `instructions` は利用者の書いた指示として扱うが、権限は変えられない。
- 出力スキーマの宣言: 役割の `outputSchema` は「既定の出力」になり、ノードの `output` のスロットのスキーマが優先する（役割定義の schemaVersion は 1 のまま、`outputSchema` を任意に変える）。

## 9. 成果物の置き場

### 9.1 git（MVP。今の方式の一般化）【決定 Q7】

- 書込みノードの worktree は、前向きの親の中で最も近い書込みノードの最新コミットから作る（`WorktreeBase::FromCommit(sha)`＝新しいブランチ `agentdock/org/<wf>/<task>` をそのコミットから。新規）。親に書込みノードがなければ HEAD（今のまま）。plannedTasks の項目は今のまま HEAD。
- 読取り役は今の detached worktree（対象のコミットに固定、再レビューで `RefreshDetached`）。
- 自動マージしない。完了時はブランチ・コミット・差分を示す（今のまま）。

### 9.2 folder（G-B）【設計】

- 書込みノードの作業場所: `org\<wf>\work\<task>\`（AgentDock 専用）。前向きの親に確定物があれば、その最新スナップショットをコピーして始める。
- 利用者の作業フォルダ（入力）は**読むだけ**の資料として場所を渡す（プロンプトにパスを書く）。利用者のフォルダへは書かない。入力ファイルを選んだ場合は、添付と同じく AgentDock の領域へコピーして渡す（元は動かさない）。
- 確定（`Snapshot`）: turn 完了後、ホストが作業フォルダを `org\<wf>\snapshots\<n>\` にコピーし、`manifest.json`（相対パス・サイズ・sha256）を作る。上限: 2,000ファイル・200MB（超えたら `blocked{要対応}`）。空き容量の確認は既存の `precheck_space`。
- 検証対象の固定: 読取り役の cwd はそのスナップショット。送る直前にマニフェストを照合し（`VerifySnapshot`）、合わなければ送らない（`blocked{要対応}`）。
- 成果物の取り出し: ワークフロー画面の「保存…」（既存の成果物の保存・開くの規則）。利用者のフォルダへ自動で書き出さない（Q7 と同じ考え）。
- 削除: スナップショットは自動で消さない。使用量の「組織実行」に含める。
- trust の追記: 作業フォルダが新しい cwd になるので、Codex が `config.toml` に trust を追記し得る。J2-A のとおり「Codex 本体の追記」として区別表示する。
- 実機確認（G-B）: workspace-write の役割が cwd の外（利用者の入力フォルダ）を読めるか、書込みが cwd の外で拒否されるか。

### 9.3 将来案

利用者の Git なしフォルダへの「取込み」（差分を見て選んで書き戻す。変更を戻す機能のハッシュ照合を流用）、Office 文書の差分表示、ライブラリへの登録（G-C）。

## 10. 口を出す入口（グラフのイベントとして）

すべてワークフロー画面からの利用者操作（`UserConfirmed`、確認ダイアログ付き）。エンジンへの入力になり、送るのは従来どおり ready のタスクだけ。役割チャットへの手入力は不可のまま（Q9）。

|#|入口|入力 → 記録|効果|段階|
|---|---|---|---|---|
|①a|計画承認カードの「差し戻して再計画」に理由|`UserReplan { note }` → `UserNoteAdded`|再計画の Planner に利用者の要望として渡す|G-A|
|①b|gate の「直してほしい（理由）」|`UserGate { task, choice, note }` → `GateDecided`|選択肢の辺（フィードバックなら回数を数える）。理由は戻り先の次の turn に渡す|G-A|
|①c|周回上限カード（`blocked{周回上限}`）の「理由を書いてもう1周」|`UserExtraRound { task, note }` → `FeedbackAllowanceRaised`＋`UserNoteAdded`|その辺を1回だけ増やして戻り先を ready に|G-A|
|②|実行中の「追記…」|`UserAmend { text }` → 承認カード（影響するタスクを列挙）→ 承認で `UserNoteAdded{scope: 以後すべて}`|**これから送る turn** に要望として入る。実行中の turn には何もしない（追加指示・steer をしない）。取り消せる（以後に入れない）|G-B|
|③|完了したワークフローの「続きを依頼…」|新しいワークフローを `Start { lineage: 前のワークフロー, graph, request }`|同じ系統。前の最終スロットを資料に、git は前の最終コミットから、folder は前の最終スナップショットのコピーから始める。前のワークフローは変えない|G-B|

- ②で「計画を作り直す（未着手分だけ）」まで行う案は §15 U3。
- Q9 との整合: 役割チャットは今のまま打ち込めない。介入の入口が「手動に切替」だけ、という §3.15.2 の文言を「手動に切替と、ワークフロー画面の理由付き差し戻し・追記・続き」に改める（§13）。

## 11. UI

### 11.1 開始ダイアログ（G-A）

- テンプレートの一覧（`org\graphs\`）: 名前・説明・置き場（git／folder）・使う役割。誤りのある定義は選べず、理由を並べる。「定義ファイルを開く」「再読み込み」。
- 入力: 依頼、作業フォルダ（git のテンプレートはリポジトリとコミットの事前確認＝今のまま）、並列数（1〜4）、turn 上限（既定30、テンプレートの提案値を並べて表示）。
- 見積り: 「通常 N turn／最大 M turn（形式修正込み）。1 turn あたり約1.7万トークンの土台があるため、最大でおよそ M×1.7万トークン（目安）」。役割ごとのモデル・effort。有効化されていない役割があれば開始できない理由として出す。

### 11.2 ワークフロー画面（G-A）

- 上部に今の帯（依頼・状態・並列・turn 残り）＋テンプレート名。
- **グラフ表示（読み取り専用）**: MVP は図ではなく縦の一覧。前向きの順にノードを並べ、各行に 状態（アイコン＋文字）・役割・第N周／最大M周・最後の判定。辺は文字で「→ verify」「↺ build（rework のとき、最大3周、残り1）」。通っていない分岐は薄く「未実行」。ノードを選ぶと役割チャットを開く（閲覧だけ、今のまま）。
- 周回の表示: フィードバックの辺ごとに履歴「1周目: rework（指摘3件）／2周目: pass」。指摘の本文はスロットから開ける。
- カード: 計画承認（今）、gate、周回上限、追記の承認（G-B）。
- plannedTasks（テンプレート1）はノードの下に今のタスク表をそのまま入れる。

### 11.3 定義エラーの表示

- 設定の「組織実行」に「グラフ定義」節（役割定義と同じ形）: ファイルごとに 有効／誤り（V1〜V12 の文言、ノードID・辺を示す）。開く・再読み込み。
- 作らないもの: グラフエディタ、図の描画（G-E）。

### 11.4 IPC と型

- 追加: `org_graphs_view`、`org_reload_graphs`、`org_graph_file_path`、`org_start` の引数に `graphId`・`maxTurns`、`org_gate_decide`、`org_extra_round`、`org_replan` に `note`（G-A）。`org_amend`・`org_amend_decide`・`org_continue`・`org_save_artifact`（G-B）。状態を変えるものは全部 `UserConfirmed`。
- 表示用: `OrgWorkflowView` に `graph: Option<OrgGraphView>`（`nodes[{id,label,kind,roleId,status,confirm,round,maxRound,lastVerdict}]`・`edges[{from,to,kind,label,used,allowed}]`・`rounds[]`）。ts-rs で生成（`src/ipc/gen/`）。Codex 固有の型を出さない。

## 12. 安全（既存の合意を壊さない確認表）

|合意|この設計での扱い|
|---|---|
|`UserConfirmed`／`OrgGrant`|開始・再開は今のまま grant を作る。gate・周回追加・追記・続きの操作も `UserConfirmed`。grant は増やさない|
|監視経路から送れない|新しい入力は全部 `OrgInput`。送信は `send_turn`→`Send`→`org_send`（`DispatchPermit`）だけ。窓口チャット（G-D）は利用者の送信で、組織の送信経路と別|
|承認代行なし|gate・計画承認はエンジン内の判断で、Codex の承認要求には今のまま応答しない。gate を自動で決める設定は作らない|
|受理不明で再送しない|変わらない。周回は判定に基づく新しい指示。確認状態≠確定のタスクは辺を選ばない|
|終了・通信断を done/failed にしない|変わらない。本体を閉じる（completed）は live の完了＋スキーマ検証＋確定の後だけ。`failed{上限到達}` は `onExhausted: fail` の判定の結果だけ|
|自動 resume なし、自動マージなし|変わらない。folder も利用者のフォルダへ自動で書かない|
|利用枠の上限|静的上限（V11）＋turn 上限（必ず止まる）＋開始時の見積り。もう1周は利用者だけ|
|利用者テキストの取り込み|区切りを分け、無害化し、長さを切り、「権限・形式は変わらない」と明記。権限は sandbox と cwd が決める|
|フィードバックの暴走|V3〜V7・V11、回数はワークフロー全体で戻さない、条件なしの戻りは禁止|
|`~/.codex` を書かない|変わらない。案件メモ・ライブラリも AgentDock の領域だけ|
|自動削除しない|スロット・スナップショット・案件は自動で消さない|

---

## 13. 確定事項・正本 §3.15 への影響

|項目|変更|
|---|---|
|Q1・Q3・Q4・Q6・Q7・Q8・Q10・J1|変更なし（Q3 の自動承認は plannedTasks の計画だけに効く。グラフの開始は利用者の操作そのもの）|
|Q2|拡張: 役割は標準3役以外も動く（流れはグラフが決める）。標準役割に `verifier`・`writer`（G-A）、`researcher`（G-B）を足す|
|Q5|一般化: 差し戻し上限は「フィードバックの辺ごとの最大周回数（1〜5、定義で必須）」。テンプレート1は今のまま2回・自動再計画1回|
|Q9|文言の追加: 役割チャットへの手入力は不可のまま。介入はワークフロー画面の操作（手動に切替、理由付き差し戻し、もう1周、追記の承認、続きの依頼）|
|Q11|変更なし（turn 上限は全ノードに効く）。開始ダイアログで turn 上限を選べ、見積りを示す|
|J2|追記: folder の作業フォルダでも trust の追記が起こり得る（区別表示のまま）|
|§3.15.1|「依頼文と作業フォルダ（Gitリポジトリ）」→「テンプレート（グラフ定義）と依頼文と作業フォルダ（git のテンプレートは Git リポジトリ）」。形式検証にグラフ定義の検証を追加|
|§3.15.2|介入の入口の追加（上の Q9）|
|§3.15.3|標準以外の役割、出力スキーマの宣言、承認方針 untrusted の反映|
|§3.15.4|folder の置き場（専用の作業フォルダ＋スナップショット、利用者のフォルダへは書かない）（G-B）|
|§3.15.5|周回（フィードバックの辺）の上限、静的上限、開始時の見積り|
|§3.15.7|完了の判定に「周回の本体のノードは周回が終わったときに完了」を追加（他の条件は同じ）|
|§3.15.8|グラフ表示（一覧形式）・周回の表示・gate／周回上限カード・グラフ定義の誤りの表示|
|新設（G-C 以降）|§3.16 案件（ゴール・案件メモ・ライブラリ・使用量）、§3.17 窓口チャット・継続チャット。§3.6 のプロジェクト表示は名前を「Codexのプロジェクト」に（再開時）|

正本の版更新は司令塔がユーザー承認後に行う（G-A 着手前に §3.15 分、G-C 着手前に §3.16・3.17）。

## 14. 実装タスク

### 14.1 G-A（MVP-1）

|#|内容|担当|依存|受入条件|確認コマンド|
|---|---|---|---|---|---|
|G-A0|`planned.rs` への切出し（Planner→タスク表→Worker⇄Reviewer の処理を移すだけ）。`role_kind()` を「テンプレート1の組込みグラフでノードが plannedTasks か planner か」に置換。動作は変えない|implementer|—|`org-engine` の既存テストが**期待値を変えずに**全部通る。差分にテストの期待値の変更がない|`cargo test --manifest-path app/org-engine/Cargo.toml`|
|G-A1|`graph/def.rs`・`validate.rs`・`CompiledGraph`・`route.rs`、`outschema.rs`（組込み6種）、組込みテンプレート `standard`・`quality-loop`|implementer|—（G-A0 と並行可）|V1〜V12 各1件以上の不合格テスト、テンプレート2つが合格、V11 の上限計算（入れ子・独立の周回）、route の宣言順・既定、outschema の送る形と検証の一致（同じ定義から作った JSON を検証に通す）|同上|
|G-A2|`graphrun.rs`: agent・router・gate、§7.3 の遷移、周回、`onExhausted`、`FeedbackAllowanceRaised`、`UserNoteAdded`、§7.6 のイベント、`SCHEMA_VERSION`=2、スロット（`PutSlot`）、復元時の辺の選び直し|implementer|G-A0・G-A1|テスト: 3周で pass→本体が completed／上限で ask→`blocked{周回上限}`／fail→`failed{上限到達}`／入れ子で外側の戻りが内側の回数を戻さない／受理不明・要照合のタスクで辺を選ばない／`Freshness` で completed にならない／`PutSlot` が `Persist(SlotWritten)` より前、`Send` の前に `Persist(Dispatched)`／旧 v1 のイベントだけのワークフローが `standard` で復元される／`EdgeTaken` のない completed から同じ辺を選び直す／gate を入力なしで決めない|同上|
|G-A3|役割: `roledef.rs` の標準3役限定を外す、`workplace` 基準の既定、`untrusted` の反映（`ports.rs`）、標準役割 `verifier`・`writer` の初回作成（上書きしない）|implementer|G-A1|既存の役割ファイルを上書きしない／広げる定義が無効のまま理由付き／`untrusted` の役割で作ったチャットの承認方針が untrusted（ホストのテスト）|`cargo test --manifest-path app/org-engine/Cargo.toml`、`cargo test --manifest-path app/src-tauri/Cargo.toml --lib host::org`|
|G-A4|ホスト: `host/org/graphs.rs`（読込み・誤り一覧・初回作成）、開始時の写し（`graph.json`・`roles.json`）、`OrgStartArgs` に graph・maxTurns、スロットファイルの読み書き、`WorktreeBase::FromCommit`（gitops 追加。所有の確認は今の規則）|implementer|G-A2・G-A3|写しで復元し、定義ファイルを変えても動いているワークフローが変わらない／スロットが読めないとき推測で辺を選ばない／`rg -n "from_user_command|OrgGrant::issue" app/src-tauri/src/host/org` が今と同じ箇所だけ|`cargo check --manifest-path app/src-tauri/Cargo.toml`、`cargo test --manifest-path app/src-tauri/Cargo.toml --lib host::org`、`--lib gitops`|
|G-A5|IPC・UI: 開始ダイアログ（テンプレート選択・見積り）、グラフ表示（一覧形式）、周回の表示、gate・周回上限カード、再計画の理由、設定の「グラフ定義」節|implementer|G-A4|選ぶ・開くだけで何も送らない／状態は色だけにしない／未取得は「未取得」／確認ダイアログを通らない状態変更がない|`cargo check …`、`cd app && npx tsc --noEmit`|
|R-G|G-A0・G-A2・G-A4 の差分レビュー→`app/REVIEW_ORG_GRAPH.md`|designer|G-A5|—|—|
|G-A6|R-G の指摘の修正|implementer|R-G|検証担当の合格|—|
|S-G|NSIS 作り直しと実機手順書|司令塔／ユーザー|G-A6|下記の実機項目に期待結果がある|—|

順序: (G-A0 ∥ G-A1) → (G-A2 ∥ G-A3) → G-A4 → G-A5 → R-G → G-A6 → S-G。**タスク数 9**（実装6・レビュー1・修正1・実機1）。

実機確認（ユーザー、S-G）:
1. テンプレート1で小さなリポジトリを1件完走し、今と同じ流れ（計画→実装→レビュー→差し戻し→完了）になる。
2. 品質向上周回: わざと不足のある依頼で、verify が rework を返し build が同じ worktree で直し、2周目以降で pass になる。表示が「第2周／最大3周」と判定の履歴になる。
3. 上限: maxRounds を1にした写しのテンプレートで、上限到達カードが出る。「理由を書いてもう1周」で1回だけ増える。
4. 途中で完全終了→再起動→照合→再開。周回の回数とスロット（前の報告・指摘）が残り、プロンプトに入る。
5. turn 上限を小さく（例 6）して、周回の途中で `blocked{上限}` で止まる。
6. 誤りのある定義ファイル（例: 条件のないフィードバック辺）を置き、開始ダイアログと設定で理由が出て選べない。

### 14.2 G-B（MVP-2、概要）

|#|内容|依存|
|---|---|---|
|G-B1|folder の置き場: 作業フォルダ・`Snapshot`・`VerifySnapshot`・上限・使用量の行（ホスト、gitops とは別の `orgfs` 部品）|G-A|
|G-B2|業務テンプレート3種、標準役割 `researcher`、利用者定義の出力スキーマ（`org\schemas\`）|G-B1|
|G-B3|入口②（追記→承認カード→以後の turn へ）、入口③（続きの依頼＝系統）、成果物の保存|G-A|
|G-B4|UI（folder の成果物の表示・保存、追記カード、続きの依頼、系統のまとまり）|G-B1〜3|
|R-GB・修正・S-GB|レビュー（folder の書込み範囲、追記の取り込み、続きの土台）と実機（議事録を1件、入力フォルダの読取りと書込みの拒否、スナップショットの照合）|G-B4|

タスク数 7 前後。G-C（案件・メモ・ライブラリ・使用量）は 6〜8、G-D（窓口チャット・提案ブロック・引き継ぎ）は 6〜8 の見込み（詳細設計は各段階の着手前に別書）。

---

## 15. ユーザー判断が要る点（選択式、推奨が先頭）

**U1. 新しい「プロジェクト」相当の呼び名**（§3）
- (a) 「**案件**」。停止中の既存機能は戻すとき「Codexのプロジェクト」と呼ぶ（推奨。名前と所属の推定が混ざらない）
- (b) 「プロジェクト」。既存のものを「Codexのフォルダ分類」等に改名する
- (c) 「ワークスペース」（sandbox の workspace-write と紛らわしい）

**U2. MVP の範囲**（§14）
- (a) G-A は git だけ（品質向上周回＋テンプレート1の回帰なし）。folder・業務テンプレートは G-B（推奨。リスクの高い周回の遷移を先に固める）
- (b) G-A に folder と議事録テンプレートも入れる（業務で早く使えるが、MVP が約1.5倍）

**U3. 実行中の追記（入口②）の効き方**（§10）
- (a) 承認カードを通して「これから送る turn に要望として追記」だけ（推奨。AI の再計画を挟まない）
- (b) (a)に加え、テンプレート1では「未着手のタスクを再計画」も選べる（計画承認カードを必ず通す）

**U4. 完了後の続きの依頼（入口③）**（§10）
- (a) 同じ系統の**新しいワークフロー**として始める（推奨。終わったワークフローの記録を変えない。G-C で系統を案件へ移せる）
- (b) 終わったワークフローにノードを足して続ける

**U5. G-C（案件）と継続チャットの順序**（§1・§5.2）
- (a) G-A→G-B→G-C（案件）→G-D（窓口チャット＋継続チャット）（推奨。メモとライブラリの土台が先にあると、引き継ぎ資料が作れる）
- (b) 継続チャット（窓口チャットだけの案件、引き継ぎ機能のみ）を G-B の直後に先に出し、案件メモ・ライブラリは後で足す

**U6. 継続チャットの引継ぎ資料**（§5.2）
- (a) 決定的に組み立てる（採用済みメモ・ライブラリ一覧・直近数往復の原文）。追加のモデル呼び出しなし（推奨）
- (b) 引き継ぐ前に古い thread へ「要約して」を1 turn 送る（利用者の送信の中で行い、turn として数える。内容は良くなるが利用枠を使う）

**U7. 案件メモの採用**（§4.2）
- (a) 役割の提案は全部、利用者がまとめて採用する（推奨）
- (b) スキーマ検証に通った成果物から来た「事実（fact）」だけ自動採用、決定・予定は利用者が採用

その他（形式は JSON、周回の回数はワークフロー全体で数える、1ノード1タスク1チャット、スロットの保存、出力スキーマは自前の部分集合）は設計裁量で決めた。
