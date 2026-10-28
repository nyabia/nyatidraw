use super::ExportGenerationGate;
use std::{
    path::PathBuf,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
};

#[derive(Default)]
pub(super) struct Lane {
    busy: Arc<AtomicBool>,
    generations: Arc<Mutex<ExportGenerationGate>>,
}

pub(super) struct Request {
    directory: PathBuf,
    columns: Option<u32>,
    generation: u64,
    busy: Arc<AtomicBool>,
    generations: Arc<Mutex<ExportGenerationGate>>,
}

impl Lane {
    pub(super) fn reserve(
        &self,
        directory: PathBuf,
        columns: Option<u32>,
    ) -> Result<Request, String> {
        crate::sprite_export::validate_target(&directory)?;
        if self.busy.swap(true, Ordering::AcqRel) {
            return Err("스프라이트 시트 내보내기가 진행 중입니다.".into());
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
            directory,
            columns,
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
        timeline: Option<&nyatidraw_document::AnimationTimeline>,
        canvas: nyatidraw_api::CanvasSpec,
        live_ink: &crate::live_ink::LiveInkBridge,
    ) {
        live_ink.publish_activation_notice("스프라이트 시트 내보내기 중…".into());
        let result = (|| {
            let timeline = timeline.ok_or("그림에 애니메이션 프레임이 없습니다.")?;
            let sheet = nyatidraw_editor::animation_export::export_sprite_sheet(
                tiles,
                tree,
                timeline,
                canvas,
                self.columns,
            )
            .map_err(|error| error.to_string())?;
            let generations = self
                .generations
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if !generations.is_current(self.generation) {
                return Err("이전 내보내기 요청을 취소했습니다.".into());
            }
            crate::sprite_export::validate_target(&self.directory)?;
            let manifest = nyatidraw_editor::animation_export::sprite_sheet_manifest(&sheet);
            nyatidraw_png_io::publish_sprite_sheet_new(
                &self.directory,
                &sheet.surface,
                manifest.as_bytes(),
            )
        })();
        live_ink.publish_activation_notice(match result {
            Ok(()) => format!(
                "스프라이트 시트 PNG·JSON 저장 완료: {}",
                self.directory.display()
            ),
            Err(error) => format!("스프라이트 시트 내보내기 실패 · 원본 유지: {error}"),
        });
    }
}

impl Drop for Request {
    fn drop(&mut self) {
        self.busy.store(false, Ordering::Release);
    }
}
