# DESIGN_ORG_R4 — 調査タスク（kind）とReviewerの合否の根拠（checks）

版 0.1（2026-10-23）。対象: 実機確認 R3（調査だけのタスクが成立しない）・R4（テスト失敗でも合格）。計画経路（standard）が対象。グラフ経路は §2.6 に方針だけを書く。

合意（変えない）: 承認要求に代行応答しない／自動再送しない／通信断を done・failed にしない／Q7 利用者の作業ツリーへ自動マージしない／既存の `workflow.json`・`events.jsonl` を読めること。

---

## 1. タスクの種類 `kind`（R3）

### 1.1 型と互換

```rust
// org-engine/src/types.rs
/// What a plannedTasks worker task produces. Absent in older data = Change.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum TaskKind {
    /// Changes files; the result is a commit (existing behaviour).
    #[default]
    Change,
    /// Reads only; the result is the structured report (no commit, no worktree branch).
    Investigate,
}
impl TaskKind { pub fn is_change(&self) -> bool { *self == TaskKind::Change } }

// PlanTask と Task の両方に追加
#[serde(default, skip_serializing_if = "TaskKind::is_change")]
pub kind: TaskKind,
```

- 旧データ（`kind` なし）は `Change` として読む。`Change` は書き出さないので、従来の行はバイト単位で変わらない。旧版バイナリは未知フィールドを無視する（`PlanTask` は `deny_unknown_fields` でない）ため、旧版で開くと調査タスクは Change 扱いになる（下位互換は保証しない、と明記するだけ）。
- reviewer・planner タスクは常に `Change`（意味を持たない）。
- `PlanTask` の構造体リテラル（`planned.rs:109`、`intervene.rs:161`、`notes.rs:71`、各テスト）は `kind: TaskKind::Change` を足す。

### 1.2 計画スキーマ・解析

- `planner_plan_schema`（schemas.rs:26）: 各タスクに `"kind": {"enum": ["change","investigate"]}` を追加し `required` に入れる（既存スキーマは全項目 required の方針。省略可能にはしない）。
- `RawTask` に `#[serde(default)] kind: Option<String>`（欠落は change。スキーマで必須だが、解析は寛容に）。
- `parse_plan` の追加規則（違反は形式エラー＝既存の1回の形式修正）:
  1. `kind` は worker のときだけ `investigate` を許す。reviewer に `investigate` は不可。
  2. reviewer の依存先は **change の** worker 1件（調査はレビューしない。品質は報告を使う後続タスクとその reviewer で担保）。
  3. investigate に `userTreeWrite`・`pushMergeTag`・`deletionPrimary` の宣言は不可（読むだけと矛盾）。
- ポリシー（`evaluate_plan`）は変えない。

### 1.3 Planner への指示（prompt.rs:29-38 に追記）

- 「調べるだけで済む作業（コード・文書・履歴の調査、比較、要約）は kind=investigate の worker にする。investigate はファイルを変更せず、結果を報告に書く。」
- 「ファイルを変更する作業は kind=change。調査結果を使って文書・コードを作る作業は、調査タスクに deps で依存する change タスクにする。」
- 「reviewer は change タスクだけを検証する。investigate には付けない。」
- 「実装とそのテストは同じ change タスクに含める（後続タスクの作業場所には先行タスクの変更が含まれない場合がある。§3）。」

### 1.4 状態遷移（investigate）

```
Ready → (worktree: DetachedAt("HEAD")) → Running → 出力受理
   → PutReport（必須書込み） → Persist(ReportRecorded) → Completed
```

- `WantCommit` を通らない。`CommitEmpty`・`NothingWritten` は起きない。
- reviewer を付けない（§1.2）ので `Reviewing` にはならない。
- change で変更がない場合は**従来どおり** `CommitEmpty` → `Blocked(NeedsAttention)`（変更しない）。文言だけ「調査だけのタスクなら、計画の作り直しで種類を『調査』にしてください」を足す。
- 作業場所: `WorktreeBase::DetachedAt("HEAD")`（ホストは "HEAD" を解決して `base_commit` に記録済みの経路＝worktree/org.rs:62。ブランチを作らない＝`DetachedReadOnly`）。利用者の作業ツリーを直接読ませないのは、change タスクと同じ HEAD の内容を見せるため。
- sandbox: 役割設定に関わらず `ReadOnly` に**狭める**（`begin_chat` で kind を見る。広げる方向には動かさない）。承認方針は役割の設定のまま（承認要求は利用者に出る。エンジンは応答しない）。
- 同時実行: 書込み上限（Worker同時2）には数えず、全体上限（3）には数える（scheduler.rs:91・109・116 の判定を「役割が writes かつ kind=change」に）。
- 再送・形式修正・中断・照合は既存の経路のまま（`retry_prompt` の NothingWritten 以外の文言はそのまま使える）。利用者の「差し戻し」は不可（Reviewing にならないので既存の `send_back_check` が拒否）。

### 1.5 調査報告の形（新スキーマ `investigation_report_schema`）

全項目 required・`additionalProperties:false`。

```json
{ "summary": "string（結論の要約）",
  "findings": [ { "point": "string（結論）", "basis": "string（根拠）", "refs": ["string（ファイル:行、コミット等）"] } ],
  "references": ["string（調べた対象）"],
  "openQuestions": ["string（分からなかったこと）"] }
```

```rust
// org-engine/src/schemas.rs
pub struct InvestigationFinding { pub point: String, pub basis: String, pub refs: Vec<String> }
pub struct InvestigationReport { pub summary: String, pub findings: Vec<InvestigationFinding>,
                                 pub references: Vec<String>, pub open_questions: Vec<String> }
pub fn parse_investigation_report(text: &str) -> Result<InvestigationReport, String>; // summary 空・64KB超は形式エラー
pub fn render_investigation(r: &InvestigationReport, cap_chars: usize) -> String;   // 後続へ渡す文面（上限超は「以下省略」）
```

- `Output` に `Investigation(InvestigationReport)` を追加。`parse_output` は worker の kind で `parse_worker_report` と分ける。`schema_for` も kind で分ける。
- Worker プロンプトは別関数 `investigate_prompt`（`WORKER_SCOPE_NOTE` を入れない）: 「あなたは調査担当です。ファイルは変更しません。調べた結果を、根拠と参照箇所（ファイル:行、コミット）付きで報告に書いてください。推測は推測と書き、分からないことは openQuestions に書く。」

### 1.6 後続への受け渡しと保存（artifacts をメモリのみにしない）

- 新しい動作 `OrgAction::PutReport { task, attempt, value }`: `PutSlot` と同じく**書けなければ以後の動作を止める**（保存先は既存の `outputs\<task>.<attempt>.json`、`put_output` を使う）。続けて `Persist(ReportRecorded { task, attempt, bytes })`。
- 新イベント `OrgEventKind::ReportRecorded { task: TaskId, attempt: u32, bytes: u32 }`。適用時は状態を変えない（`Task.report_attempt: Option<u32>` を `#[serde(default)]` で記録するだけ）。
- 復元: ホスト（host/org/mod.rs:640 付近）が `read_slots` と同様に `read_reports(store, &state)` で
  - investigate: `report_attempt` の版を読む（読めなければ「未取得」で一覧に返す）、
  - change worker: 最新試行の `outputs` を**読めれば**読む（従来の「未取得」を減らすだけ。必須ではない）。
  を `restored_with` に渡す（引数を `RestoredExtras { slots, reports }` にまとめる）。`artifacts: HashMap<TaskId, String>` は描画済み文面を持つ。
- 後続タスクの起動時、依存先の investigate の報告が無い（読めない）場合: 1回だけ `ReadFinalMessage`（読取りのみ。resume しない）で受理済み turn の最終回答から復元を試み、それも無理なら後続を `Blocked(NeedsAttention)`「調査 X の報告を読めません」。**「未取得」のまま送らない**（change 依存の従来の扱いとは変える。調査は報告が唯一の成果のため）。
- 後続プロンプト: `quote("先行タスク {id} の調査報告", render_investigation(r, 16_000))`。全依存の合計は 48,000 文字まで（超えた分は「全文は成果物パネル」と書く）。

### 1.7 成果物パネル

- `OrgArtifactsView` に `investigations: Vec<OrgInvestigationItem>` を追加（取り込み対象の `items` とは分ける。ファイルがないので取り込み・差分の対象外）。
  `OrgInvestigationItem { task, title, status, report: Known<OrgInvestigationView>, attempt }`。`report` は `outputs` → 履歴の最終回答 → 未取得 の順（既存 `org_report_of` と同じ方針）。
- 画面（OrgArtifacts.tsx）: 「調査報告」区画。要約、結論ごとの根拠・参照（参照は文字で表示し、リンクにしない）、未解決の点、本文のコピー。タスク表（OrgPane）では種類を「調査」バッジで示す。
- 既存の書込みタスク列挙（artifacts.rs:210）は `kind=Change` だけにする（調査の worktree 記録を書込み成果に出さない）。

### 1.8 代案（不採用）: NothingToCommit でも summary があれば完了

変更が1行で済むが、(a) 実装を頼んだのに何も書かなかった Worker まで「完了」になり R3 の逆の取り違え（未実装の見逃し）を生む、(b) 調査なのに書込み可の worktree・ブランチ・Worker 上限を消費する、(c) 報告の形が実装向け（changedFiles・tests）のままで根拠・参照が残らない、(d) 計画時点で「調査」と分からず UI・後続プロンプトで区別できない。よって採用しない。

---

## 2. Reviewer の合否の根拠（R4）

### 2.1 判定スキーマ（schemas.rs:74 を拡張）

全項目 required（スキーマ方針。`exitCode` は nullable が要るので項目にせず `summary` に書かせる）。

```json
{ "verdict": "pass|rework", "findings": ["string"],
  "checks": [ { "command": "string", "outcome": "passed|failed|notRun", "summary": "string" } ],
  "noCheckReason": "string（checks に passed が1件も無いときの理由。あれば空文字）" }
```

```rust
pub enum CheckOutcome { Passed, Failed, NotRun }
pub struct CheckResult { pub command: String, pub outcome: CheckOutcome, pub summary: String }
pub struct ReviewerVerdict {
    pub verdict: VerdictKind, pub findings: Vec<String>,
    /// None = older output without the field (display only; never produced by a live turn).
    #[serde(default)] pub checks: Option<Vec<CheckResult>>,
    #[serde(default)] pub no_check_reason: Option<String>,
}
```

- `parse_verdict`（エンジン用）: `checks`・`noCheckReason` の欠落は形式エラー。追加規則: `verdict=pass` で **passed が0件かつ noCheckReason が空** は形式エラー（既存の1回の形式修正 → それでも駄目なら従来どおり `InvalidArtifact`）。
- 表示用の解析（rules/org_artifacts の `parse_review_value`）は欠落を許し「記録なし」と表示（旧データ互換）。
- `checks` は最大20件、`command`・`summary` は各2,000文字で切る。

### 2.2 エンジンの判定規則（純粋関数）

```rust
// org-engine/src/schemas.rs
pub enum VerdictJudgement {
    Pass { unverified: bool },            // unverified = passed 0件（noCheckReason あり）
    Rework,
    /// The reviewer said pass but a check failed: the user decides (not an automatic send-back).
    PassContradicted { failed: Vec<String> },
}
pub fn judge_verdict(v: &ReviewerVerdict) -> VerdictJudgement;
```

| 判定 | checks | 結果 |
|---|---|---|
| pass | failed なし・passed≥1 | 従来どおり完了 |
| pass | passed 0・noCheckReason あり | 完了。`unverified` を出力に残し、成果物と取り込み確認に「テスト未実行の合格（理由…）」を注意として出す |
| pass | failed ≥1 | **PassContradicted**: 対象 worker を `Blocked(CheckFailed)`、reviewer は `Rework`（待機）。`FindingsRecorded`（失敗したコマンドと要約を指摘として）＋ `VerdictDemoted` を記録。差し戻し回数は数えない |
| rework | 任意 | 従来どおり |

- 自動の差し戻しにしない理由: R4 の失敗は依存先の変更が作業場所に無いこと（§3）が原因で、Worker に直させると範囲外の実装（他タスクの重複実装）を誘う。利用者が「もう一度直させる／このまま受け入れる／失敗として確定」を選ぶ（`ReworkLimit` と同じ3択。`send_back_check`・`accept_as_is_check` の対象に `CheckFailed` を加える）。
- 新 `BlockedReason::CheckFailed`（types.rs）。新イベント `VerdictDemoted { task: TaskId /*reviewer*/, target: TaskId, failed: Vec<String> }`（タイムライン用。状態は後続の `StatusChanged` が変える）。
- Worker の自己申告 `tests{ran,passed}` は判定に使わない（表示のみ）。reviewer プロンプトに「Worker の申告: 実行=…/成功=…（自己申告。必ず自分で実行して確かめる）」として渡す。申告が「不合格」で reviewer が pass・failed なしのときは成果物に「申告と判定が食い違います」と出すだけ。

### 2.3 Reviewer プロンプト（prompt.rs:126-130 を置換）

- 「テストがあれば実行する。失敗・エラー（収集エラー・ImportError を含む）が1件でもあれば pass にしない。」
- 「実行したコマンドごとに checks に command・outcome・結果の要約（終了コードや件数）を書く。実行していないコマンドを passed と書かない。」
- 「この sandbox は読み取り専用。書込みが要るためテストを実行できない場合は outcome=notRun とし理由を summary に書く（`-p no:cacheprovider` 等のキャッシュ無効化は使ってよい）。権限の変更・承認の要求で回避しない。」
- 「テストが存在しない・文書だけの変更などで実行するものが無いときは checks を空にし、noCheckReason に理由を書く。」

### 2.4 read-only での実行範囲と承認

- 読み取り専用 sandbox で pytest 等が動くことは実機確認済み。キャッシュ・ビルド出力を書くツール（cargo、npm build 等）は失敗しうる → `notRun`＋理由で表に出る（隠さない）。
- 権限は広げない。役割の承認方針が on-request なら承認要求は利用者へ出る（エンジンは応答しない）。承認を出さない設定（ReadOnly＋承認なし系）でも検証は行い、実行できなかった分は `notRun`。sandbox を書込み可にする案は不採用（検証役が対象を書き換えうる）。

### 2.5 互換

- 旧イベント・旧 `outputs`（checks なし）は表示で「記録なし」。エンジンは旧判定を読み直さない（`complete_verdict` は live の出力だけ）。
- 再起動: `CheckFailed` は `Blocked` の一種として既存の照合・復元の対象（照合で解除しない。利用者の選択待ち）。

### 2.6 グラフ（verdict3・quality-loop）— 今回は適用しない

組み込み `verdict3` を変えると、既存のグラフ定義と写しのスキーマ照合が変わる。推奨（別段階）: 新しい組み込みスキーマ `verdict3Checked`（verdict3＋checks＋noCheckReason）と同じ `judge_verdict` 規則を追加し、新規に書き出す `quality-loop` テンプレートだけがそれを使う（既存ファイルは create_new で上書きしない方針のまま）。pass＋failed は辺の選択前に `Blocked(CheckFailed)`。

---

## 3. 統合後の検証（今回は選択肢と推奨のみ）

R4 の根本は、依存先の change のコミットが後続の作業場所（HEAD 起点）に無いこと。reviewer は後続のコミットを detached で見るので、依存先を含めた状態は誰も検証していない。

| 案 | 内容 | 費用 | 効果 |
|---|---|---|---|
| A1 | change の依存が change 1件なら、後続 worker の worktree をそのコミット起点にする（`WorktreeBase::FromCommit`。グラフ 9.1 と同じ） | 小（planned.rs の launch だけ） | 直列の依存で R4 を解消。reviewer は自動で統合状態を検証。成果物パネルの「後の書込みが前を含む」判定が既にある |
| A2 | 依存が change 複数なら、所有 worktree 内で依存コミットを順にマージしてから作業開始。競合は `Blocked(NeedsAttention)` | 中（gitops に所有worktree内マージ、失敗の後始末） | 並列→統合の形でも解消 |
| B | reviewer の detached worktree だけで依存コミットを合成して検証 | 中 | 検証だけ。Worker は依存先なしで作るので不具合は防げない |
| C | 最後に全コミットの合成で検証するタスクを自動追加 | 大 | 取り込み前の総合確認。取り込みプレビューと重なる |

推奨: **A1 を次段階で実装**、A2 は要判断（所有worktree内のマージが Q7「自動マージしない」に当たるか）。それまでは §1.3 の Planner 指示（実装とテストを同じタスクに）と、依存先 change が2件以上のとき後続 worker プロンプトに「先行タスク X・Y の変更はこの作業場所に含まれていません」と明記する（黙って HEAD 起点にしない）。B・C は不採用（効果に対し費用が大きい）。

---

## 4. 実装タスク（実装担当1名が順に）

| # | 内容 | ファイル |
|---|---|---|
| K1 | `TaskKind`・`PlanTask/Task.kind`（serde 互換）、計画スキーマ・`RawTask`・`parse_plan` 規則、Planner 指示 | org-engine/src/types.rs, schemas.rs, prompt.rs, planned.rs, intervene.rs, notes.rs（リテラル） |
| K2 | 調査報告スキーマ・解析・描画、`Output::Investigation`、`investigate_prompt`、launch（DetachedAt("HEAD")・ReadOnly）、完了遷移、scheduler の書込み上限 | schemas.rs, prompt.rs, planned.rs, core.rs（begin_chat・Output）, scheduler.rs |
| K3 | `PutReport`（必須書込み）・`ReportRecorded`・`report_attempt`、復元 `RestoredExtras{slots,reports}`、報告欠落時の ReadFinalMessage 1回→Blocked | core.rs, persist.rs, host/org/driver.rs, host/org/mod.rs, host/org/graphs.rs（read_reports 同居可） |
| K4 | 判定スキーマ拡張・`judge_verdict`・`CheckFailed`・`VerdictDemoted`・send_back/accept_as_is の対象追加、Reviewer プロンプト（Worker 申告を含める） | schemas.rs, planned.rs（complete_verdict）, types.rs, persist.rs, intervene.rs, prompt.rs |
| K5 | ホスト表示: タイムライン行（ReportRecorded=Info「{task} が調査報告を出力」、VerdictDemoted=Warn「検証のコマンドが失敗したため合格を保留」）、成果物 `investigations`・checks 表示・未検証合格／申告食い違いの注意、取り込み確認の注意、状態ラベル `CheckFailed` | rules/org_timeline.rs, rules/org_artifacts.rs, host/org/artifacts.rs, host/org/view.rs, backend/org*.rs（ts-rs 再生成） |
| K6 | 画面: 「調査」バッジ、調査報告区画、checks 一覧、`CheckFailed` の3択ボタン（ReworkLimit と同じ部品） | src/ui/OrgPane.tsx, src/ui/OrgArtifacts.tsx |
| (次段階) A1 | 単一 change 依存の FromCommit 起点、複数依存時のプロンプト明記 | planned.rs, prompt.rs |

受入条件:
- 旧 `events.jsonl`／`workflow.json`（kind・checks なし）が復元でき、`Change` の行の再書出しが変わらない。
- investigate タスクは commit を試みず Completed になり、後続 change タスクのプロンプトに調査報告（根拠・参照付き）が入る。再起動後も入る（`outputs` から復元）。報告が読めなければ送らずに止まる。
- change で変更なしは従来どおり `CommitEmpty`→`NeedsAttention`。
- reviewer が pass でも failed の check があれば対象は `Blocked(CheckFailed)`、自動で再送・差し戻しされない。3択が使える。
- pass で passed 0件かつ理由空は形式修正1回→`InvalidArtifact`。
- `cargo check`（org-engine・src-tauri）、`tsc --noEmit` が通る。

単体テスト（純粋ロジックのみ）:
- `parse_plan`: kind 欠落=change、reviewer→investigate 依存の拒否、investigate の書込み効果の拒否。
- serde: kind なし PlanTask/Task の読込み、Change の書出しに kind が出ないこと。
- `parse_investigation_report`・`render_investigation`（上限で切る）。
- `parse_verdict`・`judge_verdict`: 表の4行＋checks 欠落の形式エラー。
- core（Sim）: investigate の Running→Completed で `Workspace::Commit` が出ないこと、`PutReport`→`ReportRecorded` の順、復元後の後続プロンプトに報告が入る、報告欠落で後続が Blocked。pass＋failed で `CheckFailed`・`ReworkCounted` なし・Send なし。
- scheduler: investigate が書込み上限に数えられない。
- org_timeline: 新イベント2行。

実機確認（ユーザー）:
1. R3 と同じ依頼（calc.py・README・Git履歴を調査して統合レポート）: 調査タスクが「調査」バッジで完了、成果物パネルに報告、統合タスクのプロンプト（チャット）に報告が入り、統合タスクが完了。
2. 調査中にアプリを終了→再起動→再開: 後続に報告が渡る。
3. 実装依頼でテストが失敗する状況（R4 の A2 相当）: reviewer の checks に失敗コマンドが出て、対象が「検証のコマンドが失敗」で止まり、3択が出る。自動で再送されない。
4. 文書だけの変更: reviewer が checks 空＋理由で合格、成果物に「テスト未実行の合格」の注意。
5. 承認なし設定の reviewer でテストが書込み不可で失敗する場合、notRun と理由が表示される。

---

## 5. 要判断

1. A2（所有 worktree 内で依存コミットをマージ）は Q7 に当たるか。推奨: 当たらない（利用者の作業ツリー・ブランチに触れない）として次段階で可。
2. pass＋failed を利用者判断（`CheckFailed`）にするか自動差し戻しにするか。推奨: 利用者判断（本書 §2.2）。
3. 調査タスクにレビューを付けない（§1.2）。付けたい要望が出たら、報告を資料として読む reviewer を別途設計する。
