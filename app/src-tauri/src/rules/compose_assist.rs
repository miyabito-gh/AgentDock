//! 返信・推敲（`app/DESIGN_COMPOSE.md`）の純粋な部分。I/Oをしない。
//!
//! - 会話ごとの指示（developer_instructions）: 機能別の基本指示＋設定の共通ルール＋機能別ルール。`~/.codex` は変更しない。
//! - 最初の送信文（取り込んだ文章＋追加の指示）と、再調整の送信文。
//! - モデルと推論強度の決め方（既定は速さ重視の低い強度）。

use crate::backend::local::{ComposeMode, ComposeSettings, COMPOSE_RULES_MAX_CHARS};
use crate::backend::model::{ModelChoice, ModelInfo};

/// 取り込む文章の上限（文字数）。超えたら断る（黙って切り詰めない）。
pub const SOURCE_MAX_CHARS: usize = 20_000;
/// 追加の指示の上限（文字数）。
pub const INSTRUCTION_MAX_CHARS: usize = 2_000;

/// 機能別の基本指示。どちらも、結果の文だけを出力させる。
pub fn base_instructions(mode: ComposeMode) -> &'static str {
    match mode {
        ComposeMode::Reply => {
            "あなたは文章の返信を作る補助役です。利用者が渡す「相手の文章」への返信文だけを出力してください。\
             前置き・説明・あいさつの断り書き・引用符やコードブロックでの囲みは付けないでください。\
             事実・日付・金額・固有名詞は、渡された文章と追加の指示にあるものだけを使い、推測で足さないでください。"
        }
        ComposeMode::Polish => {
            "あなたは文章の推敲をする補助役です。利用者が渡す文章を、意味・事実・数字を変えずに、読みやすく整えた文だけを出力してください。\
             前置き・説明・変更点の解説・引用符やコードブロックでの囲みは付けないでください。\
             内容を足したり削ったりせず、元の言語で書いてください。"
        }
    }
}

/// 会話ごとの指示。空のルールは足さない（標準の書き方）。
pub fn developer_instructions(mode: ComposeMode, s: &ComposeSettings) -> String {
    let mut out = base_instructions(mode).to_string();
    let specific = match mode {
        ComposeMode::Reply => &s.reply_rules,
        ComposeMode::Polish => &s.polish_rules,
    };
    for (title, rules) in [("共通の文章ルール", &s.common_rules), (if mode == ComposeMode::Reply { "返信のルール" } else { "推敲のルール" }, specific)] {
        let rules = clip_chars(rules.trim(), COMPOSE_RULES_MAX_CHARS);
        if !rules.is_empty() {
            out.push_str(&format!("\n\n【{title}】\n{rules}"));
        }
    }
    out
}

fn clip_chars(s: &str, max: usize) -> &str {
    match s.char_indices().nth(max) {
        Some((i, _)) => &s[..i],
        None => s,
    }
}

/// 入力の検査。空・長すぎる入力は送らずに理由を返す。
pub fn check_input(source: &str, instruction: &str) -> Result<(), String> {
    if source.trim().is_empty() {
        return Err("文章がありません。文章を貼り付けるか、コピーしてから取り込み直してください".into());
    }
    if source.chars().count() > SOURCE_MAX_CHARS {
        return Err(format!("文章が長すぎます（{SOURCE_MAX_CHARS}文字まで）。分けて作成してください"));
    }
    check_instruction(instruction)
}

pub fn check_instruction(instruction: &str) -> Result<(), String> {
    if instruction.chars().count() > INSTRUCTION_MAX_CHARS {
        return Err(format!("指示が長すぎます（{INSTRUCTION_MAX_CHARS}文字まで）"));
    }
    Ok(())
}

/// 最初の送信文。取り込んだ文章は区切りで囲み、追加の指示と混ざらないようにする。
pub fn first_prompt(mode: ComposeMode, source: &str, instruction: &str) -> String {
    let lead = match mode {
        ComposeMode::Reply => "次の「相手の文章」に対する返信文を作ってください。",
        ComposeMode::Polish => "次の「文章」を推敲してください。",
    };
    let label = if mode == ComposeMode::Reply { "相手の文章" } else { "文章" };
    let instruction = instruction.trim();
    let extra = if instruction.is_empty() { "なし".to_string() } else { instruction.to_string() };
    format!("{lead}\n\n【{label}】\n{}\n【{label}ここまで】\n\n【追加の指示】\n{extra}", source.trim_end())
}

/// 再調整の送信文（同じ会話へ追加で送る）。
pub fn adjust_prompt(instruction: &str) -> String {
    format!("直前の結果を、次の指示に沿って直してください。直した文だけを出力してください。\n\n【指示】\n{}", instruction.trim())
}

/// 結果の整形（前後の空白だけ。中身は変えない）。
pub fn clean_result(text: &str) -> String {
    text.trim().to_string()
}

/// 使うモデルと推論強度。モデルの指定がなければ、取得済みの一覧の既定モデルを使う（一覧がなければ指定しない＝Codexの既定）。
/// 強度は、モデルが示す選択肢にあるときだけ指定する（なければ指定しない）。
pub fn resolve_choice(s: &ComposeSettings, models: &[ModelInfo]) -> Option<ModelChoice> {
    let visible = || models.iter().filter(|m| !m.hidden);
    let info = match s.model.as_deref().map(str::trim).filter(|m| !m.is_empty()) {
        Some(id) => visible().find(|m| m.id == id),
        None => visible().find(|m| m.is_default),
    };
    let model_id = match (&s.model, info) {
        (Some(id), _) if !id.trim().is_empty() => id.trim().to_string(),
        (_, Some(m)) => m.id.clone(),
        _ => return None,
    };
    let effort = s.effort.as_deref().filter(|e| info.is_some_and(|m| m.efforts.iter().any(|o| o.id == *e))).map(str::to_string);
    Some(ModelChoice { model: model_id, effort, speed_tier: None })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::backend::model::EffortOption;

    fn model(id: &str, default: bool, efforts: &[&str]) -> ModelInfo {
        ModelInfo {
            id: id.into(),
            display_name: id.into(),
            description: None,
            efforts: efforts.iter().map(|e| EffortOption { id: (*e).into(), description: None }).collect(),
            default_effort: None,
            is_default: default,
            hidden: false,
            input_kinds: Vec::new(),
            speed_tiers: Vec::new(),
            default_speed_tier: None,
        }
    }

    #[test]
    fn instructions_combine_base_common_and_mode_rules() {
        let s = ComposeSettings { common_rules: " 敬体で書く ".into(), reply_rules: "署名は付けない".into(), polish_rules: "箇条書きにしない".into(), ..Default::default() };
        let reply = developer_instructions(ComposeMode::Reply, &s);
        assert!(reply.starts_with(base_instructions(ComposeMode::Reply)));
        assert!(reply.contains("【共通の文章ルール】\n敬体で書く") && reply.contains("【返信のルール】\n署名は付けない"));
        assert!(!reply.contains("箇条書きにしない"), "other mode's rules are not sent");
        let polish = developer_instructions(ComposeMode::Polish, &s);
        assert!(polish.contains("【推敲のルール】\n箇条書きにしない") && !polish.contains("署名は付けない"));
        // 空のルールは足さない（標準の書き方）
        assert_eq!(developer_instructions(ComposeMode::Polish, &ComposeSettings::default()), base_instructions(ComposeMode::Polish));
    }

    #[test]
    fn base_instructions_forbid_preamble_and_quoting_and_keep_facts() {
        for m in [ComposeMode::Reply, ComposeMode::Polish] {
            let t = base_instructions(m);
            assert!(t.contains("前置き") && t.contains("引用符"));
        }
        assert!(base_instructions(ComposeMode::Polish).contains("意味・事実・数字を変えず"));
    }

    #[test]
    fn rules_are_clipped_by_chars_not_bytes() {
        let s = ComposeSettings { common_rules: "Ω".repeat(COMPOSE_RULES_MAX_CHARS + 10), ..Default::default() };
        let t = developer_instructions(ComposeMode::Reply, &s);
        assert_eq!(t.matches('Ω').count(), COMPOSE_RULES_MAX_CHARS);
    }

    #[test]
    fn prompts_separate_source_from_instruction() {
        let p = first_prompt(ComposeMode::Reply, "明日の会議は10時です。\n", "短めに");
        assert!(p.contains("【相手の文章】\n明日の会議は10時です。\n【相手の文章ここまで】") && p.ends_with("【追加の指示】\n短めに"));
        assert!(first_prompt(ComposeMode::Polish, "x", "  ").ends_with("【追加の指示】\nなし"));
        assert!(adjust_prompt(" もっと短く ").ends_with("【指示】\nもっと短く"));
    }

    #[test]
    fn input_is_checked_before_sending() {
        assert!(check_input("  \n", "").is_err());
        assert!(check_input("a", "").is_ok());
        assert!(check_input(&"あ".repeat(SOURCE_MAX_CHARS + 1), "").is_err());
        assert!(check_input("a", &"あ".repeat(INSTRUCTION_MAX_CHARS + 1)).is_err());
        assert!(check_instruction(&"あ".repeat(INSTRUCTION_MAX_CHARS)).is_ok());
    }

    #[test]
    fn model_defaults_to_the_listed_default_with_low_effort_only_if_offered() {
        let models = vec![model("a", false, &["low", "high"]), model("b", true, &["medium", "high"])];
        let s = ComposeSettings::default();
        // 既定モデル b は low を示していない → 強度は指定しない
        assert_eq!(resolve_choice(&s, &models), Some(ModelChoice { model: "b".into(), effort: None, speed_tier: None }));
        let chosen = ComposeSettings { model: Some("a".into()), ..Default::default() };
        assert_eq!(resolve_choice(&chosen, &models), Some(ModelChoice { model: "a".into(), effort: Some("low".into()), speed_tier: None }));
        // 一覧がなければ指定しない／一覧にないモデルでも、選んだものは尊重（強度は確かめられないので指定しない）
        assert_eq!(resolve_choice(&s, &[]), None);
        let unknown = ComposeSettings { model: Some("zzz".into()), ..Default::default() };
        assert_eq!(resolve_choice(&unknown, &models), Some(ModelChoice { model: "zzz".into(), effort: None, speed_tier: None }));
    }

    #[test]
    fn result_is_only_trimmed() {
        assert_eq!(clean_result("\n  「こんにちは」\n"), "「こんにちは」");
    }
}
