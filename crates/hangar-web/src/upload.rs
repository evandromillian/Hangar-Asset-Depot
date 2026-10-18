//! Client-side upload helpers: collecting files (incl. dropped folders),
//! BLAKE3 hashing in wasm, and relative-path normalisation.

use js_sys::{Array, Promise, Reflect};
use wasm_bindgen::prelude::*;
use wasm_bindgen::JsCast;
use wasm_bindgen_futures::JsFuture;
use web_sys::{Blob, File};

/// A file picked by the user, with its path relative to the version root.
#[derive(Clone)]
pub struct PickedFile {
    pub path: String,
    pub file: File,
}

impl PickedFile {
    pub fn size(&self) -> f64 {
        self.file.size()
    }
}

const CHUNK: f64 = 8.0 * 1024.0 * 1024.0;

/// Hashes a browser `File` with BLAKE3, reading it in 8 MiB slices.
/// `progress` receives the fraction done after each slice.
pub async fn hash_file(file: &File, mut progress: impl FnMut(f64)) -> Result<String, String> {
    let size = file.size();
    let mut hasher = blake3::Hasher::new();
    let mut offset = 0.0;
    while offset < size {
        let end = (offset + CHUNK).min(size);
        let blob: Blob = file.slice_with_f64_and_f64(offset, end).map_err(js_err)?;
        let buf = JsFuture::from(blob.array_buffer()).await.map_err(js_err)?;
        let bytes = js_sys::Uint8Array::new(&buf).to_vec();
        hasher.update(&bytes);
        offset = end;
        progress(if size > 0.0 { offset / size } else { 1.0 });
    }
    Ok(hasher.finalize().to_hex().to_string())
}

fn js_err(e: JsValue) -> String {
    e.as_string()
        .or_else(|| Reflect::get(&e, &"message".into()).ok().and_then(|m| m.as_string()))
        .unwrap_or_else(|| format!("{e:?}"))
}

/// `File.webkitRelativePath` (set when picked from a directory input).
pub fn relative_path(file: &File) -> String {
    Reflect::get(file, &"webkitRelativePath".into())
        .ok()
        .and_then(|v| v.as_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| file.name())
}

/// If every path starts with the same top folder (`Tank/…`), drop it so the
/// version tree starts at the asset's content.
pub fn strip_common_top(files: &mut [PickedFile]) {
    if files.len() < 1 {
        return;
    }
    let first_top = match files[0].path.split_once('/') {
        Some((top, _)) => top.to_string(),
        None => return,
    };
    let all_share = files.iter().all(|f| f.path.split_once('/').map(|(t, _)| t == first_top).unwrap_or(false));
    if all_share {
        for f in files.iter_mut() {
            f.path = f.path[first_top.len() + 1..].to_string();
        }
    }
}

pub fn from_file_list(list: &web_sys::FileList) -> Vec<PickedFile> {
    let mut out = Vec::new();
    for i in 0..list.length() {
        if let Some(file) = list.get(i) {
            let path = hangar_core::paths::normalize(&relative_path(&file)).unwrap_or_else(|| file.name());
            out.push(PickedFile { path, file });
        }
    }
    out
}

// --------------------------------------------------------- drag & drop folders

#[wasm_bindgen]
extern "C" {
    /// `FileSystemEntry` (File and Directory Entries API).
    #[derive(Clone)]
    type FsEntry;
    #[wasm_bindgen(method, getter, js_name = isDirectory)]
    fn is_directory(this: &FsEntry) -> bool;
    #[wasm_bindgen(method, getter, js_name = fullPath)]
    fn full_path(this: &FsEntry) -> String;
    #[wasm_bindgen(method)]
    fn file(this: &FsEntry, ok: &js_sys::Function, err: &js_sys::Function);
    #[wasm_bindgen(method, js_name = createReader)]
    fn create_reader(this: &FsEntry) -> DirReader;

    type DirReader;
    #[wasm_bindgen(method, js_name = readEntries)]
    fn read_entries(this: &DirReader, ok: &js_sys::Function, err: &js_sys::Function);
}

/// Must be called synchronously inside the `drop` handler: the
/// DataTransferItems are invalidated once the event returns.
pub fn drop_entries(dt: &web_sys::DataTransfer) -> Vec<JsValue> {
    let mut out = Vec::new();
    let Ok(items) = Reflect::get(dt, &"items".into()) else { return out };
    let len = Reflect::get(&items, &"length".into()).ok().and_then(|v| v.as_f64()).unwrap_or(0.0) as u32;
    for i in 0..len {
        let Ok(item) = Reflect::get(&items, &JsValue::from(i)) else { continue };
        let Ok(f) = Reflect::get(&item, &"webkitGetAsEntry".into()) else { continue };
        let Some(f) = f.dyn_ref::<js_sys::Function>() else { continue };
        if let Ok(entry) = f.call0(&item) {
            if !entry.is_null() && !entry.is_undefined() {
                out.push(entry);
            }
        }
    }
    out
}

async fn entry_file(entry: &FsEntry) -> Result<File, String> {
    let e = entry.clone();
    let p = Promise::new(&mut |res, rej| e.file(&res, &rej));
    JsFuture::from(p).await.map(|v| v.unchecked_into()).map_err(js_err)
}

async fn read_dir_batch(reader: &DirReader) -> Result<Array, String> {
    let p = Promise::new(&mut |res, rej| reader.read_entries(&res, &rej));
    JsFuture::from(p).await.map(|v| v.unchecked_into()).map_err(js_err)
}

/// Walks dropped entries (files and directories, recursively).
pub async fn collect_entries(entries: Vec<JsValue>) -> Result<Vec<PickedFile>, String> {
    let mut stack: Vec<FsEntry> = entries.into_iter().map(|e| e.unchecked_into()).collect();
    let mut out = Vec::new();
    while let Some(entry) = stack.pop() {
        if entry.is_directory() {
            let reader = entry.create_reader();
            // readEntries returns at most ~100 entries per call.
            loop {
                let batch = read_dir_batch(&reader).await?;
                if batch.length() == 0 {
                    break;
                }
                for v in batch.iter() {
                    stack.push(v.unchecked_into());
                }
            }
        } else {
            let file = entry_file(&entry).await?;
            let path = hangar_core::paths::normalize(&entry.full_path()).unwrap_or_else(|| file.name());
            out.push(PickedFile { path, file });
        }
    }
    out.sort_by(|a, b| a.path.cmp(&b.path));
    Ok(out)
}
