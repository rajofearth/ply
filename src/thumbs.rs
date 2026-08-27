//! Raster thumbnails for media entries, drawn from Windows Explorer's own
//! thumbnail cache via `IShellItemImageFactory`.
//!
//! A single size is extracted per path (clamped by Explorer to the larger
//! side) and the UI scales it down; this keeps the working set small and the
//! extraction fast. Results are cached per path + mtime in [`ThumbCache`].

use std::collections::{HashMap, HashSet, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use gpui::{AppContext, Context, RenderImage};

use crate::app::Ply;
use crate::listing::{Entry, KindClass, kind_class};

/// One thumbnail size, in device-independent pixels on the larger side.
pub const THUMB_SIZE: u32 = 96;

/// Hard ceiling on cached pixel bytes (~32 MiB of RGBA).
const BUDGET: usize = 32 * 1024 * 1024;

/// Identity of a cached thumbnail: a path plus the mtime it was made from, so
/// an edited file is re-extracted.
#[derive(Clone, PartialEq, Eq, Hash)]
pub struct CacheKey {
    path: PathBuf,
    mtime: u64,
}

pub fn cache_key(path: &Path, mtime: Option<SystemTime>) -> CacheKey {
    CacheKey {
        path: path.to_path_buf(),
        mtime: mtime_nanos(mtime),
    }
}

fn mtime_nanos(t: Option<SystemTime>) -> u64 {
    t.and_then(|t| t.duration_since(UNIX_EPOCH).ok().map(|d| d.as_nanos() as u64))
        .unwrap_or(0)
}

/// Per-window cache of decoded thumbnails, kept inside [`Ply`] so it is dropped
/// with the window. LRU-evicts oldest entries past [`BUDGET`].
pub struct ThumbCache {
    map: HashMap<CacheKey, Arc<RenderImage>>,
    order: VecDeque<CacheKey>,
    inflight: HashSet<CacheKey>,
    bytes: usize,
}

impl ThumbCache {
    pub fn new() -> Self {
        Self {
            map: HashMap::new(),
            order: VecDeque::new(),
            inflight: HashSet::new(),
            bytes: 0,
        }
    }

    pub fn get(&self, key: &CacheKey) -> Option<Arc<RenderImage>> {
        self.map.get(key).cloned()
    }

    pub fn is_inflight(&self, key: &CacheKey) -> bool {
        self.inflight.contains(key)
    }

    pub fn mark_inflight(&mut self, key: CacheKey) {
        self.inflight.insert(key);
    }

    pub fn insert(&mut self, key: CacheKey, img: Arc<RenderImage>) {
        self.inflight.remove(&key);
        if self.map.contains_key(&key) {
            return;
        }
        let sz = byte_size(&img);
        self.bytes = self.bytes.saturating_add(sz);
        self.map.insert(key.clone(), img);
        self.order.push_back(key);
        while self.bytes > BUDGET {
            match self.order.pop_front() {
                Some(old) => {
                    if let Some(evicted) = self.map.remove(&old) {
                        self.bytes = self.bytes.saturating_sub(byte_size(&evicted));
                    }
                }
                None => break,
            }
        }
    }
}

fn byte_size(img: &Arc<RenderImage>) -> usize {
    let s = img.size(0);
    let w = s.width.0 as usize;
    let h = s.height.0 as usize;
    w.saturating_mul(h).max(1) * 4
}

/// Kicks off extraction for an image or video entry if it is not already
/// cached or in flight, then notifies the window when it lands. Directories,
/// MTP objects and everything else fall back to the SVG icon in the UI.
pub fn request_thumbnail(ply: &Ply, entry: &Entry, cx: &mut Context<Ply>) {
    if !matches!(kind_class(entry), KindClass::Image | KindClass::Video) {
        return;
    }
    if crate::mtp::is_mtp(&entry.path) {
        return;
    }
    let key = cache_key(&entry.path, entry.modified);
    let cache_entity = ply.thumb_cache();
    let (cached, inflight) = {
        let c = cache_entity.read(cx);
        (c.get(&key).is_some(), c.is_inflight(&key))
    };
    if cached || inflight {
        return;
    }
    cache_entity.update(cx, |c, _| c.mark_inflight(key.clone()));

    let path = entry.path.clone();
    cx.spawn(async move |this, cx| {
        let got = cx
            .background_spawn(async move { request(path, THUMB_SIZE) })
            .await;
        if let Some((bytes, w, h)) = got {
            if let Some(img) = to_render_image(bytes, w, h) {
                let _ = this.update(cx, |this, cx| {
                    this.thumb_cache().update(cx, |c, _| c.insert(key.clone(), img));
                });
            }
        }
        let _ = this.update(cx, |_, cx| cx.notify());
    })
    .detach();
}

fn to_render_image(bytes: Vec<u8>, w: u32, h: u32) -> Option<Arc<RenderImage>> {
    let buf = image::ImageBuffer::<image::Rgba<u8>, Vec<u8>>::from_raw(w, h, bytes)?;
    let frame = image::Frame::new(buf);
    Some(Arc::new(RenderImage::new(vec![frame])))
}

#[cfg(windows)]
mod backend {
    use std::os::windows::ffi::OsStrExt;
    use std::path::PathBuf;
    use std::sync::mpsc::{channel, Sender};
    use std::sync::LazyLock;
    use std::thread;

    use windows::core::PCWSTR;
    use windows::Win32::Foundation::SIZE;
    use windows::Win32::Graphics::Gdi::{
        BITMAP, BITMAPINFO, BITMAPINFOHEADER, BI_RGB, CreateCompatibleDC, DIB_RGB_COLORS,
        DeleteDC, DeleteObject, GetDIBits, GetObjectW, SelectObject, HBITMAP, HGDIOBJ,
    };
    use windows::Win32::System::Com::{CoInitializeEx, COINIT_APARTMENTTHREADED};
    use windows::Win32::UI::Shell::{IShellItemImageFactory, SHCreateItemFromParsingName, SIIGBF};

    /// Decoded BGRA pixels plus dimensions.
    type ThumbPixels = (Vec<u8>, u32, u32);

    struct Job {
        path: PathBuf,
        size: u32,
        reply: Sender<Option<ThumbPixels>>,
    }

    static WORKER: LazyLock<Sender<Job>> = LazyLock::new(|| {
        let (tx, rx) = channel::<Job>();
        thread::spawn(move || {
            // STA: IShellItemImageFactory expects an apartment-threaded caller.
            unsafe {
                let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
            }
            while let Ok(job) = rx.recv() {
                let _ = job.reply.send(extract(&job.path, job.size));
            }
        });
        tx
    });

    pub(super) fn request(path: PathBuf, size: u32) -> Option<ThumbPixels> {
        let (tx, rx) = channel();
        if WORKER.send(Job { path, size, reply: tx }).is_err() {
            return None;
        }
        rx.recv().ok().flatten()
    }

    fn extract(path: &PathBuf, size: u32) -> Option<ThumbPixels> {
        let wide: Vec<u16> = path
            .as_os_str()
            .encode_wide()
            .chain(std::iter::once(0))
            .collect();
        let item: IShellItemImageFactory =
            unsafe { SHCreateItemFromParsingName(PCWSTR(wide.as_ptr()), None) }.ok()?;
        let hbitmap: HBITMAP = unsafe {
            item.GetImage(
                SIZE {
                    cx: size as i32,
                    cy: size as i32,
                },
                SIIGBF(0),
            )
        }
        .ok()?;
        let res = hbitmap_to_bgra(hbitmap);
        unsafe {
            let _ = DeleteObject(HGDIOBJ(hbitmap.0));
        }
        res
    }

    // Windows hands back a bottom-up 32bpp DIB; flip to top-down and normalise
    // alpha (GetDIBits often reports a fully-zero alpha byte) so GPUI's BGRA
    // frame is correct.
    fn hbitmap_to_bgra(hbitmap: HBITMAP) -> Option<ThumbPixels> {
        let mut bm: BITMAP = unsafe { std::mem::zeroed() };
        if unsafe {
            GetObjectW(
                HGDIOBJ(hbitmap.0),
                std::mem::size_of::<BITMAP>() as i32,
                Some(&mut bm as *mut _ as *mut core::ffi::c_void),
            )
        } == 0
        {
            return None;
        }
        let w = bm.bmWidth as u32;
        let h = bm.bmHeight as u32;
        let dc = unsafe { CreateCompatibleDC(None) };
        if dc.0.is_null() {
            return None;
        }
        let prev = unsafe { SelectObject(dc, HGDIOBJ(hbitmap.0)) };
        let mut bmi: BITMAPINFO = unsafe { std::mem::zeroed() };
        bmi.bmiHeader.biSize = std::mem::size_of::<BITMAPINFOHEADER>() as u32;
        bmi.bmiHeader.biWidth = w as i32;
        bmi.bmiHeader.biHeight = h as i32;
        bmi.bmiHeader.biPlanes = 1;
        bmi.bmiHeader.biBitCount = 32;
        bmi.bmiHeader.biCompression = BI_RGB.0;
        let mut buf: Vec<u8> = vec![0u8; (w as usize) * (h as usize) * 4];
        let lines = unsafe {
            GetDIBits(
                dc,
                hbitmap,
                0,
                h,
                Some(buf.as_mut_ptr() as *mut core::ffi::c_void),
                &mut bmi as *mut _ as *mut BITMAPINFO,
                DIB_RGB_COLORS,
            )
        };
        unsafe {
            let _ = SelectObject(dc, prev);
            let _ = DeleteDC(dc);
        }
        if lines == 0 {
            return None;
        }

        let stride = (w as usize) * 4;
        let mut out = vec![0u8; buf.len()];
        for y in 0..(h as usize) {
            let src = &buf[y * stride..(y + 1) * stride];
            let dst = &mut out[(h as usize - 1 - y) * stride..(h as usize - y) * stride];
            dst.copy_from_slice(src);
        }
        let all_zero_alpha = out.iter().step_by(4).skip(1).all(|a| *a == 0);
        if all_zero_alpha {
            for px in out.chunks_exact_mut(4) {
                px[3] = 255;
            }
        }
        Some((out, w, h))
    }
}

#[cfg(not(windows))]
fn request(_path: PathBuf, _size: u32) -> Option<(Vec<u8>, u32, u32)> {
    None
}

#[cfg(windows)]
use backend::request;
