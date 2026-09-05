use std::fmt;

use nyatidraw_api::{ContentRootId, GroupId, HistoryNodeId, LayerId, SnapshotId};
use nyatidraw_brush::{BrushPreset, BrushPresetId, BrushSnapshot, RecordedStroke};
use nyatidraw_document::{GroupNode, LayerNode, LayerTree, LayerTreeNode};
use nyatidraw_history::{HistoryNode, OperationRecord};
use nyatidraw_input::{PenButtons, Point, PointerPhase, StylusSample};
use nyatidraw_stroke::{
    MAX_SAMPLES_PER_STROKE, MaterializedStroke, StrokeColor, StrokeCommit, StrokeCommitError,
    StrokeCommitId,
};
use nyatidraw_tiles::{
    ContentRoot, ObjectHash, TILE_EDGE, TileBounds, TileKey, TileSnapshot, TileSnapshotError,
};

const RECORD_MAGIC: [u8; 8] = *b"NYREC001";
const RECORD_SCHEMA_VERSION: u16 = 1;
const ROOT_ENTRY_ENCODED_LEN: usize = 57;
const SAMPLE_MIN_ENCODED_LEN: usize = 61;
const LAYER_TREE_WIRE_VERSION: u16 = 2;
const MAX_LAYER_TREE_NODES: usize = 4_096;
const MAX_LAYER_TREE_DEPTH: usize = 64;
const MAX_LAYER_NAME_BYTES: usize = 1_024;
/// Upper bound for one encoded layer-tree envelope, including record header.
/// The exact v1 maximum is lower; this leaves format-header slack without
/// permitting hostile projects to force an arbitrary payload allocation.
pub const MAX_LAYER_TREE_RECORD_BYTES: usize = 5 * 1_024 * 1_024;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u8)]
pub enum RecordKind {
    Tile = 1,
    ContentRoot = 2,
    StrokeCommit = 3,
    HistoryNode = 4,
    SnapshotHead = 5,
    HistoryCursor = 6,
    InitialHistoryCursor = 7,
    LayerTree = 8,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Envelope {
    pub kind: RecordKind,
    pub schema_version: u16,
    pub uncompressed_len: u64,
    pub checksum: ObjectHash,
    pub payload: Vec<u8>,
}

impl Envelope {
    #[must_use]
    pub fn new(kind: RecordKind, payload: Vec<u8>) -> Self {
        let checksum =
            ObjectHash::digest_tagged(b"nyatidraw-project-envelope-payload-v1", &payload);
        Self {
            kind,
            schema_version: RECORD_SCHEMA_VERSION,
            uncompressed_len: payload.len() as u64,
            checksum,
            payload,
        }
    }

    #[must_use]
    pub fn encode(&self) -> Vec<u8> {
        let mut output = Vec::with_capacity(self.payload.len().saturating_add(52));
        output.extend_from_slice(&RECORD_MAGIC);
        output.push(self.kind as u8);
        output.extend_from_slice(&self.schema_version.to_le_bytes());
        output.push(0);
        output.extend_from_slice(&self.uncompressed_len.to_le_bytes());
        output.extend_from_slice(&self.checksum.0);
        output.extend_from_slice(&self.payload);
        output
    }

    /// Decodes and verifies a project record envelope.
    ///
    /// # Errors
    ///
    /// Returns an error for invalid magic, schema, codec, length, kind, or checksum.
    pub fn decode(bytes: &[u8], expected_kind: RecordKind) -> Result<Self, WireError> {
        let mut decoder = Decoder::new(bytes);
        if decoder.array::<8>()? != RECORD_MAGIC {
            return Err(WireError::BadMagic);
        }
        let kind = RecordKind::try_from(decoder.u8()?)?;
        if kind != expected_kind {
            return Err(WireError::UnexpectedRecordKind {
                expected: expected_kind,
                actual: kind,
            });
        }
        let schema_version = decoder.u16()?;
        if schema_version != RECORD_SCHEMA_VERSION {
            return Err(WireError::UnsupportedSchema(schema_version));
        }
        let codec = decoder.u8()?;
        if codec != 0 {
            return Err(WireError::UnsupportedCodec(codec));
        }
        let uncompressed_len = decoder.u64()?;
        let checksum = ObjectHash(decoder.array()?);
        let payload = decoder.remaining().to_vec();
        if u64::try_from(payload.len()).ok() != Some(uncompressed_len) {
            return Err(WireError::LengthMismatch);
        }
        let actual = ObjectHash::digest_tagged(b"nyatidraw-project-envelope-payload-v1", &payload);
        if actual != checksum {
            return Err(WireError::ChecksumMismatch);
        }
        Ok(Self {
            kind,
            schema_version,
            uncompressed_len,
            checksum,
            payload,
        })
    }
}

impl TryFrom<u8> for RecordKind {
    type Error = WireError;

    fn try_from(value: u8) -> Result<Self, Self::Error> {
        match value {
            1 => Ok(Self::Tile),
            2 => Ok(Self::ContentRoot),
            3 => Ok(Self::StrokeCommit),
            4 => Ok(Self::HistoryNode),
            5 => Ok(Self::SnapshotHead),
            6 => Ok(Self::HistoryCursor),
            7 => Ok(Self::InitialHistoryCursor),
            8 => Ok(Self::LayerTree),
            _ => Err(WireError::InvalidEnum),
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum CommitBatchError {
    CommitMaterializationMismatch,
    BeforeSnapshotMismatch,
    HistoryOperationMismatch,
    HistoryRootMismatch,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ProjectCommitBatch {
    pub snapshot_id: SnapshotId,
    pub before: TileSnapshot,
    pub stroke: StrokeCommit,
    pub materialized: MaterializedStroke,
    pub history_node: HistoryNode,
}

impl ProjectCommitBatch {
    /// Builds the immutable unit accepted by a project writer.
    ///
    /// # Errors
    ///
    /// Returns an error when the stroke, tile roots, or history operation do
    /// not describe the same materialized transition.
    pub fn new(
        snapshot_id: SnapshotId,
        before: TileSnapshot,
        stroke: StrokeCommit,
        materialized: MaterializedStroke,
        history_node: HistoryNode,
    ) -> Result<Self, CommitBatchError> {
        if stroke.id != materialized.commit_id || stroke.before_root != materialized.before_root {
            return Err(CommitBatchError::CommitMaterializationMismatch);
        }
        if before.root() != stroke.before_root {
            return Err(CommitBatchError::BeforeSnapshotMismatch);
        }
        if history_node.before_root != stroke.before_root.id
            || history_node.after_root != materialized.after.root().id
        {
            return Err(CommitBatchError::HistoryRootMismatch);
        }
        match &history_node.operation {
            OperationRecord::Stroke {
                commit_id,
                recorded,
            } if *commit_id == stroke.id && *recorded == stroke.recorded => {}
            _ => return Err(CommitBatchError::HistoryOperationMismatch),
        }
        Ok(Self {
            snapshot_id,
            before,
            stroke,
            materialized,
            history_node,
        })
    }

    #[must_use]
    pub fn head(&self) -> ProjectHead {
        ProjectHead {
            snapshot_id: self.snapshot_id,
            before_root: self.before.root(),
            after_root: self.materialized.after.root(),
            history_head: self.history_node.id,
            stroke_commit: Some(self.stroke.id),
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ProjectHead {
    pub snapshot_id: SnapshotId,
    pub before_root: ContentRoot,
    pub after_root: ContentRoot,
    pub history_head: HistoryNodeId,
    pub stroke_commit: Option<StrokeCommitId>,
}

/// A durable content-root transition whose semantics are structural rather
/// than a replayable pen stroke (for example, materializing a white base
/// raster). The exact after-root remains the durable artwork authority.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ProjectStructuralBatch {
    pub snapshot_id: SnapshotId,
    pub before: TileSnapshot,
    pub after: TileSnapshot,
    pub history_node: HistoryNode,
}

impl ProjectStructuralBatch {
    /// # Errors
    ///
    /// Returns an error when the node does not describe the supplied content
    /// root transition as a structural operation.
    pub fn new(
        snapshot_id: SnapshotId,
        before: TileSnapshot,
        after: TileSnapshot,
        history_node: HistoryNode,
    ) -> Result<Self, CommitBatchError> {
        if history_node.before_root != before.root().id
            || history_node.after_root != after.root().id
        {
            return Err(CommitBatchError::HistoryRootMismatch);
        }
        if !matches!(history_node.operation, OperationRecord::StructuralChange) {
            return Err(CommitBatchError::HistoryOperationMismatch);
        }
        Ok(Self {
            snapshot_id,
            before,
            after,
            history_node,
        })
    }

    #[must_use]
    pub fn head(&self) -> ProjectHead {
        ProjectHead {
            snapshot_id: self.snapshot_id,
            before_root: self.before.root(),
            after_root: self.after.root(),
            history_head: self.history_node.id,
            stroke_commit: None,
        }
    }
}

#[must_use]
pub fn encode_history_cursor(
    snapshot_id: SnapshotId,
    history_head: HistoryNodeId,
    root: ContentRoot,
) -> Vec<u8> {
    let mut output = Vec::new();
    output.extend_from_slice(&snapshot_id.0.to_le_bytes());
    output.extend_from_slice(&history_head.0.to_le_bytes());
    encode_content_root(&mut output, root);
    output
}

/// # Errors
///
/// Returns an error when the cursor payload is malformed.
pub fn decode_history_cursor(bytes: &[u8]) -> Result<crate::ProjectHistoryCursor, WireError> {
    let mut decoder = Decoder::new(bytes);
    let cursor = crate::ProjectHistoryCursor {
        snapshot_id: SnapshotId(decoder.u128()?),
        history_head: Some(HistoryNodeId(decoder.u128()?)),
        root: decode_content_root(&mut decoder)?,
    };
    decoder.finish()?;
    Ok(cursor)
}

#[must_use]
pub fn encode_initial_history_cursor(snapshot_id: SnapshotId, root: ContentRoot) -> Vec<u8> {
    let mut output = Vec::new();
    output.extend_from_slice(&snapshot_id.0.to_le_bytes());
    encode_content_root(&mut output, root);
    output
}

/// # Errors
///
/// Returns an error when the initial cursor payload is malformed.
pub fn decode_initial_history_cursor(
    bytes: &[u8],
) -> Result<crate::ProjectHistoryCursor, WireError> {
    let mut decoder = Decoder::new(bytes);
    let cursor = crate::ProjectHistoryCursor {
        snapshot_id: SnapshotId(decoder.u128()?),
        history_head: None,
        root: decode_content_root(&mut decoder)?,
    };
    decoder.finish()?;
    Ok(cursor)
}

/// Encodes the validated layer hierarchy in a bounded, versioned wire format.
///
/// This is deliberately not a memory-layout serialization of the Rust domain
/// types. Every enum tag, integer width, string length, and collection bound
/// is part of the project format contract.
///
/// # Errors
///
/// Returns an error when a layer name or total node count exceeds the project
/// format limits.
pub fn encode_layer_tree(tree: &LayerTree) -> Result<Vec<u8>, WireError> {
    let mut output = Vec::new();
    output.extend_from_slice(&LAYER_TREE_WIRE_VERSION.to_le_bytes());
    let mut node_count = 0_usize;
    encode_group_node(&mut output, tree.root(), 0, &mut node_count)?;
    Ok(output)
}

/// Decodes and validates a bounded layer hierarchy.
///
/// # Errors
///
/// Returns an error for unsupported versions, malformed UTF-8, excessive
/// depth/count/name length, invalid tags, duplicate IDs, or mutable root
/// properties.
pub fn decode_layer_tree(bytes: &[u8]) -> Result<LayerTree, WireError> {
    let mut decoder = Decoder::new(bytes);
    let version = decoder.u16()?;
    if !(1..=LAYER_TREE_WIRE_VERSION).contains(&version) {
        return Err(WireError::InvalidData("unsupported layer tree version"));
    }
    let mut node_count = 0_usize;
    let root = decode_group_node(&mut decoder, 0, &mut node_count, version)?;
    decoder.finish()?;
    LayerTree::new(root).map_err(|_| WireError::InvalidData("invalid layer tree hierarchy"))
}

fn encode_group_node(
    output: &mut Vec<u8>,
    group: &GroupNode,
    depth: usize,
    node_count: &mut usize,
) -> Result<(), WireError> {
    if depth > MAX_LAYER_TREE_DEPTH {
        return Err(WireError::InvalidData("layer tree exceeds depth limit"));
    }
    count_layer_node(node_count)?;
    output.push(2);
    output.extend_from_slice(&group.id.0.to_le_bytes());
    encode_layer_name(output, &group.name)?;
    output.push(u8::from(group.visible));
    output.extend_from_slice(&group.opacity_u16.to_le_bytes());
    let child_count = u64::try_from(group.children.len())
        .map_err(|_| WireError::InvalidData("layer child count exceeds format"))?;
    output.extend_from_slice(&child_count.to_le_bytes());
    for child in &group.children {
        match child {
            LayerTreeNode::Raster(layer) => encode_raster_node(output, layer, node_count)?,
            LayerTreeNode::Group(child) => {
                encode_group_node(output, child, depth + 1, node_count)?;
            }
        }
    }
    Ok(())
}

fn encode_raster_node(
    output: &mut Vec<u8>,
    layer: &LayerNode,
    node_count: &mut usize,
) -> Result<(), WireError> {
    count_layer_node(node_count)?;
    output.push(1);
    output.extend_from_slice(&layer.id.0.to_le_bytes());
    encode_layer_name(output, &layer.name)?;
    output.push(u8::from(layer.visible));
    output.push(u8::from(layer.locked));
    output.push(u8::from(layer.reference));
    output.extend_from_slice(&layer.opacity_u16.to_le_bytes());
    output.extend_from_slice(&layer.content_root.0.to_le_bytes());
    Ok(())
}

fn count_layer_node(node_count: &mut usize) -> Result<(), WireError> {
    *node_count = node_count
        .checked_add(1)
        .ok_or(WireError::InvalidData("layer node count overflow"))?;
    if *node_count > MAX_LAYER_TREE_NODES {
        return Err(WireError::InvalidData("layer tree exceeds node limit"));
    }
    Ok(())
}

fn encode_layer_name(output: &mut Vec<u8>, name: &str) -> Result<(), WireError> {
    if name.len() > MAX_LAYER_NAME_BYTES {
        return Err(WireError::InvalidData("layer name exceeds byte limit"));
    }
    output.extend_from_slice(&(name.len() as u64).to_le_bytes());
    output.extend_from_slice(name.as_bytes());
    Ok(())
}

fn decode_group_node(
    decoder: &mut Decoder<'_>,
    depth: usize,
    node_count: &mut usize,
    version: u16,
) -> Result<GroupNode, WireError> {
    if depth > MAX_LAYER_TREE_DEPTH {
        return Err(WireError::InvalidData("layer tree exceeds depth limit"));
    }
    count_layer_node(node_count)?;
    if decoder.u8()? != 2 {
        return Err(WireError::InvalidData(
            "layer tree root/child is not a group",
        ));
    }
    let id = GroupId(decoder.u128()?);
    let name = decoder.bounded_string(MAX_LAYER_NAME_BYTES, "layer name exceeds byte limit")?;
    let visible = decoder.boolean()?;
    let opacity_u16 = decoder.u16()?;
    let remaining_nodes = MAX_LAYER_TREE_NODES - *node_count;
    let child_count = decoder.bounded_len(remaining_nodes, "layer child count exceeds limit")?;
    let mut children = Vec::with_capacity(child_count);
    for _ in 0..child_count {
        match decoder.peek_u8()? {
            1 => children.push(LayerTreeNode::Raster(decode_raster_node(
                decoder, node_count, version,
            )?)),
            2 => children.push(LayerTreeNode::Group(decode_group_node(
                decoder,
                depth + 1,
                node_count,
                version,
            )?)),
            _ => return Err(WireError::InvalidEnum),
        }
    }
    Ok(GroupNode {
        id,
        name,
        visible,
        opacity_u16,
        children,
    })
}

fn decode_raster_node(
    decoder: &mut Decoder<'_>,
    node_count: &mut usize,
    version: u16,
) -> Result<LayerNode, WireError> {
    count_layer_node(node_count)?;
    if decoder.u8()? != 1 {
        return Err(WireError::InvalidData("layer child is not raster"));
    }
    Ok(LayerNode {
        id: LayerId(decoder.u128()?),
        name: decoder.bounded_string(MAX_LAYER_NAME_BYTES, "layer name exceeds byte limit")?,
        visible: decoder.boolean()?,
        locked: decoder.boolean()?,
        reference: version >= 2 && decoder.boolean()?,
        opacity_u16: decoder.u16()?,
        content_root: ContentRootId(decoder.u128()?),
    })
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RootManifest {
    pub root: ContentRoot,
    pub tiles: Vec<(TileKey, ObjectHash)>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum WireError {
    UnexpectedEof,
    TrailingBytes,
    BadMagic,
    UnexpectedRecordKind {
        expected: RecordKind,
        actual: RecordKind,
    },
    UnsupportedSchema(u16),
    UnsupportedCodec(u8),
    LengthMismatch,
    ChecksumMismatch,
    InvalidEnum,
    InvalidData(&'static str),
    RootMismatch,
    CommitMismatch,
    HistoryMismatch,
    TileSnapshot(TileSnapshotError),
    StrokeCommit(StrokeCommitError),
}

impl fmt::Display for WireError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{self:?}")
    }
}

impl std::error::Error for WireError {}

#[must_use]
pub fn encode_root_manifest(snapshot: &TileSnapshot) -> Vec<u8> {
    let mut output = Vec::new();
    encode_content_root(&mut output, snapshot.root());
    output.extend_from_slice(&TILE_EDGE.to_le_bytes());
    output.extend_from_slice(
        &u64::try_from(snapshot.len())
            .unwrap_or(u64::MAX)
            .to_le_bytes(),
    );
    for (key, tile) in snapshot.iter() {
        encode_tile_key(&mut output, key);
        output.extend_from_slice(&tile.hash().0);
    }
    output
}

/// # Errors
///
/// Returns an error when the manifest is malformed or uses another tile edge.
pub fn decode_root_manifest(bytes: &[u8]) -> Result<RootManifest, WireError> {
    let mut decoder = Decoder::new(bytes);
    let root = decode_content_root(&mut decoder)?;
    if decoder.u32()? != TILE_EDGE {
        return Err(WireError::InvalidData("unsupported tile edge"));
    }
    let count = decoder.bounded_len(
        decoder.remaining_len() / ROOT_ENTRY_ENCODED_LEN,
        "root entry count exceeds payload",
    )?;
    let mut tiles = Vec::with_capacity(count);
    for _ in 0..count {
        tiles.push((decode_tile_key(&mut decoder)?, ObjectHash(decoder.array()?)));
    }
    decoder.finish()?;
    if tiles.windows(2).any(|pair| pair[0].0 >= pair[1].0) {
        return Err(WireError::InvalidData("root entries not canonical"));
    }
    Ok(RootManifest { root, tiles })
}

#[must_use]
pub fn encode_stroke_commit(commit: &StrokeCommit) -> Vec<u8> {
    let mut output = Vec::new();
    output.extend_from_slice(&(commit.id.0).0);
    output.extend_from_slice(&commit.parent_snapshot.0.to_le_bytes());
    output.extend_from_slice(&commit.layer.0.to_le_bytes());
    encode_content_root(&mut output, commit.before_root);
    encode_bounds(&mut output, commit.affected_tiles);
    encode_brush_preset(&mut output, commit.brush.preset);
    encode_recorded_stroke(&mut output, &commit.recorded);
    output.extend_from_slice(&commit.color.0);
    output.extend_from_slice(
        &u64::try_from(commit.samples().len())
            .unwrap_or(u64::MAX)
            .to_le_bytes(),
    );
    for sample in commit.samples() {
        encode_sample(&mut output, sample);
    }
    output
}

/// Decodes and re-seals a semantic stroke against its stored before snapshot.
///
/// # Errors
///
/// Returns an error when the payload is malformed or deterministic sealing no
/// longer produces the stored commit identifier and bounds.
pub fn decode_stroke_commit(
    bytes: &[u8],
    before: &TileSnapshot,
) -> Result<StrokeCommit, WireError> {
    let mut decoder = Decoder::new(bytes);
    let expected_id = StrokeCommitId(ObjectHash(decoder.array()?));
    let parent_snapshot = SnapshotId(decoder.u128()?);
    let layer = LayerId(decoder.u128()?);
    let before_root = decode_content_root(&mut decoder)?;
    let affected_tiles = decode_bounds(&mut decoder)?;
    let brush = BrushSnapshot {
        preset: decode_brush_preset(&mut decoder)?,
    };
    let recorded = decode_recorded_stroke(&mut decoder)?;
    let color = StrokeColor::new(decoder.array()?).map_err(WireError::StrokeCommit)?;
    let sample_count = decoder.bounded_len(
        MAX_SAMPLES_PER_STROKE.min(decoder.remaining_len() / SAMPLE_MIN_ENCODED_LEN),
        "sample count exceeds bounded payload",
    )?;
    let mut samples = Vec::with_capacity(sample_count);
    for _ in 0..sample_count {
        samples.push(decode_sample(&mut decoder)?);
    }
    decoder.finish()?;
    if before.root() != before_root {
        return Err(WireError::RootMismatch);
    }
    let commit = StrokeCommit::seal(
        parent_snapshot,
        layer,
        before,
        brush,
        recorded,
        color,
        samples,
    )
    .map_err(WireError::StrokeCommit)?;
    if commit.id != expected_id || commit.affected_tiles != affected_tiles {
        return Err(WireError::CommitMismatch);
    }
    Ok(commit)
}

#[must_use]
pub fn encode_history_node(node: &HistoryNode) -> Vec<u8> {
    let mut output = Vec::new();
    output.extend_from_slice(&node.id.0.to_le_bytes());
    encode_optional_u128(&mut output, node.parent.map(|id| id.0));
    output.extend_from_slice(&node.timestamp_ns.to_le_bytes());
    match &node.operation {
        OperationRecord::Stroke {
            commit_id,
            recorded,
        } => {
            output.push(1);
            output.extend_from_slice(&(commit_id.0).0);
            encode_recorded_stroke(&mut output, recorded);
        }
        OperationRecord::StructuralChange => output.push(2),
    }
    output.extend_from_slice(&node.before_root.0.to_le_bytes());
    output.extend_from_slice(&node.after_root.0.to_le_bytes());
    output
}

/// # Errors
///
/// Returns an error when a history node payload is malformed.
pub fn decode_history_node(bytes: &[u8]) -> Result<HistoryNode, WireError> {
    let mut decoder = Decoder::new(bytes);
    let id = HistoryNodeId(decoder.u128()?);
    let parent = decode_optional_u128(&mut decoder)?.map(HistoryNodeId);
    let timestamp_ns = decoder.u64()?;
    let operation = match decoder.u8()? {
        1 => OperationRecord::Stroke {
            commit_id: StrokeCommitId(ObjectHash(decoder.array()?)),
            recorded: decode_recorded_stroke(&mut decoder)?,
        },
        2 => OperationRecord::StructuralChange,
        _ => return Err(WireError::InvalidEnum),
    };
    let before_root = ContentRootId(decoder.u128()?);
    let after_root = ContentRootId(decoder.u128()?);
    decoder.finish()?;
    Ok(HistoryNode {
        id,
        parent,
        timestamp_ns,
        operation,
        before_root,
        after_root,
    })
}

#[must_use]
pub fn encode_project_head(head: ProjectHead) -> Vec<u8> {
    let mut output = Vec::new();
    output.extend_from_slice(&head.snapshot_id.0.to_le_bytes());
    encode_content_root(&mut output, head.before_root);
    encode_content_root(&mut output, head.after_root);
    output.extend_from_slice(&head.history_head.0.to_le_bytes());
    match head.stroke_commit {
        Some(commit) => {
            output.push(1);
            output.extend_from_slice(&(commit.0).0);
        }
        None => output.push(0),
    }
    output
}

/// # Errors
///
/// Returns an error when a snapshot head payload is malformed.
pub fn decode_project_head(bytes: &[u8]) -> Result<ProjectHead, WireError> {
    let mut decoder = Decoder::new(bytes);
    let snapshot_id = SnapshotId(decoder.u128()?);
    let before_root = decode_content_root(&mut decoder)?;
    let after_root = decode_content_root(&mut decoder)?;
    let history_head = HistoryNodeId(decoder.u128()?);
    // The original Sprint 2 payload ended with the 32-byte stroke ID. Sprint 3
    // added an explicit option tag so structural heads can omit it. Accept the
    // old fixed-width tail without weakening the strict lengths of either new
    // representation.
    let stroke_commit = match decoder.remaining_len() {
        32 => Some(StrokeCommitId(ObjectHash(decoder.array()?))),
        1 | 33 => match decoder.u8()? {
            0 => None,
            1 => Some(StrokeCommitId(ObjectHash(decoder.array()?))),
            _ => return Err(WireError::InvalidEnum),
        },
        _ => return Err(WireError::LengthMismatch),
    };
    let head = ProjectHead {
        snapshot_id,
        before_root,
        after_root,
        history_head,
        stroke_commit,
    };
    decoder.finish()?;
    Ok(head)
}

fn encode_content_root(output: &mut Vec<u8>, root: ContentRoot) {
    output.extend_from_slice(&root.id.0.to_le_bytes());
    output.extend_from_slice(&root.hash.0);
}

fn decode_content_root(decoder: &mut Decoder<'_>) -> Result<ContentRoot, WireError> {
    let root = ContentRoot {
        id: ContentRootId(decoder.u128()?),
        hash: ObjectHash(decoder.array()?),
    };
    if root.hash.content_root_id() != root.id {
        return Err(WireError::RootMismatch);
    }
    Ok(root)
}

fn encode_bounds(output: &mut Vec<u8>, bounds: TileBounds) {
    output.extend_from_slice(&bounds.min_x.to_le_bytes());
    output.extend_from_slice(&bounds.min_y.to_le_bytes());
    output.extend_from_slice(&bounds.max_x_exclusive.to_le_bytes());
    output.extend_from_slice(&bounds.max_y_exclusive.to_le_bytes());
}

fn decode_bounds(decoder: &mut Decoder<'_>) -> Result<TileBounds, WireError> {
    let bounds = TileBounds {
        min_x: decoder.i32()?,
        min_y: decoder.i32()?,
        max_x_exclusive: decoder.i32()?,
        max_y_exclusive: decoder.i32()?,
    };
    if bounds.min_x >= bounds.max_x_exclusive || bounds.min_y >= bounds.max_y_exclusive {
        return Err(WireError::InvalidData("invalid tile bounds"));
    }
    Ok(bounds)
}

fn encode_brush_preset(output: &mut Vec<u8>, preset: BrushPreset) {
    output.extend_from_slice(&preset.id.0.to_le_bytes());
    output.extend_from_slice(&preset.schema_version.to_le_bytes());
    output.extend_from_slice(&preset.engine_version.to_le_bytes());
    output.extend_from_slice(&preset.size_px.to_bits().to_le_bytes());
    output.extend_from_slice(&preset.opacity.to_bits().to_le_bytes());
    output.extend_from_slice(&preset.flow.to_bits().to_le_bytes());
    output.extend_from_slice(&preset.spacing_ratio.to_bits().to_le_bytes());
}

fn decode_brush_preset(decoder: &mut Decoder<'_>) -> Result<BrushPreset, WireError> {
    Ok(BrushPreset {
        id: BrushPresetId(decoder.u128()?),
        schema_version: decoder.u32()?,
        engine_version: decoder.u32()?,
        size_px: f32::from_bits(decoder.u32()?),
        opacity: f32::from_bits(decoder.u32()?),
        flow: f32::from_bits(decoder.u32()?),
        spacing_ratio: f32::from_bits(decoder.u32()?),
    })
}

fn encode_recorded_stroke(output: &mut Vec<u8>, recorded: &RecordedStroke) {
    output.extend_from_slice(&recorded.brush_engine_version.to_le_bytes());
    output.extend_from_slice(&recorded.preset_id.0.to_le_bytes());
    output.extend_from_slice(&recorded.preset_schema_version.to_le_bytes());
    output.extend_from_slice(&recorded.random_seed.to_le_bytes());
    output.extend_from_slice(&recorded.sample_count.to_le_bytes());
    output.extend_from_slice(&recorded.first_sample_sequence.to_le_bytes());
    output.extend_from_slice(&recorded.last_sample_sequence.to_le_bytes());
}

fn decode_recorded_stroke(decoder: &mut Decoder<'_>) -> Result<RecordedStroke, WireError> {
    Ok(RecordedStroke {
        brush_engine_version: decoder.u32()?,
        preset_id: BrushPresetId(decoder.u128()?),
        preset_schema_version: decoder.u32()?,
        random_seed: decoder.u64()?,
        sample_count: decoder.u64()?,
        first_sample_sequence: decoder.u64()?,
        last_sample_sequence: decoder.u64()?,
    })
}

fn encode_tile_key(output: &mut Vec<u8>, key: TileKey) {
    output.extend_from_slice(&key.layer.0.to_le_bytes());
    output.push(key.mip);
    output.extend_from_slice(&key.x.to_le_bytes());
    output.extend_from_slice(&key.y.to_le_bytes());
}

fn decode_tile_key(decoder: &mut Decoder<'_>) -> Result<TileKey, WireError> {
    Ok(TileKey {
        layer: LayerId(decoder.u128()?),
        mip: decoder.u8()?,
        x: decoder.i32()?,
        y: decoder.i32()?,
    })
}

fn encode_sample(output: &mut Vec<u8>, sample: &StylusSample) {
    output.extend_from_slice(&sample.sequence.to_le_bytes());
    output.extend_from_slice(&sample.timestamp_ns.to_le_bytes());
    output.extend_from_slice(&sample.device_id.to_le_bytes());
    output.push(match sample.phase {
        PointerPhase::Begin => 0,
        PointerPhase::Move => 1,
        PointerPhase::End => 2,
        PointerPhase::Cancel => 3,
    });
    output.extend_from_slice(&sample.position_document.x.to_bits().to_le_bytes());
    output.extend_from_slice(&sample.position_document.y.to_bits().to_le_bytes());
    output.extend_from_slice(&sample.pressure.to_bits().to_le_bytes());
    encode_optional_pair(output, sample.tilt);
    encode_optional_f32(output, sample.twist_radians);
    encode_optional_f32(output, sample.tangential_pressure);
    output.extend_from_slice(&sample.buttons.0.to_le_bytes());
    output.push(u8::from(sample.eraser));
    output.extend_from_slice(&sample.viewport_revision.to_le_bytes());
}

fn decode_sample(decoder: &mut Decoder<'_>) -> Result<StylusSample, WireError> {
    let sequence = decoder.u64()?;
    let timestamp_ns = decoder.u64()?;
    let device_id = decoder.u64()?;
    let phase = match decoder.u8()? {
        0 => PointerPhase::Begin,
        1 => PointerPhase::Move,
        2 => PointerPhase::End,
        3 => PointerPhase::Cancel,
        _ => return Err(WireError::InvalidEnum),
    };
    let position_document = Point {
        x: f64::from_bits(decoder.u64()?),
        y: f64::from_bits(decoder.u64()?),
    };
    let pressure = f32::from_bits(decoder.u32()?);
    let tilt = decode_optional_pair(decoder)?;
    let twist_radians = decode_optional_f32(decoder)?;
    let tangential_pressure = decode_optional_f32(decoder)?;
    let buttons = PenButtons(decoder.u32()?);
    let eraser = match decoder.u8()? {
        0 => false,
        1 => true,
        _ => return Err(WireError::InvalidEnum),
    };
    let viewport_revision = decoder.u64()?;
    Ok(StylusSample {
        sequence,
        timestamp_ns,
        device_id,
        phase,
        position_document,
        pressure,
        tilt,
        twist_radians,
        tangential_pressure,
        buttons,
        eraser,
        viewport_revision,
    })
}

fn encode_optional_u128(output: &mut Vec<u8>, value: Option<u128>) {
    match value {
        Some(value) => {
            output.push(1);
            output.extend_from_slice(&value.to_le_bytes());
        }
        None => output.push(0),
    }
}

fn decode_optional_u128(decoder: &mut Decoder<'_>) -> Result<Option<u128>, WireError> {
    match decoder.u8()? {
        0 => Ok(None),
        1 => Ok(Some(decoder.u128()?)),
        _ => Err(WireError::InvalidEnum),
    }
}

fn encode_optional_pair(output: &mut Vec<u8>, value: Option<[f32; 2]>) {
    match value {
        Some([x, y]) => {
            output.push(1);
            output.extend_from_slice(&x.to_bits().to_le_bytes());
            output.extend_from_slice(&y.to_bits().to_le_bytes());
        }
        None => output.push(0),
    }
}

fn decode_optional_pair(decoder: &mut Decoder<'_>) -> Result<Option<[f32; 2]>, WireError> {
    match decoder.u8()? {
        0 => Ok(None),
        1 => Ok(Some([
            f32::from_bits(decoder.u32()?),
            f32::from_bits(decoder.u32()?),
        ])),
        _ => Err(WireError::InvalidEnum),
    }
}

fn encode_optional_f32(output: &mut Vec<u8>, value: Option<f32>) {
    match value {
        Some(value) => {
            output.push(1);
            output.extend_from_slice(&value.to_bits().to_le_bytes());
        }
        None => output.push(0),
    }
}

fn decode_optional_f32(decoder: &mut Decoder<'_>) -> Result<Option<f32>, WireError> {
    match decoder.u8()? {
        0 => Ok(None),
        1 => Ok(Some(f32::from_bits(decoder.u32()?))),
        _ => Err(WireError::InvalidEnum),
    }
}

struct Decoder<'a> {
    bytes: &'a [u8],
    position: usize,
}

impl<'a> Decoder<'a> {
    const fn new(bytes: &'a [u8]) -> Self {
        Self { bytes, position: 0 }
    }

    fn array<const N: usize>(&mut self) -> Result<[u8; N], WireError> {
        let end = self
            .position
            .checked_add(N)
            .ok_or(WireError::UnexpectedEof)?;
        let bytes = self
            .bytes
            .get(self.position..end)
            .ok_or(WireError::UnexpectedEof)?;
        self.position = end;
        bytes.try_into().map_err(|_| WireError::UnexpectedEof)
    }

    fn u8(&mut self) -> Result<u8, WireError> {
        Ok(self.array::<1>()?[0])
    }

    fn peek_u8(&self) -> Result<u8, WireError> {
        self.bytes
            .get(self.position)
            .copied()
            .ok_or(WireError::UnexpectedEof)
    }

    fn boolean(&mut self) -> Result<bool, WireError> {
        match self.u8()? {
            0 => Ok(false),
            1 => Ok(true),
            _ => Err(WireError::InvalidEnum),
        }
    }

    fn u16(&mut self) -> Result<u16, WireError> {
        Ok(u16::from_le_bytes(self.array()?))
    }

    fn u32(&mut self) -> Result<u32, WireError> {
        Ok(u32::from_le_bytes(self.array()?))
    }

    fn i32(&mut self) -> Result<i32, WireError> {
        Ok(i32::from_le_bytes(self.array()?))
    }

    fn u64(&mut self) -> Result<u64, WireError> {
        Ok(u64::from_le_bytes(self.array()?))
    }

    fn u128(&mut self) -> Result<u128, WireError> {
        Ok(u128::from_le_bytes(self.array()?))
    }

    fn bounded_len(&mut self, maximum: usize, message: &'static str) -> Result<usize, WireError> {
        let length = usize::try_from(self.u64()?).map_err(|_| WireError::LengthMismatch)?;
        if length > maximum {
            return Err(WireError::InvalidData(message));
        }
        Ok(length)
    }

    fn bounded_string(
        &mut self,
        maximum: usize,
        message: &'static str,
    ) -> Result<String, WireError> {
        let length = self.bounded_len(maximum, message)?;
        let end = self
            .position
            .checked_add(length)
            .ok_or(WireError::UnexpectedEof)?;
        let bytes = self
            .bytes
            .get(self.position..end)
            .ok_or(WireError::UnexpectedEof)?;
        self.position = end;
        std::str::from_utf8(bytes)
            .map(str::to_owned)
            .map_err(|_| WireError::InvalidData("layer name is not UTF-8"))
    }

    const fn remaining_len(&self) -> usize {
        self.bytes.len() - self.position
    }

    fn remaining(&mut self) -> &'a [u8] {
        let remaining = &self.bytes[self.position..];
        self.position = self.bytes.len();
        remaining
    }

    fn finish(self) -> Result<(), WireError> {
        if self.position == self.bytes.len() {
            Ok(())
        } else {
            Err(WireError::TrailingBytes)
        }
    }
}

#[cfg(test)]
mod project_head_compatibility_tests {
    use super::*;

    #[test]
    fn reference_metadata_preserves_legacy_layers_and_rejects_malformed_membership() {
        // Product risk: a new source flag must not reinterpret existing layer
        // bytes or silently select the wrong pixels in later selection/fill.
        // This v1 fixture uses explicit fields, independently of the encoder.
        let mut legacy = vec![1, 0, 2];
        legacy.extend_from_slice(&100_u128.to_le_bytes());
        legacy.extend_from_slice(&1_u64.to_le_bytes());
        legacy.extend_from_slice(b"R");
        legacy.extend_from_slice(&[1, 255, 255]);
        legacy.extend_from_slice(&1_u64.to_le_bytes());
        legacy.push(1);
        legacy.extend_from_slice(&7_u128.to_le_bytes());
        legacy.extend_from_slice(&1_u64.to_le_bytes());
        legacy.extend_from_slice(b"L");
        legacy.extend_from_slice(&[1, 0, 255, 255]);
        legacy.extend_from_slice(&0_u128.to_le_bytes());
        let mut tree = decode_layer_tree(&legacy).expect("legacy layer remains readable");
        let LayerTreeNode::Raster(layer) = &tree.root().children[0] else {
            panic!("raster fixture")
        };
        assert!(!layer.reference);
        assert_eq!(layer.id, LayerId(7));
        let prior = tree.clone();
        assert!(tree.set_reference(LayerId(99), true).is_err());
        assert_eq!(tree, prior, "unknown source must preserve artwork metadata");
        tree.set_reference(LayerId(7), true).expect("select source");
        let current = encode_layer_tree(&tree).expect("v2 source encoding");
        assert_eq!(&current[..2], &[2, 0]);
        assert_eq!(decode_layer_tree(&current), Ok(tree));
        let reference_offset = current.len() - 19;
        for invalid in [2, 255] {
            let mut corrupt = current.clone();
            corrupt[reference_offset] = invalid;
            assert_eq!(decode_layer_tree(&corrupt), Err(WireError::InvalidEnum));
        }
        let mut unsupported = current;
        unsupported[..2].copy_from_slice(&3_u16.to_le_bytes());
        assert!(decode_layer_tree(&unsupported).is_err());
    }

    #[test]
    fn snapshot_head_decoder_preserves_legacy_stroke_and_new_structural_forms() {
        // Product risk: adding structural history must not make an existing
        // stroke-only project look corrupt on its next open.
        let stroke = StrokeCommitId(ObjectHash([0x55; 32]));
        let before_hash = ObjectHash([0x11; 32]);
        let after_hash = ObjectHash([0x22; 32]);
        let stroke_head = ProjectHead {
            snapshot_id: SnapshotId(7),
            before_root: ContentRoot {
                id: before_hash.content_root_id(),
                hash: before_hash,
            },
            after_root: ContentRoot {
                id: after_hash.content_root_id(),
                hash: after_hash,
            },
            history_head: HistoryNodeId(10),
            stroke_commit: Some(stroke),
        };
        let current = encode_project_head(stroke_head);
        let mut legacy = current.clone();
        legacy.remove(current.len() - 33);
        assert_eq!(decode_project_head(&legacy), Ok(stroke_head));
        assert_eq!(decode_project_head(&current), Ok(stroke_head));

        let structural_head = ProjectHead {
            stroke_commit: None,
            ..stroke_head
        };
        assert_eq!(
            decode_project_head(&encode_project_head(structural_head)),
            Ok(structural_head)
        );
    }
}
