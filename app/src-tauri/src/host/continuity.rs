//! 継続チャット最小版のホスト側（`app/DESIGN_CONT.md` §4・§5）: 使用率の取込み、目的メモの編集と届けた記録。
//!
//! - 資料を付けるのは利用者の手動送信（新しい turn・キュー項目の追加指示でない）だけ。キュー・追加指示・組織の送信は通らない。
//! - 「届いた」は受理を確認できた送信だけ。受理不明・拒否は記録しない（次の手動送信でもう一度添える。自動で再送しない）。
//! - 使用量が取れないときは推定しない。書込みは2秒まとめ。

use std::sync::Arc;
use std::time::Duration;

use super::state::agent_key_of;
use super::{err, now_ms, Host};
use crate::backend::backend::{AgentHistory, AiBackend, ReadOptions};
use crate::backend::ipc::*;
use crate::backend::local::ChatArgs;
use crate::backend::local::{HandoffBlock, SaveScope};
use crate::backend::model::*;
use crate::rules::continuity as cont;
use crate::store::records::{ChatLocalFile, ContextUsageSample, ConversationRecord, HandoffSeed, PurposeNote, PurposeSource, SeedState, Superseded};
use crate::store::{Restored, RestoredChat, Store, StoreError};

/// 使用率・届けた記録の書込みをまとめる時間（turn 中に何度も来るため）。
pub(super) const SAVE_DELAY: Duration = Duration::from_secs(2);

/// 目的メモの保存の判断（純粋）。結果は新しい目的メモ（None＝削除）と、変更があるか。
#[derive(Debug, PartialEq)]
pub enum PurposeEdit {
    /// 変更なし（同じ本文、または元から無い）。
    Unchanged,
    /// 保存する。`None` は削除（記録は残す）。
    Apply(Option<PurposeNote>),
}

/// 目的メモの更新を判断する。版の不一致・上限超過は Err。版は保存のたびに増やし、削除後に作り直しても届け済みの版と衝突しない。
pub fn plan_purpose_edit(current: Option<&ConversationRecord>, text: &str, expected_revision: Option<u32>, now: UnixMillis) -> Result<PurposeEdit, String> {
    if cont::purpose_too_long(text) {
        return Err(format!("目的メモは{}字までです（現在 {} 字）", cont::PURPOSE_MAX_CHARS, text.chars().count()));
    }
    let existing = current.and_then(|c| c.purpose.as_ref());
    if existing.map(|p| p.revision) != expected_revision {
        return Err("目的メモが別の場所で更新されています。開き直してから編集してください".to_string());
    }
    if text.trim().is_empty() {
        return Ok(if existing.is_some() { PurposeEdit::Apply(None) } else { PurposeEdit::Unchanged });
    }
    if existing.is_some_and(|p| p.text == text) {
        return Ok(PurposeEdit::Unchanged);
    }
    let floor = existing.map(|p| p.revision).max(current.and_then(|c| c.purpose_delivered_rev)).unwrap_or(0);
    Ok(PurposeEdit::Apply(Some(PurposeNote { text: text.to_string(), revision: floor.saturating_add(1), updated_at: now, source: PurposeSource::Chat })))
}

/// 以前の thread の履歴を読んでよいか（`segment` が `head` の会話の記録にあるときだけ）。
pub fn segment_readable(rec: Option<&ConversationRecord>, segment: &ChatKey) -> bool {
    rec.is_some_and(|c| c.threads.iter().any(|t| &t.chat == segment))
}

/// 事前表示を組み立てる（純粋）。`material` が None（読めなかった）でも、目的メモだけの資料と理由は返す。
pub fn assemble_preview(
    purpose: Option<&PurposeNote>,
    from: &ChatKey,
    material: Option<cont::Material>,
    blockers: Vec<HandoffBlock>,
    notes: Vec<String>,
    now: UnixMillis,
) -> HandoffPreview {
    let (rounds_included, rounds_available, truncated) = material.as_ref().map(|m| (m.rounds_included, m.rounds_available, m.truncated)).unwrap_or((0, 0, false));
    let seed = material.map(|m| HandoffSeed {
        from: from.clone(),
        material: m.text,
        rounds_included: m.rounds_included,
        truncated: m.truncated,
        created_at: now,
        state: SeedState::Pending,
    });
    let envelope = cont::build_envelope(purpose, seed.as_ref()).unwrap_or_default();
    HandoffPreview { blockers, notes, chars: envelope.chars().count().min(u32::MAX as usize) as u32, envelope, rounds_included, rounds_available, truncated }
}

/// 起動時の鎖の照合の結果（適用済みの分）。
#[derive(Debug, Default, PartialEq, Eq)]
pub struct ChainApplied {
    /// 途中停止の修復として `superseded_by` を書いた先頭の元（呼び出し側が保存する）。
    pub repaired: Vec<ChatKey>,
    /// 引き継ぎ先が読み込めないため、一覧へ戻した（メモリ上で `superseded_by` を外した）チャット。保存はしない。
    pub restored_to_list: Vec<ChatKey>,
    /// 競合の警告文。
    pub conflicts: Vec<String>,
}

/// 読み込んだ chat.json 群に `resolve_chains` を適用する。隠す＝`superseded_by` が残る、戻す＝メモリ上で外す。
/// 削除・アーカイブ・resume・送信はしない。
pub fn apply_chain_resolution(chats: &mut [RestoredChat], now: UnixMillis) -> ChainApplied {
    let entries: Vec<cont::ChainEntry> = chats
        .iter()
        .filter_map(|c| {
            Some(cont::ChainEntry {
                chat: c.local.chat.clone()?,
                segments: c.local.conversation.as_ref().map(|r| r.threads.iter().map(|t| t.chat.clone()).collect()).unwrap_or_default(),
                superseded_by: c.local.superseded_by.as_ref().map(|s| s.by.clone()),
                replaces: c.local.conversation.as_ref().and_then(|r| r.replaces.clone()),
            })
        })
        .collect();
    let res = cont::resolve_chains(&entries);
    let mut out = ChainApplied::default();
    for rep in &res.repairs {
        if let Some(c) = chats.iter_mut().find(|c| c.local.chat.as_ref() == Some(&rep.chat)) {
            c.local.superseded_by = Some(Superseded { by: rep.by.clone(), at: now });
            out.repaired.push(rep.chat.clone());
        }
    }
    for c in chats.iter_mut() {
        let Some(key) = c.local.chat.clone() else { continue };
        if c.local.superseded_by.is_some() && !res.hidden.contains(&key) {
            c.local.superseded_by = None;
            out.repaired.retain(|k| k != &key);
            out.restored_to_list.push(key);
        }
    }
    out.conflicts = res
        .conflicts
        .iter()
        .map(|c| format!("同じ以前の thread を引き継ぎ元とする会話が{}件あります。自動では統合せず、別の会話として表示します（{}）。", c.claimants.len(), c.segment.id.0))
        .collect();
    out
}

/// 読込み直後（`HostData::restore` の前）に鎖を照合する。途中停止の修復は書き込み、競合は警告にする。
/// 書込みに失敗した修復はメモリ上だけ有効（次の起動で再度試す）。読めないファイルには触らない（読込みの時点で除外済み）。
pub fn resolve_chains_on_load(store: &Store, restored: &mut Restored, now: UnixMillis) {
    let applied = apply_chain_resolution(&mut restored.chats, now);
    for key in &applied.repaired {
        if let Some(c) = restored.chats.iter().find(|c| c.local.chat.as_ref() == Some(key)) {
            if let Err(e) = store.save_chat_local(&c.local) {
                restored.problems.push(e);
            }
        }
    }
    restored.problems.extend(applied.conflicts.into_iter().map(|message| StoreError::Notice { message }));
}

impl Host {
    /// 引き継ぎの事前表示（読取りのみ）。引き継ぎ元には resume・送信・管理操作をしない。
    /// 止める理由があっても、履歴を読めた範囲の資料は返す。読めなかったときは目的メモだけの資料と、その旨の注記を返す。
    pub async fn preview_handoff(self: &Arc<Self>, args: ChatArgs) -> Result<HandoffPreview, IpcError> {
        let chat = args.chat;
        if self.read(|d| d.chat(&chat).is_none() && !d.locals.contains_key(&chat)) {
            return Err(err(IpcErrorCode::NotFound, "チャットが見つかりません"));
        }
        let super::continuity_handoff::HandoffFacts { ctx, redo, scan_incomplete } = self.handoff_send_ctx(&chat).await;
        let (blockers, mut notes) = match cont::handoff_blocker(&ctx) {
            Ok(notes) => (Vec::new(), super::continuity_handoff::with_scan_note(notes, scan_incomplete)),
            Err(b) => (vec![b], Vec::new()),
        };
        let (purpose, rounds, seed) = self.read(|d| {
            let rec = d.locals.get(&chat).and_then(|f| f.conversation.as_ref());
            (rec.and_then(|c| c.purpose.clone()), d.settings.continuation.rounds, rec.and_then(|c| c.seed.clone()))
        });
        let rounds = rounds.clamp(1, 10) as usize;
        let material = if let (true, Some(s)) = (redo, seed) {
            // 空の先頭のやり直し: 送る資料は先頭の資料そのまま（履歴を読み直さない）。
            Some(cont::Material { text: s.material, rounds_included: s.rounds_included, rounds_available: s.rounds_included, truncated: s.truncated })
        } else if ctx.connected {
            match self.read_limit.run(self.backend.read(agent_key_of(&chat), ReadOptions { include_turns: true })).await {
                Ok(h) => Some(cont::build_material(&h.turns, rounds, crate::win::clock::local_utc_offset_min())),
                // 読めない理由が他にあるなら理由だけを返し、なければエラーにする（空の資料にしない）。
                Err(e) if blockers.is_empty() => return Err(err(IpcErrorCode::Io, format!("履歴を取得できませんでした（未取得）: {e}"))),
                Err(_) => {
                    notes.push("履歴を読めなかったため、引き継ぎ資料の往復は表示できません。".to_string());
                    None
                }
            }
        } else {
            notes.push("接続していないため、履歴を読めません。".to_string());
            None
        };
        Ok(assemble_preview(purpose.as_ref(), &chat, material, blockers, notes, now_ms()))
    }

    /// 以前の thread の履歴を読む（読取りのみ。resume・状態への反映・保存をしない）。`segment` は `head` の記録にある thread だけ。
    pub async fn read_segment_history(self: &Arc<Self>, args: ReadSegmentHistoryArgs) -> Result<AgentHistory, IpcError> {
        if !self.read(|d| segment_readable(d.locals.get(&args.head).and_then(|f| f.conversation.as_ref()), &args.segment)) {
            return Err(err(IpcErrorCode::InvalidArgs, "この会話の以前の thread ではありません"));
        }
        self.read_limit
            .run(self.backend.read(agent_key_of(&args.segment), ReadOptions { include_turns: true }))
            .await
            .map_err(|e| err(IpcErrorCode::Io, format!("履歴を取得できませんでした（未取得）: {e}")))
    }

    /// 手動送信の本文を組み立てる（届けるべきなら目的メモの資料を先頭に付ける。利用者の文の見出しは無害化）。
    pub(super) fn compose_manual_text(&self, chat: &ChatKey, user_text: &str) -> String {
        let envelope = self.read(|d| d.locals.get(chat).and_then(|f| cont::envelope_for_manual_send(f.conversation.as_ref(), true, false)));
        cont::compose_send_text(envelope.as_deref(), user_text)
    }

    /// ルートの使用量を取り込む。メモリの値を即時に通知し、chat.json へは2秒まとめで書く。
    pub(super) fn note_context_usage(self: &Arc<Self>, chat: &ChatKey, sample: ContextUsageSample) {
        let dir_id = self.persist.dir_id_for(chat);
        self.mutate(|d| ((), d.note_context_usage(chat, sample, dir_id)));
        self.schedule_save(SaveScope::ChatLocal { chat: chat.clone() }, SAVE_DELAY);
    }

    /// 目的メモを添えた送信の受理を確認した。届けた版を記録し、圧縮の印を外す。会話の記録がなければ何もしない。
    pub(super) fn note_purpose_delivered(self: &Arc<Self>, chat: &ChatKey, rev: u32) {
        let has = self.read(|d| d.locals.get(chat).is_some_and(|f| f.conversation.is_some()));
        if !has {
            return;
        }
        self.update_local(chat, true, SAVE_DELAY, |f| {
            if let Some(c) = f.conversation.as_mut() {
                c.purpose_delivered_rev = Some(rev);
                c.compacted_since_delivery = false;
            }
        });
    }

    /// 目的メモの保存（ヘッダー「…」の「目的メモ…」）。組織が所有するチャットでは編集しない。
    pub fn set_purpose_note(self: &Arc<Self>, args: SetPurposeNoteArgs) -> Result<crate::backend::local::ConversationView, IpcError> {
        self.check_not_org_owned(&args.chat)?;
        if self.read(|d| d.chat(&args.chat).is_none()) {
            return Err(err(IpcErrorCode::NotFound, "チャットが見つかりません"));
        }
        self.precheck_space()?;
        let now = now_ms();
        let new_id = self.local_id("conv");
        let dir_id = self.persist.dir_id_for(&args.chat);
        // 版の照合と書換えを同じロックの中で行う（二重押下・別窓の上書きを防ぐ）。
        let result = self.mutate(|d| {
            let current = d.locals.get(&args.chat).and_then(|f| f.conversation.as_ref());
            let edit = match plan_purpose_edit(current, &args.text, args.expected_revision, now) {
                Ok(e) => e,
                Err(m) => return (Err(err(IpcErrorCode::InvalidArgs, m)), vec![]),
            };
            let view = |c: &ConversationRecord| cont::conversation_view(c);
            match edit {
                PurposeEdit::Unchanged => match current {
                    Some(c) => (Ok((view(c), false)), vec![]),
                    None => (Err(err(IpcErrorCode::InvalidArgs, "目的メモは設定されていません")), vec![]),
                },
                PurposeEdit::Apply(purpose) => {
                    let file = d.locals.entry(args.chat.clone()).or_insert_with(|| ChatLocalFile::new(dir_id, Some(args.chat.clone())));
                    let rec = file.conversation.get_or_insert_with(|| ConversationRecord {
                        id: new_id,
                        purpose: None,
                        threads: Vec::new(),
                        seed: None,
                        purpose_delivered_rev: None,
                        compacted_since_delivery: false,
                        replaces: None,
                        intake: None,
                    });
                    rec.purpose = purpose;
                    let out = view(rec);
                    let ev = d.local_view(&args.chat).map(|local| HostEvent::ChatLocalUpdated { local }).into_iter().collect();
                    (Ok((out, true)), ev)
                }
            }
        });
        let (view, changed) = result?;
        if changed {
            self.schedule_save(SaveScope::ChatLocal { chat: args.chat.clone() }, Duration::ZERO);
        }
        Ok(view)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rec(purpose: Option<(&str, u32)>, delivered: Option<u32>) -> ConversationRecord {
        ConversationRecord {
            id: LocalId("c".into()),
            purpose: purpose.map(|(t, r)| PurposeNote { text: t.into(), revision: r, updated_at: UnixMillis(1), source: PurposeSource::Chat }),
            threads: vec![],
            seed: None,
            purpose_delivered_rev: delivered,
            compacted_since_delivery: false,
            replaces: None,
            intake: None,
        }
    }
    const NOW: UnixMillis = UnixMillis(9);

    fn ck(id: &str) -> ChatKey {
        ChatKey { backend: BackendKind::Codex, id: ExternalId(id.into()) }
    }
    fn restored(id: &str, segments: &[&str], by: Option<&str>) -> RestoredChat {
        let mut local = ChatLocalFile::new(LocalId(format!("dir-{id}")), Some(ck(id)));
        if !segments.is_empty() {
            let mut r = rec(None, None);
            r.threads = segments.iter().map(|s| crate::store::records::ThreadSegment { chat: ck(s), handed_off_at: UnixMillis(1), title: None }).collect();
            local.conversation = Some(r);
        }
        local.superseded_by = by.map(|b| Superseded { by: ck(b), at: UnixMillis(1) });
        RestoredChat { dir: std::path::PathBuf::new(), local, queue: None }
    }
    fn by_of(chats: &[RestoredChat], id: &str) -> Option<ChatKey> {
        chats.iter().find(|c| c.local.chat == Some(ck(id))).and_then(|c| c.local.superseded_by.as_ref().map(|s| s.by.clone()))
    }

    #[test]
    fn apply_keeps_hidden_when_target_is_loaded_and_changes_nothing() {
        let mut chats = vec![restored("p", &[], Some("n")), restored("n", &["p"], None)];
        let out = apply_chain_resolution(&mut chats, NOW);
        assert_eq!(out, ChainApplied::default());
        assert_eq!(by_of(&chats, "p"), Some(ck("n")));
        assert_eq!(by_of(&chats, "n"), None);
    }

    #[test]
    fn apply_repairs_a_handoff_interrupted_before_the_mark_was_written() {
        let mut chats = vec![restored("p", &[], None), restored("n", &["p"], None)];
        let out = apply_chain_resolution(&mut chats, NOW);
        assert_eq!(out.repaired, vec![ck("p")]);
        assert!(out.restored_to_list.is_empty());
        assert_eq!(by_of(&chats, "p"), Some(ck("n")));
        assert_eq!(chats[0].local.superseded_by.as_ref().map(|s| s.at), Some(NOW));
    }

    #[test]
    fn apply_returns_the_previous_thread_to_the_list_when_the_target_is_gone() {
        // 引き継ぎ先 n が削除・破損で読み込めない: p の印はメモリ上で外し（保存しない）、一覧へ戻す。
        let mut chats = vec![restored("p", &[], Some("n"))];
        let out = apply_chain_resolution(&mut chats, NOW);
        assert_eq!(out.restored_to_list, vec![ck("p")]);
        assert!(out.repaired.is_empty());
        assert_eq!(by_of(&chats, "p"), None);
    }

    #[test]
    fn apply_reports_conflicts_without_merging() {
        let mut chats = vec![restored("p", &[], Some("a")), restored("a", &["p"], None), restored("b", &["p"], None)];
        let out = apply_chain_resolution(&mut chats, NOW);
        assert_eq!(out.conflicts.len(), 1);
        assert_eq!(by_of(&chats, "p"), Some(ck("a")));
        assert_eq!(by_of(&chats, "b"), None);
    }

    #[test]
    fn segment_is_readable_only_when_recorded_in_the_head() {
        let mut r = rec(None, None);
        r.threads = vec![crate::store::records::ThreadSegment { chat: ck("p"), handed_off_at: UnixMillis(1), title: None }];
        assert!(segment_readable(Some(&r), &ck("p")));
        assert!(!segment_readable(Some(&r), &ck("other")));
        assert!(!segment_readable(None, &ck("p")));
    }

    #[test]
    fn preview_without_history_still_returns_the_purpose_and_the_reasons() {
        let purpose = PurposeNote { text: "目的".into(), revision: 2, updated_at: UnixMillis(1), source: PurposeSource::Chat };
        let block = HandoffBlock { kind: crate::backend::local::HandoffBlockKind::Busy, message: "実行中".into() };
        let p = assemble_preview(Some(&purpose), &ck("p"), None, vec![block.clone()], vec!["注記".into()], NOW);
        assert_eq!(p.blockers, vec![block]);
        assert_eq!((p.rounds_included, p.rounds_available, p.truncated), (0, 0, false));
        assert!(p.envelope.contains("目的メモ 版2") && p.envelope.contains("目的"));
        assert!(!p.envelope.contains("引き継ぎ"));
        assert_eq!(p.chars as usize, p.envelope.chars().count());
        // 目的メモも履歴もなければ空（資料なし）。
        assert_eq!(assemble_preview(None, &ck("p"), None, vec![], vec![], NOW).envelope, "");
    }

    #[test]
    fn preview_with_zero_rounds_is_still_a_valid_handoff() {
        let m = cont::build_material(&[], 3, 0);
        let p = assemble_preview(None, &ck("p"), Some(m), vec![], vec![], NOW);
        assert!(p.envelope.contains("以前の会話に依頼はありません"));
        assert_eq!((p.rounds_included, p.rounds_available), (0, 0));
    }

    #[test]
    fn first_save_is_revision_one_and_needs_matching_expectation() {
        let Ok(PurposeEdit::Apply(Some(p))) = plan_purpose_edit(None, "目的", None, NOW) else { panic!("保存される") };
        assert_eq!((p.revision, p.updated_at), (1, NOW));
        // 画面が版2を見ていたのに、実際は無い／版が違う → 拒否（上書きしない）。
        assert!(plan_purpose_edit(None, "目的", Some(2), NOW).is_err());
        let c = rec(Some(("旧", 3)), None);
        assert!(plan_purpose_edit(Some(&c), "新", Some(2), NOW).is_err());
        assert!(plan_purpose_edit(Some(&c), "新", None, NOW).is_err());
    }

    #[test]
    fn change_bumps_revision_and_same_text_is_unchanged() {
        let c = rec(Some(("旧", 3)), Some(3));
        assert_eq!(plan_purpose_edit(Some(&c), "旧", Some(3), NOW), Ok(PurposeEdit::Unchanged));
        let Ok(PurposeEdit::Apply(Some(p))) = plan_purpose_edit(Some(&c), "新", Some(3), NOW) else { panic!("保存される") };
        assert_eq!(p.revision, 4);
    }

    #[test]
    fn empty_text_deletes_and_recreation_does_not_reuse_delivered_revision() {
        let c = rec(Some(("旧", 3)), Some(3));
        assert_eq!(plan_purpose_edit(Some(&c), " \n", Some(3), NOW), Ok(PurposeEdit::Apply(None)));
        assert_eq!(plan_purpose_edit(None, "  ", None, NOW), Ok(PurposeEdit::Unchanged));
        // 削除後の作り直しは、届け済みの版より新しい版にする（届け済みと誤認しない）。
        let deleted = rec(None, Some(3));
        let Ok(PurposeEdit::Apply(Some(p))) = plan_purpose_edit(Some(&deleted), "作り直し", None, NOW) else { panic!("保存される") };
        assert_eq!(p.revision, 4);
    }

    #[test]
    fn over_limit_is_rejected_at_the_limit_accepted() {
        let at_limit = "あ".repeat(cont::PURPOSE_MAX_CHARS);
        assert!(plan_purpose_edit(None, &at_limit, None, NOW).is_ok());
        let over = "あ".repeat(cont::PURPOSE_MAX_CHARS + 1);
        assert!(plan_purpose_edit(None, &over, None, NOW).is_err());
    }
}
