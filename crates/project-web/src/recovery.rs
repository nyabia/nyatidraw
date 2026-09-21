use nyatidraw_api::{CanvasSpec, DrawingTool, LayerId};
use nyatidraw_project::{decode_layer_tree, encode_layer_tree};
use nyatidraw_tiles::{TILE_BYTE_LEN, TileKey, TileObject, TileSnapshot};
use nyatidraw_web_core::{BrushSettings, MAX_RESIDENT_TILES, WebDocument, WebPreferences, WebTool};

use crate::{MAX_NTDR_BYTES, WebProject, message};

const MAGIC: &[u8; 8] = b"NYWORK01";
const MAX_METADATA_BYTES: usize = 128 * 1024;
const MAX_REQUEST_BYTES: usize =
    MAX_NTDR_BYTES + MAX_RESIDENT_TILES * (TILE_BYTE_LEN + 25) + MAX_METADATA_BYTES + 512;
const TOOLS: [DrawingTool; 12] = [
    DrawingTool::Move,
    DrawingTool::MoveSelection,
    DrawingTool::Wand,
    DrawingTool::Lasso,
    DrawingTool::RectangleSelection,
    DrawingTool::Pencil,
    DrawingTool::Pen,
    DrawingTool::Brush,
    DrawingTool::Eraser,
    DrawingTool::Fill,
    DrawingTool::Gradient,
    DrawingTool::Eyedropper,
];

pub struct RecoveryCheckpoint {
    epoch: u64,
    revision: u64,
    tiles: TileSnapshot,
}

pub struct RecoveryRequest {
    pub bytes: Vec<u8>,
    pub checkpoint: RecoveryCheckpoint,
}

impl WebProject {
    /// Captures immutable tile differences without compression or opening the database.
    /// # Errors
    /// Rejects live strokes, invalid generations, metadata and resource limits.
    pub fn prepare_recovery(
        &self,
        epoch: u64,
        revision: u64,
        previous: Option<&RecoveryCheckpoint>,
    ) -> Result<RecoveryRequest, String> {
        if self.is_drawing() || epoch == 0 || revision == 0 {
            return Err("Cannot capture an active stroke or invalid recovery revision".into());
        }
        let previous = previous.filter(|previous| previous.epoch == epoch);
        if previous.is_some_and(|previous| revision <= previous.revision) {
            return Err("Recovery revisions must advance".into());
        }
        let mut bytes = Vec::new();
        bytes.extend_from_slice(MAGIC);
        bytes.extend_from_slice(&epoch.to_le_bytes());
        bytes.extend_from_slice(&revision.to_le_bytes());
        bytes.extend_from_slice(&previous.map_or(0, |old| old.revision).to_le_bytes());
        if previous.is_none() {
            put_blob(&mut bytes, &self.backing, MAX_NTDR_BYTES)?;
        }
        let canvas = self.canvas();
        for value in [canvas.width_px, canvas.height_px, canvas.pixels_per_inch] {
            bytes.extend_from_slice(&value.to_le_bytes());
        }
        put_blob(
            &mut bytes,
            &encode_layer_tree(self.layers()).map_err(message)?,
            MAX_METADATA_BYTES,
        )?;
        bytes.extend_from_slice(&self.active_layer().0.to_le_bytes());
        put_preferences(&mut bytes, self.preferences(), self.selected_tool)?;
        let count = u32::try_from(self.snapshot().len()).map_err(message)?;
        bytes.extend_from_slice(&count.to_le_bytes());
        for (key, tile) in self.snapshot().iter() {
            if key.mip != 0 {
                return Err("Unsupported recovery tile mip".into());
            }
            bytes.extend_from_slice(&key.layer.0.to_le_bytes());
            bytes.extend_from_slice(&key.x.to_le_bytes());
            bytes.extend_from_slice(&key.y.to_le_bytes());
            let unchanged = previous
                .and_then(|old| old.tiles.get(key))
                .is_some_and(|old| old.hash() == tile.hash());
            bytes.push(u8::from(!unchanged));
            if !unchanged {
                bytes.extend_from_slice(tile.pixels());
            }
        }
        if bytes.len() > MAX_REQUEST_BYTES {
            return Err("Recovery request too large".into());
        }
        Ok(RecoveryRequest {
            bytes,
            checkpoint: RecoveryCheckpoint {
                epoch,
                revision,
                tiles: self.snapshot().clone(),
            },
        })
    }
}

struct StoredRecovery {
    epoch: u64,
    revision: u64,
    project: WebProject,
}

#[derive(Default)]
pub struct RecoveryWriter {
    accepted: Option<StoredRecovery>,
    staged: Option<StoredRecovery>,
}

impl RecoveryWriter {
    /// Produces native NTDR bytes, but does not accept them before storage acknowledges.
    /// # Errors
    /// Rejects stale bases, malformed packets and failed project serialization atomically.
    pub fn stage(&mut self, packet: &[u8]) -> Result<Vec<u8>, String> {
        if self.staged.is_some() {
            return Err("A recovery write is already staged".into());
        }
        let mut next = read_request(packet, self.accepted.as_ref())?;
        let bytes = next.project.encode_ntdr()?;
        self.staged = Some(next);
        Ok(bytes)
    }

    /// # Errors
    /// Requires a successfully staged and durably written recovery file.
    pub fn accept(&mut self) -> Result<(), String> {
        self.accepted = Some(self.staged.take().ok_or("No staged recovery write")?);
        Ok(())
    }

    pub fn abort(&mut self) {
        self.staged = None;
    }

    /// # Errors
    /// No file exists until the first acknowledged write.
    pub fn file(&self) -> Result<&[u8], String> {
        self.accepted
            .as_ref()
            .map(|state| state.project.backing.as_slice())
            .ok_or_else(|| "No accepted recovery file".into())
    }
}

fn read_request(
    packet: &[u8],
    accepted: Option<&StoredRecovery>,
) -> Result<StoredRecovery, String> {
    if packet.len() > MAX_REQUEST_BYTES {
        return Err("Recovery request too large".into());
    }
    let mut reader = Reader(packet);
    if reader.take(8)? != MAGIC {
        return Err("Unknown recovery protocol".into());
    }
    let epoch = u64::from_le_bytes(reader.array()?);
    let revision = u64::from_le_bytes(reader.array()?);
    let base = u64::from_le_bytes(reader.array()?);
    if epoch == 0 || revision <= base {
        return Err("Invalid recovery revision".into());
    }
    let source = if base == 0 {
        if accepted.is_some_and(|old| epoch <= old.epoch) {
            return Err("Stale project replacement".into());
        }
        let backing = reader.blob(MAX_NTDR_BYTES)?;
        if backing.is_empty() {
            None
        } else {
            Some(WebProject::decode_ntdr(backing)?)
        }
    } else {
        None
    };
    let previous = if base == 0 {
        source.as_ref()
    } else {
        let old = accepted
            .filter(|old| old.epoch == epoch && old.revision == base)
            .ok_or("Recovery base mismatch")?;
        Some(&old.project)
    };
    let canvas = CanvasSpec {
        width_px: reader.u32()?,
        height_px: reader.u32()?,
        pixels_per_inch: reader.u32()?,
    };
    let layers = decode_layer_tree(reader.blob(MAX_METADATA_BYTES)?).map_err(message)?;
    let active = LayerId(u128::from_le_bytes(reader.array()?));
    let (preferences, selected) = read_preferences(&mut reader)?;
    let count = usize::try_from(reader.u32()?).map_err(message)?;
    if count > MAX_RESIDENT_TILES {
        return Err("Too many recovery tiles".into());
    }
    let mut tiles = Vec::with_capacity(count);
    for _ in 0..count {
        let key = TileKey {
            layer: LayerId(u128::from_le_bytes(reader.array()?)),
            mip: 0,
            x: i32::from_le_bytes(reader.array()?),
            y: i32::from_le_bytes(reader.array()?),
        };
        let tile = match reader.byte()? {
            0 if base != 0 => previous
                .and_then(|old| old.snapshot().get(key))
                .cloned()
                .ok_or("Missing unchanged recovery tile")?,
            1 => TileObject::new(reader.take(TILE_BYTE_LEN)?.to_vec())
                .map_err(|error| format!("Invalid recovery tile: {error:?}"))?,
            _ => return Err("Invalid recovery tile encoding".into()),
        };
        tiles.push((key, tile));
    }
    if !reader.0.is_empty() {
        return Err("Trailing recovery bytes".into());
    }
    let tiles = TileSnapshot::from_objects(tiles)
        .map_err(|error| format!("Invalid recovery tiles: {error:?}"))?;
    let mut document =
        WebDocument::from_snapshot(canvas, tiles, layers, active).map_err(message)?;
    document.restore_preferences(preferences).map_err(message)?;
    let mut project = WebProject::from_document(document);
    if let Some(previous) = previous {
        project.backing.clone_from(&previous.backing);
        project.saved_preferences = previous.saved_preferences;
        project.saved_selected_tool = previous.saved_selected_tool;
    }
    project.selected_tool = selected;
    Ok(StoredRecovery {
        epoch,
        revision,
        project,
    })
}

fn put_blob(bytes: &mut Vec<u8>, value: &[u8], limit: usize) -> Result<(), String> {
    if value.len() > limit {
        return Err("Recovery field too large".into());
    }
    bytes.extend_from_slice(&u32::try_from(value.len()).map_err(message)?.to_le_bytes());
    bytes.extend_from_slice(value);
    Ok(())
}

fn put_preferences(
    bytes: &mut Vec<u8>,
    preferences: WebPreferences,
    selected: Option<DrawingTool>,
) -> Result<(), String> {
    let selected = match selected {
        None => u8::MAX,
        Some(tool) => u8::try_from(
            TOOLS
                .iter()
                .position(|candidate| *candidate == tool)
                .ok_or("Unsupported recovery tool")?,
        )
        .map_err(message)?,
    };
    bytes.extend_from_slice(&[selected, preferences.tool as u8]);
    bytes.extend_from_slice(&preferences.foreground);
    bytes.extend_from_slice(&preferences.background);
    for brush in preferences.brushes {
        for value in [brush.size_px, brush.opacity, brush.hardness] {
            bytes.extend_from_slice(&value.to_le_bytes());
        }
        bytes.extend_from_slice(&[
            u8::from(brush.size_pressure),
            u8::from(brush.opacity_pressure),
            brush.smoothing,
        ]);
        bytes.extend_from_slice(&brush.size_minimum_u16.to_le_bytes());
        bytes.extend_from_slice(&brush.opacity_minimum_u16.to_le_bytes());
    }
    Ok(())
}

fn read_preferences(
    reader: &mut Reader<'_>,
) -> Result<(WebPreferences, Option<DrawingTool>), String> {
    let selected = match reader.byte()? {
        u8::MAX => None,
        tag => Some(*TOOLS.get(usize::from(tag)).ok_or("Invalid recovery tool")?),
    };
    let tool = *WebTool::ALL
        .get(usize::from(reader.byte()?))
        .ok_or("Invalid recovery brush")?;
    let foreground = reader.array()?;
    let background = reader.array()?;
    let mut brushes = WebTool::ALL.map(BrushSettings::for_tool);
    for brush in &mut brushes {
        *brush = BrushSettings {
            size_px: f32::from_le_bytes(reader.array()?),
            opacity: f32::from_le_bytes(reader.array()?),
            hardness: f32::from_le_bytes(reader.array()?),
            size_pressure: reader.boolean()?,
            opacity_pressure: reader.boolean()?,
            smoothing: reader.byte()?,
            size_minimum_u16: u16::from_le_bytes(reader.array()?),
            opacity_minimum_u16: u16::from_le_bytes(reader.array()?),
        };
    }
    Ok((
        WebPreferences {
            tool,
            brushes,
            foreground,
            background,
        },
        selected,
    ))
}

struct Reader<'a>(&'a [u8]);
impl<'a> Reader<'a> {
    fn take(&mut self, count: usize) -> Result<&'a [u8], String> {
        let (value, remaining) = self
            .0
            .split_at_checked(count)
            .ok_or("Truncated recovery packet")?;
        self.0 = remaining;
        Ok(value)
    }
    fn array<const N: usize>(&mut self) -> Result<[u8; N], String> {
        self.take(N)?.try_into().map_err(message)
    }
    fn byte(&mut self) -> Result<u8, String> {
        Ok(self.array::<1>()?[0])
    }
    fn u32(&mut self) -> Result<u32, String> {
        Ok(u32::from_le_bytes(self.array()?))
    }
    fn boolean(&mut self) -> Result<bool, String> {
        match self.byte()? {
            0 => Ok(false),
            1 => Ok(true),
            _ => Err("Invalid recovery boolean".into()),
        }
    }
    fn blob(&mut self, limit: usize) -> Result<&'a [u8], String> {
        let length = usize::try_from(self.u32()?).map_err(message)?;
        if length > limit {
            return Err("Recovery field too large".into());
        }
        self.take(length)
    }
}
