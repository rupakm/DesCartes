use descartes_core::async_runtime::{ReadyTaskPolicy, TaskId};
use descartes_core::{EventFrontierPolicy, FrontierEvent, FrontierEventKind, SimTime};

use descartes_explore::frontier::ReplayFrontierPolicy;
use descartes_explore::ready_task::ReplayReadyTaskPolicy;
use descartes_explore::trace::{AsyncRuntimeDecision, SchedulerDecision, TraceEvent};

#[test]
fn replay_frontier_allows_reordered_choice_set_when_chosen_seq_present() {
    let events = vec![TraceEvent::SchedulerDecision(SchedulerDecision {
        time_nanos: 0,
        chosen_index: 0,
        chosen_seq: Some(20),
        frontier_seqs: vec![10, 20],
        frontier: vec![],
    })];

    let mut policy = ReplayFrontierPolicy::from_trace_events(&events);

    // Same set of seqs, but in a different observed order.
    let frontier = vec![
        FrontierEvent {
            seq: 20,
            kind: FrontierEventKind::Task,
            component_id: None,
            task_id: None,
        },
        FrontierEvent {
            seq: 10,
            kind: FrontierEventKind::Task,
            component_id: None,
            task_id: None,
        },
    ];

    let idx = policy.choose(SimTime::zero(), &frontier);
    assert_eq!(idx, 0);
    assert!(policy.take_error().is_none());
}

#[test]
fn replay_tokio_ready_allows_reordered_ready_set_when_chosen_task_id_present() {
    let events = vec![TraceEvent::AsyncRuntimeDecision(AsyncRuntimeDecision {
        time_nanos: 0,
        chosen_index: 0,
        chosen_task_id: Some(2),
        ready_task_ids: vec![1, 2],
    })];

    let mut policy = ReplayReadyTaskPolicy::from_trace_events(&events);

    // Same set of tasks, different observed order.
    let ready = vec![TaskId(2), TaskId(1)];

    let idx = policy.choose(SimTime::zero(), &ready);
    assert_eq!(idx, 0);
    assert!(policy.take_error().is_none());
}
