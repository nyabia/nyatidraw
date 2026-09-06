//! Installation updates never own artwork and never terminate a live writer.
use crate::live_ink::{CloseStatus, ExportStatus, LiveInkBridge};
use std::{
    path::PathBuf,
    sync::{Arc, Mutex, OnceLock},
    thread,
};
use velopack::{UpdateCheck, UpdateManager, VelopackAsset, sources::GithubSource};

static UPDATER: OnceLock<Arc<Updater>> = OnceLock::new();

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Status {
    Development,
    Checking,
    Current(String),
    Downloading,
    Ready(String),
    Failed(String),
}

struct State {
    status: Status,
    pending: Option<(UpdateManager, VelopackAsset)>,
    restart_requested: bool,
    project_path: Option<PathBuf>,
}

struct Updater {
    state: Mutex<State>,
    live_ink: LiveInkBridge,
}

pub(crate) fn initialize(live_ink: LiveInkBridge) {
    let updater = Arc::new(Updater {
        state: Mutex::new(State {
            status: Status::Development,
            pending: None,
            restart_requested: false,
            project_path: None,
        }),
        live_ink,
    });
    if UPDATER.set(updater).is_ok() {
        check();
    }
}

pub(crate) fn status() -> Status {
    UPDATER.get().map_or(Status::Development, |updater| {
        updater
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .status
            .clone()
    })
}

pub(crate) fn set_project_path(path: PathBuf) {
    if let Some(updater) = UPDATER.get() {
        updater
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .project_path = Some(path);
    }
}

pub(crate) fn check() {
    let Some(updater) = UPDATER.get().cloned() else {
        return;
    };
    {
        let mut state = updater
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if matches!(
            state.status,
            Status::Checking | Status::Downloading | Status::Ready(_)
        ) {
            return;
        }
        state.status = Status::Checking;
    }
    updater.live_ink.notify_ui();
    let worker_updater = updater.clone();
    if let Err(error) = thread::Builder::new()
        .name("nyatidraw-update".into())
        .spawn(move || {
            worker_updater.check_and_download();
        })
    {
        updater.publish(Status::Failed(format!(
            "업데이트 확인을 시작하지 못했습니다: {error}"
        )));
    }
}

impl Updater {
    fn publish(&self, status: Status) {
        self.state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .status = status;
        self.live_ink.notify_ui();
    }

    fn check_and_download(&self) {
        let source = GithubSource::new("https://github.com/nyabia/nyatidraw", None, true);
        let Ok(manager) = UpdateManager::new(source, None, None) else {
            self.publish(Status::Development);
            return;
        };
        // A completed package remains useful when the next launch is offline.
        if let Some(asset) = manager.get_update_pending_restart() {
            self.ready(manager, asset);
            return;
        }
        match manager.check_for_updates() {
            Ok(UpdateCheck::UpdateAvailable(update)) => {
                self.publish(Status::Downloading);
                match manager.download_updates(&update, None) {
                    Ok(()) => self.ready(manager, update.TargetFullRelease.clone()),
                    Err(error) => self.publish(Status::Failed(format!(
                        "다운로드 실패 · 다시 시도할 수 있습니다: {error}"
                    ))),
                }
            }
            Ok(UpdateCheck::NoUpdateAvailable | UpdateCheck::RemoteIsEmpty) => {
                self.publish(Status::Current(manager.get_current_version_as_string()));
            }
            Err(error) => self.publish(Status::Failed(format!(
                "업데이트 확인 실패 · 현재 버전은 계속 사용할 수 있습니다: {error}"
            ))),
        }
    }

    fn ready(&self, manager: UpdateManager, asset: VelopackAsset) {
        let version = asset.Version.clone();
        {
            let mut state = self
                .state
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            state.pending = Some((manager, asset));
            state.status = Status::Ready(version);
        }
        self.live_ink.notify_ui();
    }
}

pub(crate) fn request_restart() -> bool {
    let Some(updater) = UPDATER.get() else {
        return false;
    };
    let mut state = updater
        .state
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    if state.pending.is_none() {
        return false;
    }
    state.restart_requested = true;
    true
}

pub(crate) fn cancel_restart() {
    if let Some(updater) = UPDATER.get() {
        updater
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .restart_requested = false;
    }
}

/// Called only by the parent's ordinary `WM_CLOSE` path after the writer and
/// export have joined. Velopack waits for process exit; it does not kill us.
pub(crate) fn apply_after_saved_close() -> Result<(), String> {
    let Some(updater) = UPDATER.get() else {
        return Ok(());
    };
    let mut state = updater
        .state
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    if !state.restart_requested {
        return Ok(());
    }
    state.restart_requested = false;
    if updater.live_ink.close_status() != CloseStatus::Ready
        || updater.live_ink.workspace_failed()
        || matches!(
            updater.live_ink.export_status_snapshot(),
            ExportStatus::Failed { .. }
        )
    {
        return Err("작품 저장을 확인한 뒤 업데이트를 다시 시도해주세요".into());
    }
    if let Some((manager, asset)) = &state.pending {
        let args = state
            .project_path
            .iter()
            .map(|path| path.as_os_str())
            .collect::<Vec<_>>();
        if let Err(error) = manager.wait_exit_then_apply_updates(asset, false, true, args) {
            let summary = format!("업데이트 적용을 시작하지 못했습니다: {error}");
            state.status = Status::Failed(summary.clone());
            return Err(summary);
        }
    }
    Ok(())
}
