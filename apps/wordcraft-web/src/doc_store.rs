//! Documents kept in the browser for the next visit, so Recent reopens them without the file
//! picker: one OPFS file per document under `navigator.storage().getDirectory()/documents`. A
//! document is a file (name → bytes), which OPFS stores as-is where IndexedDB would copy every
//! record through structured clone; the fonts' IndexedDB (`font_store`) and this share the
//! browser's storage quota either way. Every failure is logged and skipped: a document that
//! can't be kept still opens, edits and saves as before — the store is a cache, never a gate.
//!
//! The bounds and the eviction below are plain functions so the crate's native tests cover them;
//! only the OPFS calls are wasm. Native builds have no OPFS and every entry point is a no-op.

/// Documents kept, the most recently written first: about as many as the Recent list shows.
pub const MAX_DOCS: usize = 20;
/// Largest document kept whole. Bigger ones still open, edit and download; they just aren't kept.
pub const MAX_DOC_BYTES: usize = 16 << 20;
/// Longest file name kept (a picker's names are short; anything else is refused).
pub const MAX_NAME_BYTES: usize = 255;
/// Files one eviction scan looks at, so a directory that grew without bound can't spin it.
#[cfg(target_arch = "wasm32")]
const MAX_SCAN: usize = 1000;

/// The OPFS directory the documents live in.
#[cfg(target_arch = "wasm32")]
const DIR: &str = "documents";

/// Whether a document with this name and size belongs in the store at all. A name is taken as
/// the picker gave it: a path separator, a `..`, or control characters make it a path wearing a
/// file's name, and the store keeps files, not a directory tree.
pub fn storable(name: &str, bytes: usize) -> bool {
    !name.is_empty()
        && name.len() <= MAX_NAME_BYTES
        && bytes <= MAX_DOC_BYTES
        && !name.bytes().any(|b| b < 0x20 || b == 0x7f)
        && !name.contains(['/', '\\'])
        && name != ".."
        && name != "."
}

/// Which of the stored files (name, last-written unix milliseconds) to delete to hold
/// [`MAX_DOCS`]: the oldest ones. Equal stamps keep the earlier name (scan order), so eviction
/// stays deterministic.
pub fn surplus(files: &[(String, f64)]) -> Vec<String> {
    let mut kept: Vec<&(String, f64)> = files.iter().collect();
    kept.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal).then_with(|| a.0.cmp(&b.0)));
    kept.into_iter().skip(MAX_DOCS).map(|(name, _)| name.clone()).collect()
}

/// Keep a document's bytes under its name, evicting what no longer fits. Fire-and-forget.
#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
pub fn store(name: &str, bytes: &[u8]) {
    #[cfg(target_arch = "wasm32")]
    {
        let (name, bytes) = (name.to_string(), bytes.to_vec());
        wasm_bindgen_futures::spawn_local(async move {
            if let Err(e) = store_async(name.clone(), bytes).await {
                log::error!("couldn't keep the document {name} for the next visit: {e}");
            }
        });
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        _ = (name, bytes);
    }
}

/// Read a kept document, or `None` when this browser doesn't hold it (never stored, evicted, or
/// the store is unavailable). Fire-and-forget: `then` runs on the answer.
#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
pub fn open(name: &str, then: impl FnOnce(Option<Vec<u8>>) + 'static) {
    #[cfg(target_arch = "wasm32")]
    {
        let name = name.to_string();
        wasm_bindgen_futures::spawn_local(async move {
            let bytes = load_async(&name).await;
            if let Err(e) = &bytes {
                log::warn!("couldn't read the kept document {name}: {e}");
            }
            then(bytes.unwrap_or(None));
        });
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        _ = name;
        then(None);
    }
}

/// Ask once that the browser not evict what this origin stores (documents, fonts) under
/// storage pressure. The browser may still say no; nothing waits on the answer.
#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
pub fn persist() {
    #[cfg(target_arch = "wasm32")]
    {
        wasm_bindgen_futures::spawn_local(async {
            let Some(window) = web_sys::window() else { return };
            let Ok(promise) = window.navigator().storage().persist() else {
                return;
            };
            if let Ok(answer) = wasm_bindgen_futures::JsFuture::from(promise).await
                && answer != wasm_bindgen::JsValue::TRUE
            {
                log::info!("the browser won't mark this origin's storage persistent");
            }
        });
    }
}

/// The positions file (#local): one JSON map, name → `[page, x, y]` in page points, kept at
/// the root of the browser's file system beside the documents directory.
#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
const POSITIONS_FILE: &str = "positions.json";

/// Where each document was last read to (#local): a page and a point on it, in page points.
/// Held in memory (`None` until the file has been read), written as one small JSON file.
static POSITIONS: std::sync::Mutex<Option<std::collections::HashMap<String, (usize, f32, f32)>>> = std::sync::Mutex::new(None);

/// The positions map, empty until [load_positions] has read the file.
fn positions() -> std::collections::HashMap<String, (usize, f32, f32)> {
    POSITIONS.lock().unwrap_or_else(std::sync::PoisonError::into_inner).clone().unwrap_or_default()
}

/// Remember where a document was read to (#local). Fire-and-forget: the write lands when it
/// lands; the position is a convenience, never a dependency.
#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
pub fn store_position(name: &str, page: usize, x: f32, y: f32) {
    if !storable(name, 0) || !x.is_finite() || !y.is_finite() {
        return;
    }
    {
        let mut held = POSITIONS.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        held.get_or_insert_with(Default::default).insert(name.to_string(), (page, x, y));
    }
    #[cfg(target_arch = "wasm32")]
    write_positions(positions());
}

/// Where a document was last read to (#local), if this browser remembers.
#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
pub fn load_position(name: &str) -> Option<(usize, f32, f32)> {
    positions().get(name).copied()
}

/// Drop the positions of documents the store no longer keeps: their bytes are gone, and with
/// them the point of resuming (#local).
#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
pub fn forget_positions(names: &[String]) {
    let mut held = POSITIONS.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    if let Some(map) = held.as_mut() {
        map.retain(|name, _| !names.contains(name));
    }
}

/// The positions map as JSON: `{"Report.docx": [2, 0.0, 640.5], …}`.
fn positions_to_json(map: &std::collections::HashMap<String, (usize, f32, f32)>) -> String {
    let mut out = String::from("{");
    for (i, (name, (page, x, y))) in map.iter().enumerate() {
        if i > 0 {
            out.push(',');
        }
        out.push_str(&serde_json::to_string(name).unwrap_or_default());
        out.push_str(&format!(":[{page},{x},{y}]"));
    }
    out.push('}');
    out
}

/// The positions map from JSON. A file that doesn't parse at all reads as no positions — a
/// fresh start — and inside one that does, a row that isn't a page and two finite numbers is
/// dropped, not argued with (the file is the browser's to corrupt).
fn positions_from_json(text: &str) -> std::collections::HashMap<String, (usize, f32, f32)> {
    let Ok(serde_json::Value::Object(rows)) = serde_json::from_str::<serde_json::Value>(text) else {
        return Default::default();
    };
    rows.into_iter()
        .filter_map(|(name, v)| {
            let nums = v.as_array()?.iter().map(|n| n.as_f64()).collect::<Vec<_>>();
            match nums.as_slice() {
                [Some(page), Some(x), Some(y)] => Some((name, (*page, *x, *y))),
                _ => None,
            }
        })
        .filter(|(_, (page, x, y))| *page >= 0.0 && *page <= 1e6 && x.is_finite() && y.is_finite())
        .map(|(name, (page, x, y))| (name, (page as usize, x as f32, y as f32)))
        .collect()
}

/// Read the positions file into memory, once, at startup (#local).
#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
pub fn load_positions() {
    #[cfg(target_arch = "wasm32")]
    wasm_bindgen_futures::spawn_local(async {
        match opfs_positions::read_positions_async().await {
            Ok(map) => *POSITIONS.lock().unwrap_or_else(std::sync::PoisonError::into_inner) = Some(map),
            Err(e) => log::warn!("couldn't read the kept reading positions: {e}"),
        }
    });
    #[cfg(not(target_arch = "wasm32"))]
    {}
}

#[cfg(target_arch = "wasm32")]
fn write_positions(map: std::collections::HashMap<String, (usize, f32, f32)>) {
    wasm_bindgen_futures::spawn_local(async {
        if let Err(e) = opfs_positions::write_positions_async(&map).await {
            log::warn!("couldn't keep the reading positions: {e}");
        }
    });
}

#[cfg(target_arch = "wasm32")]
mod opfs {
    use wasm_bindgen::{JsCast as _, JsValue};
    use wasm_bindgen_futures::JsFuture;
    use web_sys::{FileSystemDirectoryHandle, FileSystemFileHandle, FileSystemHandle, FileSystemWritableFileStream};

    use super::{DIR, MAX_SCAN, storable, surplus};

    fn js(e: JsValue) -> String {
        format!("{e:?}")
    }

    /// The documents directory, made on first use.
    async fn directory() -> Result<FileSystemDirectoryHandle, String> {
        let window = web_sys::window().ok_or("no window")?;
        let root = FileSystemDirectoryHandle::from(JsFuture::from(window.navigator().storage().get_directory()).await.map_err(js)?);
        let mut make = web_sys::FileSystemGetDirectoryOptions::new();
        make.create(true);
        Ok(FileSystemDirectoryHandle::from(JsFuture::from(root.get_directory_handle_with_options(DIR, &make)).await.map_err(js)?))
    }

    pub async fn store_async(name: String, bytes: Vec<u8>) -> Result<(), String> {
        if !storable(&name, bytes.len()) {
            return Ok(());
        }
        let dir = directory().await?;
        let mut make = web_sys::FileSystemGetFileOptions::new();
        make.create(true);
        let file = FileSystemFileHandle::from(JsFuture::from(dir.get_file_handle_with_options(&name, &make)).await.map_err(js)?);
        let stream = FileSystemWritableFileStream::from(JsFuture::from(file.create_writable()).await.map_err(js)?);
        wasm_bindgen_futures::JsFuture::from(FileSystemWritableFileStream::write_with_u8_array(&stream, &bytes).map_err(js)?).await.map_err(js)?;
        wasm_bindgen_futures::JsFuture::from(web_sys::WritableStream::close(stream.as_ref())).await.map_err(js)?;
        evict(&dir).await;
        Ok(())
    }

    pub async fn load_async(name: &str) -> Result<Option<Vec<u8>>, String> {
        if !storable(name, 0) {
            return Ok(None);
        }
        let dir = directory().await?;
        // No create: a name this browser never kept is a miss, not an error.
        let Ok(found) = JsFuture::from(dir.get_file_handle(name)).await else {
            return Ok(None);
        };
        let file = FileSystemFileHandle::from(found);
        let blob = web_sys::File::from(JsFuture::from(file.get_file()).await.map_err(js)?);
        let bytes = js_sys::Uint8Array::new(&JsFuture::from(blob.array_buffer()).await.map_err(js)?).to_vec();
        Ok(Some(bytes))
    }

    /// Delete the oldest files past [`super::MAX_DOCS`] (`last_modified` is when the write
    /// landed, so it orders by recency). A delete that fails is logged and left: an orphan
    /// costs quota, never correctness.
    async fn evict(dir: &FileSystemDirectoryHandle) {
        let scan: Result<Vec<(String, f64)>, String> = async {
            let mut files = Vec::new();
            let iter = dir.values();
            loop {
                if files.len() >= MAX_SCAN {
                    log::warn!("the document store holds more than {MAX_SCAN} files; not scanning further");
                    break;
                }
                let next = js_sys::IteratorNext::from(JsFuture::from(iter.next().map_err(js)?).await.map_err(js)?);
                if next.done() {
                    break;
                }
                let handle = FileSystemHandle::from(next.value());
                if handle.kind() == web_sys::FileSystemHandleKind::Directory {
                    continue;
                }
                let name = FileSystemHandle::name(&handle);
                let Ok(found) = JsFuture::from(dir.get_file_handle(&name)).await else { continue };
                let Ok(file) = JsFuture::from(FileSystemFileHandle::from(found).get_file()).await else { continue };
                let file = file.dyn_into::<web_sys::File>().map_err(js)?;
                files.push((name, file.last_modified()));
            }
            Ok(files)
        }
        .await;
        let files = match scan {
            Ok(files) => files,
            Err(e) => {
                log::warn!("couldn't scan the document store to evict: {e}");
                return;
            }
        };
        for name in surplus(&files) {
            if let Err(e) = JsFuture::from(dir.remove_entry(&name)).await {
                log::warn!("couldn't evict the stored document {name}: {e:?}");
            }
            super::forget_positions(&[name]);
        }
    }
}

#[cfg(target_arch = "wasm32")]
use opfs::{load_async, store_async};

#[cfg(target_arch = "wasm32")]
mod positions_io {
    use wasm_bindgen::{JsCast as _, JsValue};
    use wasm_bindgen_futures::JsFuture;
    use web_sys::{FileSystemDirectoryHandle, FileSystemFileHandle, FileSystemWritableFileStream};

    use super::{POSITIONS_FILE, positions_from_json, positions_to_json};

    fn js(e: JsValue) -> String {
        format!("{e:?}")
    }

    /// The root of the browser's file system, where the positions file lives beside the
    /// documents directory.
    async fn root() -> Result<FileSystemDirectoryHandle, String> {
        let window = web_sys::window().ok_or("no window")?;
        Ok(FileSystemDirectoryHandle::from(JsFuture::from(window.navigator().storage().get_directory()).await.map_err(js)?))
    }

    pub async fn read_positions_async() -> Result<std::collections::HashMap<String, (usize, f32, f32)>, String> {
        let root = root().await?;
        // No create: a first visit holds no positions, which is an empty map, not an error.
        let Ok(found) = JsFuture::from(root.get_file_handle(POSITIONS_FILE)).await else {
            return Ok(Default::default());
        };
        let file = FileSystemFileHandle::from(found);
        let blob = web_sys::File::from(JsFuture::from(file.get_file()).await.map_err(js)?);
        let text = js_sys::JsString::from(JsFuture::from(blob.text()).await.map_err(js)?).as_string().unwrap_or_default();
        Ok(positions_from_json(&text))
    }

    pub async fn write_positions_async(map: &std::collections::HashMap<String, (usize, f32, f32)>) -> Result<(), String> {
        let root = root().await?;
        let mut make = web_sys::FileSystemGetFileOptions::new();
        make.create(true);
        let file = FileSystemFileHandle::from(JsFuture::from(root.get_file_handle_with_options(POSITIONS_FILE, &make)).await.map_err(js)?);
        let stream = FileSystemWritableFileStream::from(JsFuture::from(file.create_writable()).await.map_err(js)?);
        let bytes = positions_to_json(map).into_bytes();
        wasm_bindgen_futures::JsFuture::from(FileSystemWritableFileStream::write_with_u8_array(&stream, &bytes).map_err(js)?).await.map_err(js)?;
        wasm_bindgen_futures::JsFuture::from(web_sys::WritableStream::close(stream.as_ref())).await.map_err(js)?;
        Ok(())
    }
}

#[cfg(target_arch = "wasm32")]
use positions_io as opfs_positions;

#[cfg(test)]
mod tests {
    use super::*;

    fn file(name: &str, ms: f64) -> (String, f64) {
        (name.to_string(), ms)
    }

    #[test]
    fn names_and_sizes_are_bounded() {
        assert!(storable("Report.docx", 1024));
        assert!(!storable("", 1024), "no name");
        assert!(!storable(&"x".repeat(256), 1024), "a name longer than the bound");
        assert!(!storable("a/b.docx", 1024), "a path, not a name");
        assert!(!storable("..", 1024), "a parent hop");
        assert!(!storable("bad\nname.docx", 1024), "a control character");
        assert!(!storable("Report.docx", (16 << 20) + 1), "over the byte bound");
        assert!(storable("Report.docx", 16 << 20), "the bound itself holds");
    }

    #[test]
    fn positions_round_trip_and_drop_what_isnt_a_point() {
        let mut map = std::collections::HashMap::new();
        map.insert("Report.docx".to_string(), (2, 0.0, 640.5));
        map.insert("Notes.txt".to_string(), (0, 12.25, 8.5));
        let text = positions_to_json(&map);
        assert_eq!(positions_from_json(&text), map, "the file comes back as it went in");
        // A row the browser corrupted, a negative page, an unbounded point: dropped, not fatal.
        let junk = r#"{"ok.docx":[1,0.0,10.0],"short.docx":[1],"words.docx":["one"],"neg.docx":[-1,0.0,0.0]}"#;
        let read = positions_from_json(junk);
        assert_eq!(read.len(), 1, "only the well-formed row survives");
        assert!(read.contains_key("ok.docx"));
        assert!(positions_from_json("not json").is_empty());
    }

    #[test]
    fn positions_live_in_memory_until_forgotten() {
        store_position("Report.docx", 2, 0.0, 640.5);
        assert_eq!(load_position("Report.docx"), Some((2, 0.0, 640.5)), "what was stored loads");
        assert_eq!(load_position("Elsewhere.docx"), None, "an unkept document has no position");
        store_position("bad/name.docx", 0, 0.0, 0.0);
        assert_eq!(load_position("bad/name.docx"), None, "a path is not kept");
        forget_positions(&["Report.docx".to_string()]);
        assert_eq!(load_position("Report.docx"), None, "an evicted document's position goes with it");
    }

    #[test]
    fn the_oldest_documents_beyond_the_cap_are_evicted() {
        let files: Vec<(String, f64)> = (0..MAX_DOCS + 3).map(|i| file(&format!("d{i}.docx"), i as f64)).collect();
        assert_eq!(surplus(&files), vec!["d2.docx", "d1.docx", "d0.docx"], "the oldest go, newest-first order");
        assert_eq!(surplus(&files[..MAX_DOCS]), Vec::<String>::new(), "a full store keeps everything");
        // A stamp the JS side couldn't compare (NaN) sorts as the newest known place, not a panic.
        assert_eq!(surplus(&[file("nan.docx", f64::NAN), file("old.docx", 1.0)]), Vec::<String>::new());
    }
}
