//! 想起比較試験（probes/app-server/recall-probe.mjs）用: 往復の記録(JSON)から、製品の純関数で引継ぎ資料と会話台帳を書き出す。
//!
//! 使い方: `cargo run --example handoff_material -- <transcript.json> <out_dir> <ledger_path> <purpose> [rounds]`
//! - transcript.json: `[{"user": "...", "ai": "..."}, ...]`
//! - out_dir に `envelope.txt`（資料ブロック。目的メモ＋直近往復＋目次＋台帳の案内）と `ledger.md` を書く。
//! - ledger_path: 資料の中で案内する台帳のパス（recall-probe が実際に台帳を置く場所）。
//! - rounds: 直近の往復数（省略時は製品の既定 3）。目的メモ以外の上限は製品の既定値。
//! 追加のモデル呼び出しはしない。

use agentdock_lib::backend::model::*;
use agentdock_lib::rules::continuity::{build_envelope, build_ledger, build_material_with, LedgerNote};
use agentdock_lib::store::records::*;
use serde::Deserialize;
use std::{env, fs, path::PathBuf};

#[derive(Deserialize)]
struct Round {
    user: String,
    ai: String,
}

fn entry(turn: &str, kind: ActivityKind, text: String) -> TranscriptEntry {
    TranscriptEntry {
        key: ItemKey {
            agent: AgentKey { backend: BackendKind::Codex, id: ExternalId("root".into()) },
            turn_id: Some(ExternalId(turn.into())),
            item_id: ExternalId(format!("{turn}-{kind:?}")),
        },
        kind,
        text: Known::direct(text),
        phase: ActivityPhase::Completed,
        message_phase: MessagePhase::default(),
        attachments: vec![],
    }
}

fn main() {
    let args: Vec<String> = env::args().skip(1).collect();
    if args.len() < 4 {
        eprintln!("usage: handoff_material <transcript.json> <out_dir> <ledger_path> <purpose> [rounds]");
        std::process::exit(2);
    }
    let rounds: usize = args.get(4).and_then(|s| s.parse().ok()).unwrap_or(3);
    let list: Vec<Round> = serde_json::from_slice(&fs::read(&args[0]).expect("read transcript")).expect("parse transcript");
    let turns: Vec<TurnRecord> = list
        .into_iter()
        .enumerate()
        .map(|(i, r)| {
            let id = format!("t{}", i + 1);
            TurnRecord {
                key: TurnKey { agent: AgentKey { backend: BackendKind::Codex, id: ExternalId("root".into()) }, turn_id: ExternalId(id.clone()) },
                end: Some(TurnEnd::Completed),
                // 固定の架空時刻（2027-01-12 09:00 UTC から1日ずつ）。UTC表示（offset 0）。
                started_at: Known::direct(UnixMillis(1_800_000_000_000 + i as i64 * 86_400_000)),
                completed_at: Known::NotFetched,
                entries: vec![entry(&id, ActivityKind::UserMessage, r.user), entry(&id, ActivityKind::AgentMessage, r.ai)],
                complete: true,
                failure: Known::NotFetched,
            }
        })
        .collect();

    let ledger_path = args[2].clone();
    let material = build_material_with(&turns, rounds, 0, &LedgerNote::Path(ledger_path.clone()));
    let ledger = build_ledger(&turns, 0);
    let seed = HandoffSeed {
        from: ChatKey { backend: BackendKind::Codex, id: ExternalId("source".into()) },
        material: material.text.clone(),
        rounds_included: material.rounds_included,
        truncated: material.truncated,
        created_at: UnixMillis(1),
        state: SeedState::Pending,
        toc_rounds: material.toc_rounds,
        ledger: if material.ledger { Some(ledger_path) } else { None },
    };
    let purpose = PurposeNote { text: args[3].clone(), revision: 1, updated_at: UnixMillis(1), source: PurposeSource::Chat };
    let envelope = build_envelope(Some(&purpose), Some(&seed), None).expect("envelope");

    let out = PathBuf::from(&args[1]);
    fs::create_dir_all(&out).expect("create out_dir");
    fs::write(out.join("envelope.txt"), &envelope).expect("write envelope");
    fs::write(out.join("ledger.md"), &ledger.text).expect("write ledger");
    println!(
        "{}",
        serde_json::json!({"roundsIncluded": material.rounds_included, "roundsAvailable": material.rounds_available, "truncated": material.truncated,
            "tocRounds": material.toc_rounds, "ledgerRounds": ledger.rounds, "envelopeChars": envelope.chars().count(), "ledgerChars": ledger.text.chars().count()})
    );
}
