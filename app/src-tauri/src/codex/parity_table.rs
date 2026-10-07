//! 同等性の操作の確認状況の表（版別、`app/DESIGN_P3.md` §0.3）。
//!
//! `(Codex版, ParityOp) → Verification`。初期値はすべて `Unverified`。実測で成立を確認したものだけを
//! 版ごとに `Verified` として足す。実機確認の結果は司令塔がこの表を更新する。
//! 宣言（Support）とは別の軸で、`Unverified` の操作は「未確認」と表示し、結果は観測事実だけを書く。

use crate::backend::model::{ParityOp, Verification};

/// 0.160.0 で実測により成立を確認した操作。明示Skillの呼出し（`UserInput.skill`）だけ（DESIGN_P3 §0.3、#11）。
const VERIFIED_0_160_0: &[ParityOp] = &[ParityOp::Skills];

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
    fn everything_is_unverified_except_explicit_skill_on_the_target_version() {
        for op in ParityOp::ALL {
            let expected = if op == ParityOp::Skills { Verification::Verified } else { Verification::Unverified };
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
