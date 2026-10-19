//! Image decoding, resizing and web encoding with a per-run cache.

use image::{DynamicImage, GenericImageView, ImageFormat, RgbaImage};
use std::collections::HashMap;
use std::io::Cursor;
use std::path::Path;
use std::sync::Arc;

/// A decoded source image, already capped to the pipeline's max size.
pub struct Decoded {
    pub image: Arc<RgbaImage>,
    pub orig_width: u32,
    pub orig_height: u32,
    pub format: String,
    pub has_alpha: bool,
}

/// Web-encoded image bytes and their MIME type.
pub type Encoded = Arc<(Vec<u8>, &'static str)>;

/// Decodes each image at most once, and encodes each (image, size) once.
pub struct ImageCache {
    max_size: u32,
    cache: HashMap<String, Option<Arc<Decoded>>>,
    encoded: HashMap<(usize, u32, bool), Encoded>,
}

impl ImageCache {
    pub fn new(max_size: u32) -> Self {
        Self { max_size: max_size.max(64), cache: HashMap::new(), encoded: HashMap::new() }
    }

    /// Decode a version file (`key` = version-relative path).
    pub fn get_file(&mut self, root: &Path, key: &str) -> Option<Arc<Decoded>> {
        if let Some(v) = self.cache.get(key) {
            return v.clone();
        }
        let res = std::fs::read(root.join(key)).ok().and_then(|b| decode_bytes(&b, Some(key), self.max_size));
        let res = res.map(Arc::new);
        self.cache.insert(key.to_string(), res.clone());
        res
    }

    /// Web-encode a decoded image at `max` size, memoised by image identity.
    pub fn encode(&mut self, d: &Arc<Decoded>, max: u32) -> Encoded {
        let effective = max.min(d.image.width().max(d.image.height()));
        let key = (Arc::as_ptr(d) as usize, effective, d.has_alpha);
        self.encoded
            .entry(key)
            .or_insert_with(|| Arc::new(encode_web(&d.image, d.has_alpha, effective)))
            .clone()
    }

    /// Decode embedded bytes, keyed by a synthetic name.
    pub fn get_bytes(&mut self, key: &str, bytes: &[u8]) -> Option<Arc<Decoded>> {
        if let Some(v) = self.cache.get(key) {
            return v.clone();
        }
        let res = decode_bytes(bytes, None, self.max_size).map(Arc::new);
        self.cache.insert(key.to_string(), res.clone());
        res
    }
}

/// Whether the `image` crate (with our enabled features) can decode this extension.
pub fn is_decodable_ext(ext: &str) -> bool {
    matches!(ext, "png" | "jpg" | "jpeg" | "tga" | "bmp" | "tif" | "tiff" | "webp" | "hdr")
}

fn decode_bytes(bytes: &[u8], name: Option<&str>, max: u32) -> Option<Decoded> {
    let guessed = image::guess_format(bytes).ok();
    let by_ext = name.and_then(|n| ImageFormat::from_extension(hangar_core::paths::extension(n)));
    // TGA has no magic number: fall back to the extension.
    let fmt = guessed.or(by_ext)?;
    let img = image::load_from_memory_with_format(bytes, fmt).ok()?;
    let (w, h) = img.dimensions();
    let has_alpha = img.color().has_alpha();
    let img = if w.max(h) > max {
        let (nw, nh) = scale_dim(w, h, max);
        img.thumbnail_exact(nw, nh)
    } else {
        img
    };
    let mut rgba = match img {
        DynamicImage::ImageRgb32F(_) | DynamicImage::ImageRgba32F(_) => tonemap_hdr(&img),
        _ => img.to_rgba8(),
    };
    // Many textures carry a fully opaque alpha channel: treat them as opaque.
    let source_alpha = has_alpha;
    let has_alpha = source_alpha && rgba.pixels().any(|p| p.0[3] < 250);
    if source_alpha && !has_alpha {
        for p in rgba.pixels_mut() {
            p.0[3] = 255;
        }
    }
    Some(Decoded {
        image: Arc::new(rgba),
        orig_width: w,
        orig_height: h,
        format: format!("{fmt:?}").to_ascii_lowercase(),
        has_alpha,
    })
}

/// Reinhard tone mapping for HDR previews.
fn tonemap_hdr(img: &DynamicImage) -> RgbaImage {
    let f = img.to_rgba32f();
    let mut out = RgbaImage::new(f.width(), f.height());
    for (o, p) in out.pixels_mut().zip(f.pixels()) {
        let map = |v: f32| ((v / (1.0 + v)).powf(1.0 / 2.2) * 255.0).clamp(0.0, 255.0) as u8;
        o.0 = [map(p.0[0]), map(p.0[1]), map(p.0[2]), 255];
    }
    out
}

/// Downscale (never upscale) to fit `max` and encode for the web:
/// PNG when the image has transparency, JPEG q85 otherwise.
pub fn encode_web(img: &RgbaImage, has_alpha: bool, max: u32) -> (Vec<u8>, &'static str) {
    let resized;
    let img = if img.width().max(img.height()) > max {
        let (nw, nh) = scale_dim(img.width(), img.height(), max);
        // Area-averaging downscale: much faster than a convolution filter and
        // just as good for halving texture sizes.
        resized = image::imageops::thumbnail(img, nw, nh);
        &resized
    } else {
        img
    };
    let mut buf = Cursor::new(Vec::new());
    if has_alpha {
        img.write_to(&mut buf, ImageFormat::Png).expect("png encode");
        (buf.into_inner(), "image/png")
    } else {
        let rgb = DynamicImage::ImageRgba8(img.clone()).to_rgb8();
        let enc = image::codecs::jpeg::JpegEncoder::new_with_quality(&mut buf, 85);
        rgb.write_with_encoder(enc).expect("jpeg encode");
        (buf.into_inner(), "image/jpeg")
    }
}

pub fn encode_png(img: &RgbaImage) -> Vec<u8> {
    let mut buf = Cursor::new(Vec::new());
    img.write_to(&mut buf, ImageFormat::Png).expect("png encode");
    buf.into_inner()
}

fn scale_dim(w: u32, h: u32, max: u32) -> (u32, u32) {
    if w >= h {
        (max, ((h as f32 * max as f32 / w as f32).round() as u32).max(1))
    } else {
        (((w as f32 * max as f32 / h as f32).round() as u32).max(1), max)
    }
}

/// Resize `img` to exactly `w`x`h` if needed.
pub fn fit_exact(img: &RgbaImage, w: u32, h: u32) -> RgbaImage {
    if img.width() == w && img.height() == h {
        img.clone()
    } else {
        if w <= img.width() && h <= img.height() {
            image::imageops::thumbnail(img, w, h)
        } else {
            image::imageops::resize(img, w, h, image::imageops::FilterType::Triangle)
        }
    }
}

/// Pack separate roughness / metallic maps into a glTF metallicRoughness
/// texture (G = roughness, B = metallic). Missing maps use the constant.
pub fn pack_metal_rough(rough: Option<&RgbaImage>, metal: Option<&RgbaImage>, rough_k: f32, metal_k: f32) -> RgbaImage {
    let (w, h) = rough
        .map(|i| i.dimensions())
        .or(metal.map(|i| i.dimensions()))
        .unwrap_or((4, 4));
    let rough = rough.map(|i| fit_exact(i, w, h));
    let metal = metal.map(|i| fit_exact(i, w, h));
    let mut out = RgbaImage::new(w, h);
    for (x, y, p) in out.enumerate_pixels_mut() {
        let r = rough.as_ref().map(|i| i.get_pixel(x, y).0[0]).unwrap_or((rough_k * 255.0) as u8);
        let m = metal.as_ref().map(|i| i.get_pixel(x, y).0[0]).unwrap_or((metal_k * 255.0) as u8);
        p.0 = [255, r, m, 255];
    }
    out
}

/// Copy with a fully opaque alpha channel.
pub fn without_alpha(img: &RgbaImage) -> RgbaImage {
    let mut out = img.clone();
    for p in out.pixels_mut() {
        p.0[3] = 255;
    }
    out
}

/// Put an opacity map into the alpha channel of a base colour map. The map's own
/// alpha channel is the opacity when it has one (a colour texture whose alpha is a
/// cutout, e.g. the base colour itself); a grayscale map gives its red channel.
pub fn merge_alpha(base: &RgbaImage, opacity: &Decoded) -> RgbaImage {
    let channel = if opacity.has_alpha { 3 } else { 0 };
    let op = fit_exact(&opacity.image, base.width(), base.height());
    let mut out = base.clone();
    for (o, a) in out.pixels_mut().zip(op.pixels()) {
        o.0[3] = a.0[channel];
    }
    out
}

/// Sample with bilinear filtering and repeat wrapping (UV in glTF convention).
pub fn sample(img: &RgbaImage, u: f32, v: f32) -> [f32; 4] {
    let (w, h) = (img.width() as f32, img.height() as f32);
    let x = (u - u.floor()) * w - 0.5;
    let y = (v - v.floor()) * h - 0.5;
    let x0 = x.floor();
    let y0 = y.floor();
    let fx = x - x0;
    let fy = y - y0;
    let wi = img.width() as i64;
    let hi = img.height() as i64;
    let px = |xx: i64, yy: i64| {
        let p = img.get_pixel(xx.rem_euclid(wi) as u32, yy.rem_euclid(hi) as u32).0;
        [p[0] as f32, p[1] as f32, p[2] as f32, p[3] as f32]
    };
    let (x0, y0) = (x0 as i64, y0 as i64);
    let a = px(x0, y0);
    let b = px(x0 + 1, y0);
    let c = px(x0, y0 + 1);
    let d = px(x0 + 1, y0 + 1);
    let mut out = [0.0; 4];
    for i in 0..4 {
        let top = a[i] + (b[i] - a[i]) * fx;
        let bot = c[i] + (d[i] - c[i]) * fx;
        out[i] = (top + (bot - top) * fy) / 255.0;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn encode_choice() {
        let mut img = RgbaImage::from_pixel(8, 8, image::Rgba([10, 20, 30, 255]));
        let (b, mime) = encode_web(&img, false, 4);
        assert_eq!(mime, "image/jpeg");
        assert_eq!(image::load_from_memory(&b).unwrap().dimensions(), (4, 4));
        img.put_pixel(0, 0, image::Rgba([0, 0, 0, 0]));
        let (_, mime) = encode_web(&img, true, 16);
        assert_eq!(mime, "image/png");
    }

    #[test]
    fn packing() {
        let r = RgbaImage::from_pixel(2, 2, image::Rgba([100, 100, 100, 255]));
        let p = pack_metal_rough(Some(&r), None, 0.5, 1.0);
        assert_eq!(p.get_pixel(0, 0).0, [255, 100, 255, 255]);
    }

    #[test]
    fn opacity_channel() {
        let base = RgbaImage::from_pixel(2, 2, image::Rgba([1, 2, 3, 255]));
        let decoded = |px: [u8; 4]| {
            let d = decode_bytes(&encode_png(&RgbaImage::from_pixel(2, 2, image::Rgba(px))), Some("o.png"), 16);
            d.unwrap()
        };
        // Grayscale map (opaque alpha): red channel.
        assert_eq!(merge_alpha(&base, &decoded([90, 90, 90, 255])).get_pixel(0, 0).0, [1, 2, 3, 90]);
        // Colour texture with a cutout in alpha: the alpha channel, not its colour.
        assert_eq!(merge_alpha(&base, &decoded([200, 150, 100, 40])).get_pixel(0, 0).0, [1, 2, 3, 40]);
    }

    #[test]
    fn decode_and_cap() {
        let img = RgbaImage::from_pixel(64, 32, image::Rgba([1, 2, 3, 255]));
        let png = encode_png(&img);
        let d = decode_bytes(&png, Some("a.png"), 16).unwrap();
        assert_eq!(d.image.dimensions(), (16, 8));
        assert_eq!((d.orig_width, d.orig_height), (64, 32));
        assert!(!d.has_alpha);
    }
}
