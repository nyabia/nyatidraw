//! OS boundary for artwork exchange. Pixel algorithms and tests use the trait,
//! never the user's system clipboard. Windows supports private exact RGBA plus
//! registered PNG or bounded Windows DIB input.
use nyatidraw_paint_cpu::RasterFragment;

#[cfg(any(windows, test))]
pub(crate) mod dib;

pub(crate) use nyatidraw_editor::pixel_edit::ArtworkClipboard;

pub(crate) struct SystemClipboard;

#[derive(Default)]
pub(crate) struct PageCopyWorker {
    handle: Option<std::thread::JoinHandle<()>>,
}

impl PageCopyWorker {
    pub(crate) fn start(
        &mut self,
        tiles: &std::collections::BTreeMap<nyatidraw_tiles::TileKey, Vec<u8>>,
        tree: nyatidraw_document::LayerTree,
        canvas: nyatidraw_api::CanvasSpec,
        live_ink: crate::live_ink::LiveInkBridge,
    ) -> Result<(), String> {
        if self
            .handle
            .as_ref()
            .is_some_and(|handle| !handle.is_finished())
        {
            return Err("PNG 복사를 처리 중입니다. 잠시 뒤 다시 시도해주세요.".into());
        }
        if let Some(handle) = self.handle.take() {
            let _ = handle.join();
        }
        let tiles = page_tiles(tiles, canvas)?;
        live_ink.publish_activation_notice("그림 전체를 PNG로 복사하고 있습니다.".into());
        self.handle = Some(
            std::thread::Builder::new()
                .name("nyatidraw-png-clipboard".into())
                .spawn(move || {
                    let result = nyatidraw_tiles::TileSnapshot::from_tiles(tiles)
                        .map_err(|error| format!("복사할 픽셀을 준비하지 못했습니다: {error:?}"))
                        .and_then(|tiles| copy_page(&tiles, &tree, canvas, &mut SystemClipboard));
                    live_ink.publish_activation_notice(match result {
                        Ok(()) => {
                            "그림 전체를 PNG로 복사했습니다. 다른 앱에 붙여넣을 수 있습니다.".into()
                        }
                        Err(error) => {
                            format!("PNG를 복사하지 못했습니다. 그림은 그대로 유지됩니다: {error}")
                        }
                    });
                })
                .map_err(|error| format!("PNG 복사 작업을 시작하지 못했습니다: {error}"))?,
        );
        Ok(())
    }
}

impl Drop for PageCopyWorker {
    fn drop(&mut self) {
        if let Some(handle) = self.handle.take() {
            let _ = handle.join();
        }
    }
}

pub(crate) fn page_tiles(
    tiles: &std::collections::BTreeMap<nyatidraw_tiles::TileKey, Vec<u8>>,
    canvas: nyatidraw_api::CanvasSpec,
) -> Result<Vec<(nyatidraw_tiles::TileKey, Vec<u8>)>, String> {
    RasterFragment::byte_len([0, 0], [canvas.width_px, canvas.height_px])
        .map_err(|_| "그림이 PNG 클립보드 크기 한도(16백만 픽셀)를 넘습니다.".to_owned())?;
    let columns = i64::from(canvas.width_px.div_ceil(nyatidraw_tiles::TILE_EDGE));
    let rows = i64::from(canvas.height_px.div_ceil(nyatidraw_tiles::TILE_EDGE));
    let mut captured_bytes = 0_usize;
    tiles
        .iter()
        .filter(|(key, _)| {
            key.mip == 0
                && (0..columns).contains(&i64::from(key.x))
                && (0..rows).contains(&i64::from(key.y))
        })
        .map(|(key, pixels)| {
            captured_bytes = captured_bytes.saturating_add(pixels.len());
            if captured_bytes > MAX_BYTES {
                return Err("레이어 픽셀이 PNG 복사 메모리 한도(128 MiB)를 넘습니다.".into());
            }
            Ok((*key, pixels.clone()))
        })
        .collect()
}

pub(crate) fn copy_page(
    tiles: &nyatidraw_tiles::TileSnapshot,
    tree: &nyatidraw_document::LayerTree,
    canvas: nyatidraw_api::CanvasSpec,
    clipboard: &mut dyn ArtworkClipboard,
) -> Result<(), String> {
    RasterFragment::byte_len([0, 0], [canvas.width_px, canvas.height_px])
        .map_err(|_| "그림이 PNG 클립보드 크기 한도(16백만 픽셀)를 넘습니다.".to_owned())?;
    let surface = nyatidraw_paint_cpu::flatten_layer_tree_rgba8(tiles, tree, canvas)
        .map_err(|error| format!("PNG 합성 실패: {error:?}"))?;
    let fragment = RasterFragment::new([0, 0], [surface.width, surface.height], surface.pixels)
        .map_err(|error| format!("PNG 복사 실패: {error:?}"))?;
    clipboard.write(&fragment)
}

#[cfg(test)]
#[derive(Default)]
pub(crate) struct MemoryClipboard {
    pub(crate) bytes: Option<Vec<u8>>,
    pub(crate) reject_write: bool,
}

#[cfg(test)]
impl ArtworkClipboard for MemoryClipboard {
    fn write(&mut self, fragment: &RasterFragment) -> Result<(), String> {
        if self.reject_write {
            return Err("Injected clipboard contention".into());
        }
        self.bytes = Some(encode_exact(fragment));
        Ok(())
    }
    fn read(&mut self) -> Result<RasterFragment, String> {
        decode_exact(self.bytes.as_deref().ok_or("Empty clipboard")?)
    }
}

const HEADER: &[u8; 8] = b"NTDRCLP1";
const HEADER_BYTES: usize = 24;
const MAX_BYTES: usize = 128 * 1024 * 1024;

fn encode_exact(fragment: &RasterFragment) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(HEADER_BYTES + fragment.pixels().len());
    bytes.extend_from_slice(HEADER);
    for value in fragment.origin() {
        bytes.extend_from_slice(&value.to_le_bytes());
    }
    for value in fragment.size() {
        bytes.extend_from_slice(&value.to_le_bytes());
    }
    bytes.extend_from_slice(fragment.pixels());
    bytes
}

fn decode_exact(bytes: &[u8]) -> Result<RasterFragment, String> {
    if bytes.len() < HEADER_BYTES || &bytes[..8] != HEADER || bytes.len() > MAX_BYTES {
        return Err("잘못된 NyatiDraw 클립보드 데이터입니다.".into());
    }
    let word = |offset| <[u8; 4]>::try_from(&bytes[offset..offset + 4]).expect("checked header");
    let origin = [i32::from_le_bytes(word(8)), i32::from_le_bytes(word(12))];
    let size = [u32::from_le_bytes(word(16)), u32::from_le_bytes(word(20))];
    let length = RasterFragment::byte_len(origin, size).map_err(|e| format!("Clipboard: {e:?}"))?;
    let pixels = bytes
        .get(HEADER_BYTES..HEADER_BYTES + length)
        .ok_or("잘린 클립보드 데이터입니다.")?;
    RasterFragment::new(origin, size, pixels.to_vec()).map_err(|e| format!("Clipboard: {e:?}"))
}

#[cfg(windows)]
mod windows_clipboard {
    use super::{
        ArtworkClipboard, MAX_BYTES, RasterFragment, SystemClipboard, decode_exact, dib,
        encode_exact,
    };
    use windows::{
        Win32::{
            Foundation::{GlobalFree, HANDLE, HGLOBAL, HWND},
            System::{
                DataExchange::{
                    CloseClipboard, EmptyClipboard, GetClipboardData, IsClipboardFormatAvailable,
                    OpenClipboard, RegisterClipboardFormatW, SetClipboardData,
                },
                Memory::{GMEM_MOVEABLE, GlobalAlloc, GlobalLock, GlobalSize, GlobalUnlock},
            },
            UI::WindowsAndMessaging::{
                CreateWindowExW, DestroyWindow, HWND_MESSAGE, WINDOW_EX_STYLE, WINDOW_STYLE,
            },
        },
        core::w,
    };

    const CF_DIB: u32 = 8;
    const CF_DIBV5: u32 = 17;

    struct Owner(HWND);
    impl Drop for Owner {
        fn drop(&mut self) {
            // SAFETY: owned hidden window is destroyed on its creating thread.
            let _ = unsafe { DestroyWindow(self.0) };
        }
    }
    struct Open;
    impl Drop for Open {
        fn drop(&mut self) {
            // SAFETY: this guard exists only after this thread opened clipboard.
            let _ = unsafe { CloseClipboard() };
        }
    }
    struct Block(Option<HGLOBAL>);
    impl Drop for Block {
        fn drop(&mut self) {
            if let Some(handle) = self.0.take() {
                // SAFETY: blocks not transferred to the OS remain ours.
                let _ = unsafe { GlobalFree(Some(handle)) };
            }
        }
    }
    impl Block {
        fn new(bytes: &[u8]) -> Result<Self, String> {
            if bytes.is_empty() || bytes.len() > MAX_BYTES {
                return Err("클립보드 크기 한도 초과".into());
            }
            // SAFETY: movable allocation, exact checked nonzero byte length.
            let handle =
                unsafe { GlobalAlloc(GMEM_MOVEABLE, bytes.len()) }.map_err(|e| e.to_string())?;
            let block = Self(Some(handle));
            // SAFETY: handle is live and owned; checked pointer spans allocation.
            let pointer = unsafe { GlobalLock(handle) }.cast::<u8>();
            if pointer.is_null() {
                return Err("클립보드 메모리를 잠글 수 없습니다.".into());
            }
            // SAFETY: disjoint buffers and allocation has bytes.len() capacity.
            unsafe {
                std::ptr::copy_nonoverlapping(bytes.as_ptr(), pointer, bytes.len());
            }
            // SAFETY: balances the successful GlobalLock above.
            let _ = unsafe { GlobalUnlock(handle) };
            Ok(block)
        }
        fn publish(&mut self, format: u32) -> Result<(), String> {
            let handle = self.0.expect("unpublished block");
            // SAFETY: clipboard is open and allocation is movable, unlocked.
            unsafe { SetClipboardData(format, Some(HANDLE(handle.0))) }
                .map_err(|e| e.to_string())?;
            self.0 = None; // Successful transfer: Windows owns the allocation.
            Ok(())
        }
    }
    fn formats() -> Result<(u32, u32), String> {
        // SAFETY: static null-terminated names, no borrowed output pointers.
        let exact = unsafe { RegisterClipboardFormatW(w!("NyatiDraw.RasterFragment.v1")) };
        let png = unsafe { RegisterClipboardFormatW(w!("PNG")) };
        if exact == 0 || png == 0 {
            return Err("클립보드 형식 등록 실패".into());
        }
        Ok((exact, png))
    }
    fn read_bytes(format: u32) -> Result<Vec<u8>, String> {
        // SAFETY: caller holds Open until all bytes are copied.
        let handle = unsafe { GetClipboardData(format) }.map_err(|e| e.to_string())?;
        let global = HGLOBAL(handle.0);
        // SAFETY: registered PNG/exact and CF_DIB(V5) carry HGLOBAL, not GDI handles.
        let length = unsafe { GlobalSize(global) };
        if length == 0 || length > MAX_BYTES {
            return Err("클립보드 크기 한도 초과".into());
        }
        // SAFETY: clipboard remains open, preventing replacement of the handle.
        let pointer = unsafe { GlobalLock(global) }.cast::<u8>();
        if pointer.is_null() {
            return Err("클립보드 메모리 읽기 실패".into());
        }
        // SAFETY: GlobalSize checked the full allocation, GlobalLock succeeded.
        let bytes = unsafe { std::slice::from_raw_parts(pointer, length) }.to_vec();
        // SAFETY: balances successful lock, OS ownership is retained.
        let _ = unsafe { GlobalUnlock(global) };
        Ok(bytes)
    }
    impl ArtworkClipboard for SystemClipboard {
        fn write(&mut self, fragment: &RasterFragment) -> Result<(), String> {
            let (exact, png) = formats()?;
            let surface = nyatidraw_tiles::FlattenedRgba8 {
                origin_x: 0,
                origin_y: 0,
                width: fragment.size()[0],
                height: fragment.size()[1],
                pixels: fragment.pixels().to_vec(),
            };
            let png_bytes =
                nyatidraw_png_io::encode_png_bytes(&surface).map_err(|e| e.to_string())?;
            let mut exact_block = Block::new(&encode_exact(fragment))?;
            let mut png_block = Block::new(&png_bytes)?;
            // SAFETY: built-in STATIC class, hidden message-only window. A real
            // owner is required for EmptyClipboard followed by SetClipboardData.
            let owner = Owner(
                unsafe {
                    CreateWindowExW(
                        WINDOW_EX_STYLE(0),
                        w!("STATIC"),
                        w!("NyatiDraw clipboard"),
                        WINDOW_STYLE(0),
                        0,
                        0,
                        0,
                        0,
                        Some(HWND_MESSAGE),
                        None,
                        None,
                        None,
                    )
                }
                .map_err(|e| e.to_string())?,
            );
            // SAFETY: valid owner HWND on this worker thread, no UI access.
            unsafe { OpenClipboard(Some(owner.0)) }
                .map_err(|_| "클립보드를 다른 앱이 사용 중입니다. 다시 시도하세요.")?;
            let _open = Open;
            // SAFETY: our thread holds the clipboard; all encoding/allocation
            // completed before this destructive OS operation. Cut commits later.
            unsafe { EmptyClipboard() }.map_err(|e| e.to_string())?;
            exact_block.publish(exact)?;
            png_block.publish(png)?;
            Ok(())
        }
        fn read(&mut self) -> Result<RasterFragment, String> {
            let (exact, png) = formats()?;
            let (format, bytes) = {
                // SAFETY: read-only access needs no owner; release before decode.
                unsafe { OpenClipboard(None) }
                    .map_err(|_| "클립보드를 다른 앱이 사용 중입니다. 다시 시도하세요.")?;
                let _open = Open;
                // SAFETY: queries registered identifiers only.
                if unsafe { IsClipboardFormatAvailable(exact) }.is_ok() {
                    (exact, read_bytes(exact)?)
                } else if unsafe { IsClipboardFormatAvailable(png) }.is_ok() {
                    (png, read_bytes(png)?)
                } else if unsafe { IsClipboardFormatAvailable(CF_DIBV5) }.is_ok() {
                    (CF_DIBV5, read_bytes(CF_DIBV5)?)
                } else if unsafe { IsClipboardFormatAvailable(CF_DIB) }.is_ok() {
                    (CF_DIB, read_bytes(CF_DIB)?)
                } else {
                    return Err("PNG 또는 비트맵 그림이 클립보드에 없습니다.".into());
                }
            };
            if format == exact {
                return decode_exact(&bytes);
            }
            if format != png {
                return dib::decode(&bytes);
            }
            let imported = nyatidraw_png_io::decode_png_bytes(
                &bytes,
                nyatidraw_api::LayerId(1),
                nyatidraw_paint_cpu::EditLimits::default().max_pixels,
            )
            .map_err(|e| e.to_string())?;
            let surface = imported
                .tiles
                .crop_base_layer_rgba8_to_canvas(nyatidraw_api::LayerId(1), imported.canvas)
                .map_err(|e| format!("Clipboard: {e:?}"))?;
            RasterFragment::new([0, 0], [surface.width, surface.height], surface.pixels)
                .map_err(|e| format!("Clipboard: {e:?}"))
        }
    }
}

#[cfg(not(windows))]
impl ArtworkClipboard for SystemClipboard {
    fn write(&mut self, _: &RasterFragment) -> Result<(), String> {
        Err("이 플랫폼의 그림 클립보드는 아직 지원하지 않습니다.".into())
    }
    fn read(&mut self) -> Result<RasterFragment, String> {
        Err("이 플랫폼의 그림 클립보드는 아직 지원하지 않습니다.".into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn clipboard_wire_rejects_forged_extents_and_preserves_exact_alpha() {
        // Product risk: untrusted clipboard sizes may exhaust memory; sRGB
        // conversion on internal copy must not change canonical artwork bytes.
        let source = RasterFragment::new([-129, 1], [2, 1], vec![1, 2, 3, 4, 0, 0, 0, 0]).unwrap();
        let bytes = encode_exact(&source);
        assert_eq!(decode_exact(&bytes).unwrap(), source);
        for length in [0, 7, 23, 24, bytes.len() - 1] {
            assert!(decode_exact(&bytes[..length]).is_err());
        }
        for (offset, value) in [(16, u32::MAX), (20, u32::MAX), (16, 0)] {
            let mut invalid = bytes.clone();
            invalid[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
            assert!(decode_exact(&invalid).is_err());
        }
        let mut invalid = bytes.clone();
        invalid[24] = 255;
        assert!(decode_exact(&invalid).is_err());
        let surface = nyatidraw_tiles::FlattenedRgba8 {
            origin_x: 0,
            origin_y: 0,
            width: 2,
            height: 1,
            pixels: source.pixels().to_vec(),
        };
        let png = nyatidraw_png_io::encode_png_bytes(&surface).unwrap();
        assert!(nyatidraw_png_io::decode_png_bytes(&png, nyatidraw_api::LayerId(1), 1).is_err());
        let imported =
            nyatidraw_png_io::decode_png_bytes(&png, nyatidraw_api::LayerId(1), 2).unwrap();
        let flat = imported
            .tiles
            .crop_base_layer_rgba8_to_canvas(nyatidraw_api::LayerId(1), imported.canvas)
            .unwrap();
        assert_eq!(flat.pixels, surface.pixels);
    }
}
