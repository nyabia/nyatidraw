use nyatidraw_project::{Envelope, RecordKind};

pub(super) const SCHEMA_FLAG: u64 = 0x4000;
const MAGIC: &[u8; 8] = b"NYROOT01";
const HEADER_BYTES: usize = 16;
const MAX_DECODED_BYTES: usize = 64 * 1024 * 1024;

pub(super) fn encode(envelope: &Envelope) -> Vec<u8> {
    let raw = envelope.encode();
    if raw.len() <= MAX_DECODED_BYTES
        && let Ok(compressed) = zstd::bulk::compress(&raw, 1)
        && compressed.len() + HEADER_BYTES < raw.len()
    {
        let mut bytes = Vec::with_capacity(HEADER_BYTES + compressed.len());
        bytes.extend_from_slice(MAGIC);
        bytes.extend_from_slice(&(raw.len() as u64).to_le_bytes());
        bytes.extend_from_slice(&compressed);
        return bytes;
    }
    raw
}

pub(super) fn decode(bytes: &[u8]) -> Result<Envelope, String> {
    if !bytes.starts_with(MAGIC) {
        return Envelope::decode(bytes, RecordKind::ContentRoot).map_err(|error| error.to_string());
    }
    decode_with_limit(bytes, MAX_DECODED_BYTES)
}

pub(super) fn decode_with_limit(bytes: &[u8], limit: usize) -> Result<Envelope, String> {
    if !bytes.starts_with(MAGIC) {
        if bytes.len() > limit {
            return Err("root exceeds browser decode budget".into());
        }
        return Envelope::decode(bytes, RecordKind::ContentRoot).map_err(|error| error.to_string());
    }
    let length = bytes.get(8..HEADER_BYTES).ok_or("truncated root header")?;
    let length = usize::try_from(u64::from_le_bytes(length.try_into().unwrap()))
        .map_err(|_| "root length exceeds address space")?;
    if !(112..=MAX_DECODED_BYTES).contains(&length) || bytes.len() >= length {
        return Err("invalid compressed root length".into());
    }
    if length > limit {
        return Err("root exceeds browser decode budget".into());
    }
    let frame = &bytes[HEADER_BYTES..];
    if zstd::zstd_safe::find_frame_compressed_size(frame)
        .map_err(|_| "invalid compressed root frame")?
        != frame.len()
    {
        return Err("trailing compressed root data".into());
    }
    let mut raw = vec![0; length];
    let decoded = zstd::bulk::decompress_to_buffer(frame, &mut raw[..])
        .map_err(|_| "invalid compressed root payload")?;
    if decoded != length {
        return Err("root decoded length mismatch".into());
    }
    Envelope::decode(&raw, RecordKind::ContentRoot).map_err(|error| error.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use nyatidraw_api::LayerId;
    use nyatidraw_project::{decode_root_manifest, encode_root_manifest};
    use nyatidraw_tiles::{TILE_BYTE_LEN, TileKey, TileObject, TileSnapshot};

    #[test]
    fn root_compression_preserves_identity_and_rejects_corrupt_or_unbounded_records() {
        let tile = TileObject::new(vec![64; TILE_BYTE_LEN]).unwrap();
        let snapshot = TileSnapshot::from_objects((-600..600).map(|x| {
            (
                TileKey {
                    layer: LayerId(8),
                    x,
                    y: -17,
                    mip: 0,
                },
                tile.clone(),
            )
        }))
        .unwrap();
        let envelope = Envelope::new(RecordKind::ContentRoot, encode_root_manifest(&snapshot));
        let raw = envelope.encode();
        let compressed = encode(&envelope);
        assert!(compressed.starts_with(MAGIC));
        assert!(compressed.len() < raw.len() / 4);
        for bytes in [&raw, &compressed] {
            let decoded = decode(bytes).unwrap();
            assert_eq!(decoded, envelope);
            let manifest = decode_root_manifest(&decoded.payload).unwrap();
            assert_eq!(manifest.root, snapshot.root());
            assert_eq!(manifest.tiles.len(), snapshot.len());
        }
        for declared in [0, 111, MAX_DECODED_BYTES as u64 + 1, u64::MAX] {
            let mut bad = compressed.clone();
            bad[8..16].copy_from_slice(&declared.to_le_bytes());
            assert!(decode(&bad).is_err());
        }
        for length in [0, 7, 8, 15, compressed.len() - 1] {
            assert!(decode(&compressed[..length]).is_err());
        }
        let mut trailing = compressed.clone();
        trailing.push(0);
        assert!(decode(&trailing).is_err());
        let mut corrupt = compressed.clone();
        corrupt[HEADER_BYTES] ^= 0x80;
        assert!(decode(&corrupt).is_err());
        let mut bad_checksum = envelope;
        bad_checksum.payload[0] ^= 1;
        assert!(decode(&encode(&bad_checksum)).is_err());
    }
}
