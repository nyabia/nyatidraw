use dioxus::prelude::*;

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum RecoveryStatus {
    Idle,
    Pending,
    Saving,
    Saved,
    Failed,
}

impl RecoveryStatus {
    pub const fn description(self) -> &'static str {
        match self {
            Self::Idle => "아직 브라우저 자동 복구 저장이 없습니다.",
            Self::Pending => "브라우저 자동 복구 저장 대기 중",
            Self::Saving => "브라우저 자동 복구 저장 중",
            Self::Saved => "이 브라우저에 자동 복구 저장됨 · 파일 다운로드와 별개입니다.",
            Self::Failed => "브라우저 자동 복구 저장 실패 · 작업 파일을 내려받아 보관하세요.",
        }
    }
}

impl super::Editor {
    pub(super) fn set_recovery_status(mut self, status: RecoveryStatus) {
        if *self.recovery_status.peek() != status {
            self.recovery_status.set(status);
        }
    }
}
