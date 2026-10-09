//! 同等性の操作の確認状況の表（版別、`app/DESIGN_P3.md` §0.3）。
//!
//! `(Codex版, ParityOp) → Verification`。表にない組は `Unverified`。実測で成立を確認したものだけを
//! 版ごとに `Verified` として足す。実機確認の結果は司令塔がこの表を更新する。
//! 宣言（Support）とは別の軸で、`Unverified` の操作は「未確認」と表示し、結果は観測事実だけを書く。

use crate::backend::model::{ParityOp, Verification};

/// 0.160.0 で実測により成立を確認した操作（2026-10-10、実機確認 `app/LIVE_CHECK_P3.md` に基づく）。
///
/// 備考（確認の範囲）:
/// - ChangeList・RevertChanges: Git基準の控え（2026-10-10 実機確認○）。同時作業時の扱い（B-7）は実機未確認。
/// - ToolServers: 一覧・追加・削除。接続状態は通知でしか取得できない。
/// - Skills: 明示Skillの呼出し（`UserInput.skill`）。
///
/// 未確認のまま: CloudDelegation、Extensions（Plugins導入・削除は未実施）、Memory、Personality（非推奨）。
const VERIFIED_0_160_0: &[ParityOp] = &[
    ParityOp::ChangeList,
    ParityOp::RevertChanges,
    ParityOp::CodeReview,
    ParityOp::ReviewToNewChat,
    ParityOp::Fork,
    ParityOp::Compact,
    ParityOp::WorkMode,
    ParityOp::Goal,
    ParityOp::BackendStatus,
    ParityOp::SpeedTier,
    ParityOp::ReferenceChat,
    ParityOp::SideChat,
    ParityOp::Skills,
    ParityOp::InstructionFiles,
    ParityOp::ToolServers,
    ParityOp::Worktree,
];

/// 版ごとの確認済みの操作。表にない版・版が不明なときは空（すべて未確認）。
fn verified_ops(version: Option<&str>) -> &'static [ParityOp] {
    match version {
        Some("0.160.0") => VERIFIED_0_160_0,
        _ => &[],
    }
}

pub fn verification(version: Option<&str>, op: ParityOp) -> Verification {
    if verified_ops(version).contains(&op) {
        Verification::Verified
    } else {
        Verification::Unverified
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_live_checked_ops_are_verified_on_the_target_version() {
        const UNVERIFIED: [ParityOp; 4] = [
            ParityOp::CloudDelegation,
            ParityOp::Extensions,
            ParityOp::Memory,
            ParityOp::Personality,
        ];
        for op in ParityOp::ALL {
            let expected = if UNVERIFIED.contains(&op) { Verification::Unverified } else { Verification::Verified };
            assert_eq!(verification(Some("0.160.0"), op), expected, "{op:?}");
        }
    }

    #[test]
    fn unknown_or_other_versions_are_all_unverified() {
        for op in ParityOp::ALL {
            assert_eq!(verification(None, op), Verification::Unverified);
            assert_eq!(verification(Some("0.161.0"), op), Verification::Unverified);
        }
    }
}
