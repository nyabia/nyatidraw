use crate::FrameId;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AnimationCommand {
    Enable,
    Select(FrameId),
    InsertBlank { after: FrameId },
    Duplicate(FrameId),
    Delete(FrameId),
    Move { frame: FrameId, index: u16 },
    SetDuration { frame: FrameId, duration_ms: u32 },
    Play,
    Stop,
    SetLoop(bool),
    SetOnionSkin(bool),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct AnimationFrameProjection {
    pub id: FrameId,
    pub duration_ms: u32,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AnimationProjection {
    pub frames: Vec<AnimationFrameProjection>,
    pub active_frame: Option<FrameId>,
    pub playing: bool,
    pub looping: bool,
    pub onion_skin: bool,
}

impl Default for AnimationProjection {
    fn default() -> Self {
        Self {
            frames: Vec::new(),
            active_frame: None,
            playing: false,
            looping: true,
            onion_skin: false,
        }
    }
}
