//! Portable browser snapshots, deliberately distinct from desktop `.ntdr`.
use std::collections::BTreeSet;

use nyatidraw_tiles::TileObject;

use crate::{
    BrushSettings, CanvasSpec, ContentRootId, GroupId, GroupNode, LayerBlendMode, LayerId,
    LayerNode, LayerTree, LayerTreeNode, MAX_LAYERS, MAX_PORTABLE_BYTES, MAX_RESIDENT_TILES,
    TILE_BYTE_LEN, TileKey, TileSnapshot, WebDocument, WebError, WebTool,
};

const MAGIC: &[u8; 8] = b"NYWEB001";
const CHECKSUM_BYTES: usize = 32;
const MAX_NAME_BYTES: usize = 512;
const MAX_TILE_COORDINATE: u32 = 257;

impl WebDocument {
    /// Encodes completed artwork and all five tool settings/colors, never RAM undo.
    /// # Errors
    /// Rejects a live stroke or exceeding the bounded portable size. Not a `.ntdr` file.
    pub fn encode_portable(&self) -> Result<Vec<u8>, WebError> {
        self.require_idle()?;
        let mut bytes = Vec::with_capacity(32 * 1024);
        bytes.extend_from_slice(MAGIC);
        put_u32(&mut bytes, self.canvas().width_px);
        put_u32(&mut bytes, self.canvas().height_px);
        put_u32(&mut bytes, self.canvas().pixels_per_inch);
        bytes.push(self.tool as u8);
        bytes.extend_from_slice(&self.foreground);
        bytes.extend_from_slice(&self.background);
        for settings in self.settings {
            encode_settings(&mut bytes, settings);
        }
        bytes.extend_from_slice(&self.active_layer().0.to_le_bytes());
        put_u32(
            &mut bytes,
            u32::try_from(self.layers().root().children.len())
                .map_err(|_| WebError::LimitExceeded)?,
        );
        for node in &self.layers().root().children {
            let LayerTreeNode::Raster(layer) = node else {
                return Err(WebError::InvalidLayer);
            };
            encode_layer(&mut bytes, layer)?;
        }
        put_u32(
            &mut bytes,
            u32::try_from(self.snapshot().len()).map_err(|_| WebError::LimitExceeded)?,
        );
        for (key, tile) in self.snapshot().iter() {
            bytes.extend_from_slice(&key.layer.0.to_le_bytes());
            bytes.extend_from_slice(&key.x.to_le_bytes());
            bytes.extend_from_slice(&key.y.to_le_bytes());
            let runs = encode_runs(tile.pixels());
            let payload = if let Some(runs) = &runs {
                bytes.push(1);
                runs.as_slice()
            } else {
                bytes.push(0);
                tile.pixels()
            };
            put_u32(
                &mut bytes,
                u32::try_from(payload.len()).map_err(|_| WebError::LimitExceeded)?,
            );
            reserve_payload(&mut bytes, payload.len())?;
            bytes.extend_from_slice(payload);
        }
        if bytes.len() + CHECKSUM_BYTES > MAX_PORTABLE_BYTES {
            return Err(WebError::LimitExceeded);
        }
        let hash = blake3::hash(&bytes);
        bytes.extend_from_slice(hash.as_bytes());
        Ok(bytes)
    }

    /// Decodes into a new document; hosts replace their live document only on success.
    /// # Errors
    /// Bounds bytes, counts, names, coordinates and allocations before use; rejects
    /// unknown versions, damaged checksums, duplicate keys, invalid alpha and trailing bytes.
    pub fn decode_portable(bytes: &[u8]) -> Result<Self, WebError> {
        if bytes.len() > MAX_PORTABLE_BYTES {
            return Err(WebError::LimitExceeded);
        }
        if bytes.len() < MAGIC.len() + CHECKSUM_BYTES {
            return Err(WebError::InvalidPortable);
        }
        if &bytes[..MAGIC.len()] != MAGIC {
            return Err(WebError::UnsupportedVersion);
        }
        let (payload, checksum) = bytes.split_at(bytes.len() - CHECKSUM_BYTES);
        if blake3::hash(payload).as_bytes() != checksum {
            return Err(WebError::InvalidPortable);
        }
        let mut reader = Reader {
            bytes: payload,
            offset: MAGIC.len(),
        };
        let mut document = Self::new(CanvasSpec {
            width_px: reader.u32()?,
            height_px: reader.u32()?,
            pixels_per_inch: reader.u32()?,
        })?;
        document.tool = *WebTool::ALL
            .get(usize::from(reader.byte()?))
            .ok_or(WebError::InvalidPortable)?;
        document.foreground = reader.array()?;
        document.background = reader.array()?;
        for (tool, settings) in WebTool::ALL.into_iter().zip(&mut document.settings) {
            *settings = decode_settings(&mut reader, tool)?;
            if matches!(tool, WebTool::Pencil2H | WebTool::Pencil2B)
                && settings.hardness.to_bits() != 1.0_f32.to_bits()
            {
                return Err(WebError::InvalidPortable);
            }
        }
        let active = LayerId(reader.u128()?);
        let count = reader.count(MAX_LAYERS)?;
        if count == 0 {
            return Err(WebError::InvalidPortable);
        }
        let mut layers = Vec::with_capacity(count);
        let mut ids = BTreeSet::new();
        let mut next = 1;
        for _ in 0..count {
            let layer = decode_layer(&mut reader)?;
            if !ids.insert(layer.id) {
                return Err(WebError::InvalidPortable);
            }
            next = next.max(layer.id.0.checked_add(1).ok_or(WebError::InvalidPortable)?);
            layers.push(LayerTreeNode::Raster(layer));
        }
        if !ids.contains(&active) {
            return Err(WebError::InvalidPortable);
        }
        document.artwork.layers = LayerTree::new(GroupNode {
            id: GroupId(0),
            name: "Root".into(),
            visible: true,
            opacity_u16: u16::MAX,
            clip_to_below: false,
            blend_mode: LayerBlendMode::Normal,
            children: layers,
        })
        .map_err(|_| WebError::InvalidPortable)?;
        document.artwork.active = active;
        document.next_layer_id = next;
        document.artwork.tiles = decode_tiles(&mut reader, &ids)?;
        if reader.offset != reader.bytes.len() {
            return Err(WebError::InvalidPortable);
        }
        Ok(document)
    }
}

fn encode_settings(bytes: &mut Vec<u8>, settings: BrushSettings) {
    for value in [settings.size_px, settings.opacity, settings.hardness] {
        put_u32(bytes, value.to_bits());
    }
    bytes.push(u8::from(settings.size_pressure));
    bytes.push(u8::from(settings.opacity_pressure));
    bytes.push(settings.smoothing);
}

fn decode_settings(reader: &mut Reader<'_>, tool: WebTool) -> Result<BrushSettings, WebError> {
    BrushSettings {
        size_px: f32::from_bits(reader.u32()?),
        opacity: f32::from_bits(reader.u32()?),
        hardness: f32::from_bits(reader.u32()?),
        size_pressure: reader.boolean()?,
        opacity_pressure: reader.boolean()?,
        smoothing: reader.byte()?,
        ..BrushSettings::for_tool(tool)
    }
    .validate()
    .map_err(|_| WebError::InvalidPortable)
}

fn encode_layer(bytes: &mut Vec<u8>, layer: &LayerNode) -> Result<(), WebError> {
    if layer.name.len() > MAX_NAME_BYTES {
        return Err(WebError::InvalidLayer);
    }
    bytes.extend_from_slice(&layer.id.0.to_le_bytes());
    put_u32(
        bytes,
        u32::try_from(layer.name.len()).map_err(|_| WebError::InvalidLayer)?,
    );
    bytes.extend_from_slice(layer.name.as_bytes());
    for flag in [
        layer.visible,
        layer.locked,
        layer.alpha_locked,
        layer.clip_to_below,
        layer.reference,
    ] {
        bytes.push(u8::from(flag));
    }
    bytes.push(match layer.blend_mode {
        LayerBlendMode::Normal => 0,
        LayerBlendMode::Multiply => 1,
    });
    bytes.extend_from_slice(&layer.opacity_u16.to_le_bytes());
    bytes.extend_from_slice(&layer.content_root.0.to_le_bytes());
    Ok(())
}

fn decode_layer(reader: &mut Reader<'_>) -> Result<LayerNode, WebError> {
    let id = LayerId(reader.u128()?);
    let length = reader.count(MAX_NAME_BYTES)?;
    let name = std::str::from_utf8(reader.take(length)?)
        .map_err(|_| WebError::InvalidPortable)?
        .to_owned();
    if name.trim().is_empty() || name.chars().count() > 128 {
        return Err(WebError::InvalidPortable);
    }
    let visible = reader.boolean()?;
    let locked = reader.boolean()?;
    let alpha_locked = reader.boolean()?;
    let clip_to_below = reader.boolean()?;
    let reference = reader.boolean()?;
    let blend_mode = match reader.byte()? {
        0 => LayerBlendMode::Normal,
        1 => LayerBlendMode::Multiply,
        _ => return Err(WebError::InvalidPortable),
    };
    let opacity_u16 = u16::from_le_bytes(reader.array()?);
    let content_root = ContentRootId(reader.u128()?);
    Ok(LayerNode {
        id,
        name,
        visible,
        locked,
        alpha_locked,
        clip_to_below,
        reference,
        blend_mode,
        opacity_u16,
        content_root,
    })
}

fn decode_tiles(
    reader: &mut Reader<'_>,
    layers: &BTreeSet<LayerId>,
) -> Result<TileSnapshot, WebError> {
    let count = reader.count(MAX_RESIDENT_TILES)?;
    let mut tiles = Vec::with_capacity(count);
    let mut keys = BTreeSet::new();
    for _ in 0..count {
        let key = TileKey {
            layer: LayerId(reader.u128()?),
            mip: 0,
            x: i32::from_le_bytes(reader.array()?),
            y: i32::from_le_bytes(reader.array()?),
        };
        if !layers.contains(&key.layer)
            || key.x.unsigned_abs() > MAX_TILE_COORDINATE
            || key.y.unsigned_abs() > MAX_TILE_COORDINATE
            || !keys.insert(key)
        {
            return Err(WebError::InvalidPortable);
        }
        let codec = reader.byte()?;
        let length = reader.count(TILE_BYTE_LEN)?;
        let encoded = reader.take(length)?;
        let pixels = match codec {
            0 if length == TILE_BYTE_LEN => encoded.to_vec(),
            1 if length < TILE_BYTE_LEN => decode_runs(encoded)?,
            _ => return Err(WebError::InvalidPortable),
        };
        if pixels
            .chunks_exact(4)
            .any(|pixel| pixel[..3].iter().any(|value| *value > pixel[3]))
            || pixels.iter().all(|byte| *byte == 0)
        {
            return Err(WebError::InvalidPortable);
        }
        tiles.push((
            key,
            TileObject::new(pixels).map_err(|_| WebError::InvalidPortable)?,
        ));
    }
    TileSnapshot::from_objects(tiles).map_err(|_| WebError::InvalidPortable)
}

fn reserve_payload(bytes: &mut Vec<u8>, payload_bytes: usize) -> Result<(), WebError> {
    let required = bytes.len() + payload_bytes + CHECKSUM_BYTES;
    if required > MAX_PORTABLE_BYTES {
        return Err(WebError::LimitExceeded);
    }
    if required > bytes.capacity() {
        let capacity = (bytes.capacity() * 2).max(required).min(MAX_PORTABLE_BYTES);
        bytes
            .try_reserve_exact(capacity - bytes.len())
            .map_err(|_| WebError::LimitExceeded)?;
    }
    Ok(())
}

fn encode_runs(pixels: &[u8]) -> Option<Vec<u8>> {
    let mut bytes = Vec::new();
    let mut offset = 0;
    while offset < pixels.len() {
        let pixel = &pixels[offset..offset + 4];
        let mut count = 1_u16;
        while offset + (usize::from(count) + 1) * 4 <= pixels.len()
            && &pixels[offset + usize::from(count) * 4..offset + (usize::from(count) + 1) * 4]
                == pixel
        {
            count += 1;
        }
        if bytes.len() + 6 >= TILE_BYTE_LEN {
            return None;
        }
        bytes.extend_from_slice(&count.to_le_bytes());
        bytes.extend_from_slice(pixel);
        offset += usize::from(count) * 4;
    }
    Some(bytes)
}

fn decode_runs(bytes: &[u8]) -> Result<Vec<u8>, WebError> {
    if bytes.is_empty() || !bytes.len().is_multiple_of(6) {
        return Err(WebError::InvalidPortable);
    }
    let mut pixels = Vec::with_capacity(TILE_BYTE_LEN);
    for run in bytes.chunks_exact(6) {
        let count = usize::from(u16::from_le_bytes([run[0], run[1]]));
        if count == 0 || pixels.len() + count * 4 > TILE_BYTE_LEN {
            return Err(WebError::InvalidPortable);
        }
        for _ in 0..count {
            pixels.extend_from_slice(&run[2..]);
        }
    }
    if pixels.len() != TILE_BYTE_LEN {
        return Err(WebError::InvalidPortable);
    }
    Ok(pixels)
}

fn put_u32(bytes: &mut Vec<u8>, value: u32) {
    bytes.extend_from_slice(&value.to_le_bytes());
}

struct Reader<'a> {
    bytes: &'a [u8],
    offset: usize,
}

impl<'a> Reader<'a> {
    fn take(&mut self, count: usize) -> Result<&'a [u8], WebError> {
        let end = self
            .offset
            .checked_add(count)
            .ok_or(WebError::InvalidPortable)?;
        let bytes = self
            .bytes
            .get(self.offset..end)
            .ok_or(WebError::InvalidPortable)?;
        self.offset = end;
        Ok(bytes)
    }
    fn array<const N: usize>(&mut self) -> Result<[u8; N], WebError> {
        self.take(N)?
            .try_into()
            .map_err(|_| WebError::InvalidPortable)
    }
    fn byte(&mut self) -> Result<u8, WebError> {
        Ok(self.array::<1>()?[0])
    }
    fn boolean(&mut self) -> Result<bool, WebError> {
        match self.byte()? {
            0 => Ok(false),
            1 => Ok(true),
            _ => Err(WebError::InvalidPortable),
        }
    }
    fn u32(&mut self) -> Result<u32, WebError> {
        Ok(u32::from_le_bytes(self.array()?))
    }
    fn u128(&mut self) -> Result<u128, WebError> {
        Ok(u128::from_le_bytes(self.array()?))
    }
    fn count(&mut self, maximum: usize) -> Result<usize, WebError> {
        let value = usize::try_from(self.u32()?).map_err(|_| WebError::LimitExceeded)?;
        if value > maximum {
            return Err(WebError::LimitExceeded);
        }
        Ok(value)
    }
}
