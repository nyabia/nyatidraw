use super::{ExportGenerationGate, ExportPngOutcome, export_current_png_sized};
use std::path::PathBuf;
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, Ordering},
};

#[derive(Default)]
pub(super) struct Lane {
    busy: Arc<AtomicBool>,
    generations: Arc<Mutex<ExportGenerationGate>>,
}

pub(super) struct Request {
    path: PathBuf,
    size: [u32; 2],
    generation: u64,
    busy: Arc<AtomicBool>,
    generations: Arc<Mutex<ExportGenerationGate>>,
}

impl Lane {
    pub(super) fn reserve(&self, path: PathBuf, size: [u32; 2]) -> Result<Request, String> {
        crate::scaled_export::validate_target(&path)?;
        if self.busy.swap(true, Ordering::AcqRel) {
            return Err("PNG 크기 지정 내보내기가 진행 중입니다.".into());
        }
        let mut generations = self
            .generations
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let Ok(generation) = generations.next_generation() else {
            self.busy.store(false, Ordering::Release);
            return Err("내보내기 번호를 갱신할 수 없습니다. 그림을 다시 열어주세요.".into());
        };
        generations.accept_submission(generation);
        Ok(Request {
            path,
            size,
            generation,
            busy: Arc::clone(&self.busy),
            generations: Arc::clone(&self.generations),
        })
    }
}

impl Request {
    pub(super) fn run(
        self,
        tiles: &nyatidraw_tiles::TileSnapshot,
        tree: &nyatidraw_document::LayerTree,
        canvas: nyatidraw_api::CanvasSpec,
        live_ink: &crate::live_ink::LiveInkBridge,
    ) {
        live_ink.publish_activation_notice(format!(
            "{} × {} PNG 내보내기 중…",
            self.size[0], self.size[1]
        ));
        let result = crate::scaled_export::validate_target(&self.path).and_then(|()| {
            export_current_png_sized(
                &self.path,
                tiles,
                tree,
                canvas,
                self.generation,
                &self.generations,
                Some(self.size),
            )
        });
        let notice = match result {
            Ok(ExportPngOutcome::Replaced) => format!(
                "{} × {} PNG 저장 완료: {}",
                self.size[0],
                self.size[1],
                self.path.display()
            ),
            Ok(ExportPngOutcome::Superseded { .. }) => {
                "새 내보내기 요청으로 이전 출력을 취소했습니다.".into()
            }
            Err(error) => format!("PNG 크기 지정 내보내기 실패 · 원본 유지: {error}"),
        };
        live_ink.publish_activation_notice(notice);
    }
}

impl Drop for Request {
    fn drop(&mut self) {
        self.busy.store(false, Ordering::Release);
    }
}
