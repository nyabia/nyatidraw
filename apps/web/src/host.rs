use std::{
    cell::{Cell, RefCell},
    rc::Rc,
    sync::Arc,
};

use nyatidraw_api::{EditorCommand, Revision, UiProjection};
use nyatidraw_editor_ui::UiBackend;

use crate::{
    adapter::WebUiBackend,
    browser,
    model_state::{ReadableExt, StateOwner},
    runtime::{Editor, RecoveryStatus},
};

pub use crate::browser::can_start_standalone;
pub use crate::runtime::{EditorModal, download_recovery_file};

pub struct BrowserEditor {
    adapter: Rc<WebUiBackend>,
    _state: StateOwner,
    initialization_failed: Cell<bool>,
}

impl BrowserEditor {
    pub fn backend(&self) -> &dyn UiBackend {
        &*self.adapter
    }

    pub fn export_sized_png(&self, size: [u32; 2]) {
        self.adapter.editor.download_scaled_png(size);
    }
    pub fn export_sprite_sheet(&self) {
        crate::sprite_export::download(self.adapter.editor, None);
    }

    #[must_use]
    pub fn new(changed: Rc<dyn Fn()>) -> Self {
        let state = StateOwner::new(changed);
        let editor = Editor {
            runtime: state.field(Rc::new(RefCell::new(None))),
            revision: state.field(0),
            status: state.field("그리기 화면을 준비하는 중…".into()),
            recovery_status: state.field(RecoveryStatus::Idle),
            ready: state.field(false),
            modal: state.field(None),
            notice: state.field(None),
            picker: state.field(None),
            integration_busy: state.field(false),
            integration_closed: state.field(false),
            copying_png: state.field(false),
            exporting_png: state.field(false),
        };
        Self {
            adapter: WebUiBackend::new(&editor),
            _state: state,
            initialization_failed: Cell::new(false),
        }
    }

    pub async fn initialize(&self) {
        let editor = self.adapter.editor;
        match editor.initialize().await {
            Ok(()) => {
                self.adapter.configure_embed_panels();
                self.adapter
                    .set_workspace_appearance(self.adapter.workspace_appearance());
                if browser::is_hosted() {
                    browser::integration_initialized();
                }
            }
            Err(error) => {
                self.initialization_failed.set(true);
                if browser::is_hosted() {
                    browser::integration_error(&error);
                }
                editor.report(error);
            }
        }
    }

    #[must_use]
    pub fn snapshot(&self) -> UiProjection {
        self.adapter.protocol_snapshot().0
    }

    /// # Errors
    /// Returns the active editor's stale revision, busy state or command execution error.
    pub fn dispatch(&self, based_on: Revision, command: EditorCommand) -> Result<(), String> {
        self.adapter.push_editor_command(based_on, command)
    }

    #[must_use]
    pub fn supports_command(&self, command: &EditorCommand) -> bool {
        self.adapter.supports_command(command)
    }

    pub fn navigator_png(&self) -> Option<Arc<str>> {
        self.adapter
            .navigator_snapshot()
            .frame
            .map(|frame| frame.data_uri.clone())
    }

    pub fn project_epoch(&self) -> u64 {
        self.adapter.project_epoch()
    }

    pub fn palette_pins(&self) -> [Option<[u8; 4]>; 10] {
        self.adapter.palette_pins()
    }
    pub fn save_palette_pins(&self, pins: [Option<[u8; 4]>; 10]) {
        self.adapter.save_palette_pins(pins);
    }

    #[must_use]
    pub fn ready(&self) -> bool {
        *self.adapter.editor.ready.peek()
    }
    #[must_use]
    pub fn busy(&self) -> bool {
        *self.adapter.editor.integration_busy.peek()
            || *self.adapter.editor.integration_closed.peek()
    }
    #[must_use]
    pub fn status(&self) -> String {
        self.adapter.editor.status.peek().clone()
    }
    #[must_use]
    pub fn notice(&self) -> Option<String> {
        self.adapter.editor.notice.peek().clone()
    }
    pub fn picker(&self) -> Option<nyatidraw_editor_ui::picker_loupe::PickerSnapshot> {
        self.adapter.editor.picker.peek().clone()
    }
    #[must_use]
    pub fn modal(&self) -> Option<EditorModal> {
        *self.adapter.editor.modal.peek()
    }

    pub fn new_document(&self, width: u32, height: u32) {
        self.adapter.editor.new_document(width, height);
    }
    pub fn show_new(&self) {
        self.adapter.editor.show_modal(EditorModal::NewDocument);
    }
    pub fn open(&self) {
        self.adapter.editor.open();
    }
    pub fn save(&self) {
        self.adapter.editor.download_project();
    }
    pub fn export_png(&self) {
        self.adapter.editor.download_png();
    }
    pub fn copy_png(&self) {
        self.adapter.editor.copy_png();
    }
    pub async fn import_layer(&self) {
        self.adapter.editor.import_png_layer().await;
    }
    pub fn accept_replacement(&self) {
        self.adapter.editor.confirm_replacement();
    }
    pub fn close_modal(&self) {
        self.adapter.editor.close_modal();
    }
    pub fn dismiss_notice(&self) {
        self.adapter.editor.dismiss_notice();
    }
    pub fn complete(&self) {
        self.adapter.editor.complete_integration();
    }
    pub fn show_cancel(&self) {
        self.adapter
            .editor
            .show_modal(EditorModal::CancelIntegration);
    }
    pub fn cancel(&self) {
        self.adapter.editor.close_modal();
        self.adapter.editor.cancel_integration();
    }
    pub fn hosted(&self) -> bool {
        browser::is_hosted()
    }
    pub fn recovery_description(&self) -> &'static str {
        self.adapter.editor.recovery_status.peek().description()
    }
    pub fn recovery_available(&self) -> bool {
        can_start_standalone() && self.initialization_failed.get()
    }
    pub fn download_recovery(&self) {
        self.adapter.editor.download_recovery();
    }
}
