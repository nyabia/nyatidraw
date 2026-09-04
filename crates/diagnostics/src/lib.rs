#![forbid(unsafe_code)]

pub mod event {
    pub const RAW_INPUT: &str = "input.raw";
    pub const SAMPLE_DEQUEUE: &str = "input.dequeue";
    pub const DAB_BATCH_READY: &str = "brush.batch_ready";
    pub const GPU_SUBMIT: &str = "gpu.submit";
    pub const STROKE_SEALED: &str = "stroke.sealed";
    pub const MATERIALIZED: &str = "stroke.materialized";
    pub const DURABLE_COMMIT: &str = "project.commit";
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct QueueCounters {
    pub enqueued: u64,
    pub coalesced_moves: u64,
    pub rejected_transitions: u64,
}
