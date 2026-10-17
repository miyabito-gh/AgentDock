# org-engine

Pure Rust logic for organization runs (DESIGN_ORG 4.1 to 4.9): types, state machine (business x confirmation), DAG validation, scheduler, plan policy, counters. No I/O, async, Tauri or Codex types. Run from this directory: `cargo test -p org-engine`.

## Design discretion (where DESIGN_ORG is ambiguous)

- Standalone `[workspace]` in this crate (the repo has no Cargo workspace); the host adds it later as a path dependency.
- Terminal states: completed, failed, cancelled have no outgoing transitions (user retry / un-completing stale tasks is not modeled).
- `cancelled` requires `Cause::User` or `Cause::StopConfirmed`; `Cause::Engine` cannot cancel. No transition maps disconnect/exit/unknown acceptance to failed/cancelled.
- Unconfirmed tasks may only move to `blocked` (hold) or be cancelled by user/confirmed stop. Confirmation axis moves freely between different values.
- Slots: running/reviewing tasks, and any non-terminal task with unconfirmed state (may still be running), occupy total/writer/role slots.
- `Role.writes` marks writer roles for the writer limit; disabled roles are never dispatched.
- Task count over the limit is a format error (not a confirmation reason); isolated tasks are reported separately (`isolated_tasks`), not an error.
- Policy confirmation for turn limit: minimum turns = one per task vs turns remaining. Parallelism request is compared to `max_total`.
- An automatic re-plan is "over the limit" when `auto_replans_used >= max_auto_replans` before it.
- Declared effects always trigger confirmation (one reason per effect); they are never the sole basis for approval.
- Turns are counted before sending (`spend_turn`); kind is informational, all kinds cost 1. Raising the turn limit must increase it.

## Persistence (P-ORG-3, `persist.rs`)

- `OrgStore` port (snapshot / events / usage rows). File implementation: host `store/org.rs` (reuses `store/atomic.rs`). Types derive serde; no other new dependency.
- Restore: snapshot (validated) + events with `seq > last_seq`, replayed through `Task::transition` / `set_confirm` / counters, so a log cannot produce a state the state machine forbids (rejected events are reported). Missing/corrupt/invalid snapshot: full replay from `Created`. A broken last line is dropped and reported (`dropped_tail`); other unreadable lines are skipped and reported (`skipped_lines`).
- `recover_on_restart`: confirmed `running`/`reviewing` -> `NeedsReconcile(Restart)`; `SendUnknown`/`InterruptRequested`/`StopUnconfirmed`/`NeedsReconcile` kept; terminal and not-in-flight tasks untouched; never cancelled/failed; sets `halt = Restart` (dispatch resumes only by user action). Classification is written as events, then a fresh snapshot.
- `Task.status` / `confirm` stay `pub`. Do not assign them outside tests; restore goes through `WorkflowState::apply` and `validate` (a dedicated constructor was judged excessive).
- Usage rows: unknown token counts are `None`, never 0 (`summarize_usage` counts them separately).
