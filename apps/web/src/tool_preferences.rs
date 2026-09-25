use base64::{Engine as _, engine::general_purpose::STANDARD};
use nyatidraw_project_web::WebProject;

use crate::browser;

pub struct AppToolPreferences {
    latest: Option<Vec<u8>>,
    writable: bool,
}

impl AppToolPreferences {
    pub fn load() -> (Self, Option<String>) {
        let loaded = browser::load_preference("tools")
            .map_err(|error| browser::error_text(&error))
            .and_then(|value| {
                value
                    .map(|value| {
                        let bytes = STANDARD.decode(value).map_err(|error| error.to_string())?;
                        if !WebProject::valid_tool_preferences(&bytes) {
                            return Err("알 수 없는 도구 설정 형식".into());
                        }
                        Ok(bytes)
                    })
                    .transpose()
            });
        match loaded {
            Ok(latest) => (
                Self {
                    latest,
                    writable: true,
                },
                None,
            ),
            Err(error) => (
                Self {
                    latest: None,
                    writable: false,
                },
                Some(format!(
                    "앱 도구 설정을 읽지 못했습니다. 기존 설정은 보존합니다: {error}"
                )),
            ),
        }
    }

    pub fn restore_fallback(&self, project: &mut WebProject) -> Result<(), String> {
        if let Some(bytes) = &self.latest {
            project.restore_fallback_tool_preferences(bytes)?;
        }
        Ok(())
    }

    pub fn save(&mut self, project: &WebProject) -> Result<(), String> {
        let bytes = project.encode_tool_preferences()?;
        if self.latest.as_ref() == Some(&bytes) {
            return Ok(());
        }
        let encoded = STANDARD.encode(&bytes);
        self.latest = Some(bytes);
        if self.writable
            && let Err(error) = browser::store_preference("tools", &encoded)
        {
            self.writable = false;
            return Err(format!(
                "앱 도구 설정 저장 실패 · 그림 저장과는 별개입니다: {}",
                browser::error_text(&error)
            ));
        }
        Ok(())
    }
}
