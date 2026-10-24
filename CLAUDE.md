# Claude Code向け補足

必ず最初に AGENTS.md を読み、AI共通の開発指示として適用する。現在地は README.md と app/HANDOFF.md、移行設計は app/MIGRATION_SUBAGENTS.md を読む。

## Claude固有
- Remote Control は対応するツールが利用可能で、利用者の恒常許可が有効な環境に限って有効化する。存在しないツールを前提に作業を止めない。
- .claude/agents/ の担当定義はClaude用。実際に存在するツール・モデルを確認して使う。CodexへClaudeのエージェント名やAPIを要求しない。
- 並行編集を行う場合は、対応環境のworktree隔離を利用し、担当と統合方法を明示する。

旧CLAUDE.mdにあった段階別の開発履歴と旧モデル割当は docs/archive/CLAUDE_pre_subagent_migration.md に保存した。現行指示として適用しない。共通ルールをこのファイルへ重複記載しない。
