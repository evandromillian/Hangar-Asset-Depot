//! Zip export of a version tree.

use crate::error::{Ctx, Result};
use crate::storage::BlobStore;
use hangar_core::FileEntry;
use std::io::{Seek, Write};
use zip::write::SimpleFileOptions;
use zip::{CompressionMethod, ZipWriter};

/// Text-like files compress well; meshes/textures are usually already compressed.
fn compression_for(path: &str) -> CompressionMethod {
    match hangar_core::paths::extension(path).as_str() {
        "obj" | "mtl" | "gltf" | "txt" | "md" | "json" | "fbx" | "dae" | "ply" | "stl" | "usda" | "tga" | "bmp" => {
            CompressionMethod::Deflated
        }
        _ => CompressionMethod::Stored,
    }
}

/// Builds the zip in an anonymous temp file (deleted when the handle drops)
/// and returns it rewound to the start.
pub async fn build_zip(store: &BlobStore, tree: &[FileEntry]) -> Result<tokio::fs::File> {
    tokio::fs::create_dir_all(store.temp_dir()).await.ctx("temp dir")?;
    let out = tempfile::tempfile_in(store.temp_dir()).ctx("create zip file")?;
    let mut zip = Some(ZipWriter::new(out));

    for entry in tree {
        let blob = tempfile::NamedTempFile::new_in(store.temp_dir()).ctx("blob temp")?;
        store.download_to(&entry.hash, blob.path()).await?;
        let mut writer = zip.take().expect("zip writer present");
        let path = entry.path.clone();
        let large = entry.size >= u32::MAX as i64;
        writer = tokio::task::spawn_blocking(move || -> std::io::Result<ZipWriter<std::fs::File>> {
            let opts = SimpleFileOptions::default()
                .compression_method(compression_for(&path))
                .large_file(large);
            writer.start_file(path, opts).map_err(std::io::Error::other)?;
            let mut src = std::fs::File::open(blob.path())?;
            std::io::copy(&mut src, &mut writer)?;
            Ok(writer)
        })
        .await
        .ctx("join")?
        .ctx("write zip entry")?;
        zip = Some(writer);
    }

    let file = tokio::task::spawn_blocking(move || -> std::io::Result<std::fs::File> {
        let mut file = zip.take().expect("zip writer").finish().map_err(std::io::Error::other)?;
        file.flush()?;
        file.rewind()?;
        Ok(file)
    })
    .await
    .ctx("join")?
    .ctx("finish zip")?;
    Ok(tokio::fs::File::from_std(file))
}
