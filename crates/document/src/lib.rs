#![forbid(unsafe_code)]

mod layers;

pub use layers::{GroupNode, LayerNode, LayerTree, LayerTreeError, LayerTreeNode};
pub use nyatidraw_api::{CanvasSpec, CanvasSpecError};

use nyatidraw_api::{DocumentId, HistoryNodeId};

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DocumentState {
    pub id: DocumentId,
    pub canvas: CanvasSpec,
    pub layers: LayerTree,
    pub history_head: Option<HistoryNodeId>,
}
