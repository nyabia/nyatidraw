use crate::{LayerId, MaskId};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RasterEditTarget {
    Image(LayerId),
    Mask { layer: LayerId, mask: MaskId },
}

impl RasterEditTarget {
    #[must_use]
    pub const fn layer(self) -> LayerId {
        match self {
            Self::Image(layer) | Self::Mask { layer, .. } => layer,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MaskCommand {
    AddRevealAll(LayerId),
    AddFromSelection(LayerId),
    SelectImage(LayerId),
    SelectMask(LayerId),
    SetEnabled { layer: LayerId, enabled: bool },
    FillSelection { layer: LayerId, color: [u8; 4] },
    Remove(LayerId),
}

impl MaskCommand {
    #[must_use]
    pub const fn layer(self) -> LayerId {
        match self {
            Self::AddRevealAll(layer)
            | Self::AddFromSelection(layer)
            | Self::SelectImage(layer)
            | Self::SelectMask(layer)
            | Self::SetEnabled { layer, .. }
            | Self::FillSelection { layer, .. }
            | Self::Remove(layer) => layer,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct LayerMaskProjection {
    pub id: MaskId,
    pub enabled: bool,
    pub default_coverage: u8,
}
