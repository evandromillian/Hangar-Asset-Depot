//! Version-relative file paths.
//!
//! A version is a tree of files (`path -> blob`). Paths are always relative,
//! use `/` as the separator and never escape the version root.

/// Normalises a client supplied path (`.\Textures\Desert\T_Body_D.png` →
/// `Textures/Desert/T_Body_D.png`). Returns `None` for paths that are empty,
/// absolute on another drive, contain `..` or control characters.
pub fn normalize(path: &str) -> Option<String> {
    let path = path.replace('\\', "/");
    if path.len() > 1024 {
        return None;
    }
    let mut out: Vec<&str> = Vec::new();
    for (i, seg) in path.split('/').enumerate() {
        if seg.is_empty() || seg == "." {
            continue;
        }
        if seg == ".." {
            return None;
        }
        if i == 0 && seg.len() == 2 && seg.ends_with(':') {
            // "C:" drive prefix
            return None;
        }
        if seg.chars().any(|c| c.is_control()) {
            return None;
        }
        out.push(seg);
    }
    if out.is_empty() { None } else { Some(out.join("/")) }
}

/// Last path segment.
pub fn file_name(path: &str) -> &str {
    path.rsplit(['/', '\\']).next().unwrap_or(path)
}

/// File name without the (last) extension.
pub fn stem(path: &str) -> &str {
    let name = file_name(path);
    match name.rfind('.') {
        Some(0) | None => name,
        Some(i) => &name[..i],
    }
}

/// Lower-cased extension without the dot (`"fbx"`), or empty.
pub fn extension(path: &str) -> String {
    let name = file_name(path);
    match name.rfind('.') {
        Some(i) if i > 0 => name[i + 1..].to_ascii_lowercase(),
        _ => String::new(),
    }
}

/// Parent directory ("" for files at the root).
pub fn parent(path: &str) -> &str {
    match path.rfind('/') {
        Some(i) => &path[..i],
        None => "",
    }
}

pub const MODEL_EXTENSIONS: &[&str] = &["glb", "gltf", "fbx", "obj", "blend", "dae", "3ds", "stl", "ply"];
pub const IMAGE_EXTENSIONS: &[&str] = &[
    "png", "jpg", "jpeg", "tga", "bmp", "tif", "tiff", "webp", "hdr", "exr", "dds", "ktx2", "psd",
];
pub const AUDIO_EXTENSIONS: &[&str] = &["wav", "ogg", "mp3", "flac", "aif", "aiff", "opus"];

pub fn is_model(path: &str) -> bool {
    MODEL_EXTENSIONS.contains(&extension(path).as_str())
}
pub fn is_image(path: &str) -> bool {
    IMAGE_EXTENSIONS.contains(&extension(path).as_str())
}
pub fn is_audio(path: &str) -> bool {
    AUDIO_EXTENSIONS.contains(&extension(path).as_str())
}

/// Best-effort MIME type from the extension.
pub fn mime_for(path: &str) -> &'static str {
    match extension(path).as_str() {
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "webp" => "image/webp",
        "tga" => "image/x-tga",
        "bmp" => "image/bmp",
        "tif" | "tiff" => "image/tiff",
        "ktx2" => "image/ktx2",
        "glb" => "model/gltf-binary",
        "gltf" => "model/gltf+json",
        "obj" => "model/obj",
        "fbx" => "application/octet-stream",
        "wav" => "audio/wav",
        "ogg" | "opus" => "audio/ogg",
        "mp3" => "audio/mpeg",
        "flac" => "audio/flac",
        "json" => "application/json",
        "txt" | "md" => "text/plain; charset=utf-8",
        "pdf" => "application/pdf",
        "zip" => "application/zip",
        _ => "application/octet-stream",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalizes() {
        assert_eq!(normalize(r".\Textures\Desert\T_D.png").as_deref(), Some("Textures/Desert/T_D.png"));
        assert_eq!(normalize("/a//b/./c.fbx").as_deref(), Some("a/b/c.fbx"));
        assert_eq!(normalize("../etc/passwd"), None);
        assert_eq!(normalize("a/../b"), None);
        assert_eq!(normalize("C:/x.png"), None);
        assert_eq!(normalize(""), None);
        assert_eq!(normalize("./"), None);
    }

    #[test]
    fn name_parts() {
        assert_eq!(file_name("a/b/Tank_LOD1.FBX"), "Tank_LOD1.FBX");
        assert_eq!(stem("a/b/Tank_LOD1.FBX"), "Tank_LOD1");
        assert_eq!(extension("a/b/Tank_LOD1.FBX"), "fbx");
        assert_eq!(extension("a/.hidden"), "");
        assert_eq!(parent("a/b/c.png"), "a/b");
        assert_eq!(parent("c.png"), "");
        assert!(is_model("x.Fbx"));
        assert!(is_image("x.TGA"));
    }
}
