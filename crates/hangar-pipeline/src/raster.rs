//! Tiny CPU rasteriser used for thumbnails: no GPU needed on the server.

use crate::scene::{Aabb, MeshPrim};
use glam::{Mat4, Vec3, Vec4};
use image::{Rgba, RgbaImage};

pub struct ThumbMaterial<'a> {
    /// Linear base colour factor.
    pub base_color: [f32; 4],
    pub texture: Option<&'a RgbaImage>,
    pub alpha_test: bool,
}

fn srgb_to_linear(c: f32) -> f32 {
    c.max(0.0).powf(2.2)
}
fn linear_to_srgb(c: f32) -> f32 {
    c.max(0.0).powf(1.0 / 2.2)
}

/// Background: subtle vertical gradient.
fn background(y: u32, h: u32) -> [f32; 3] {
    let t = y as f32 / h.max(1) as f32;
    let top = [0.235, 0.255, 0.290];
    let bottom = [0.110, 0.120, 0.140];
    [
        top[0] + (bottom[0] - top[0]) * t,
        top[1] + (bottom[1] - top[1]) * t,
        top[2] + (bottom[2] - top[2]) * t,
    ]
}

/// Render `meshes` in a 3/4 view framing their bounds.
pub fn render_thumbnail(meshes: &[MeshPrim], materials: &[ThumbMaterial], size: u32) -> RgbaImage {
    let ss = 2u32;
    let w = size * ss;
    let h = size * ss;
    let mut color = vec![[0f32; 3]; (w * h) as usize];
    for y in 0..h {
        let bg = background(y, h);
        for x in 0..w {
            color[(y * w + x) as usize] = bg;
        }
    }
    let mut depth = vec![f32::INFINITY; (w * h) as usize];

    let bounds = Aabb::of_meshes(meshes);
    if bounds.is_valid() {
        let min = Vec3::from(bounds.min);
        let max = Vec3::from(bounds.max);
        let center = (min + max) * 0.5;
        let radius = ((max - min).length() * 0.5).max(1e-4);
        let fov = 30f32.to_radians();
        let dir = Vec3::new(0.9, 0.55, 1.25).normalize();
        let dist = radius / (fov * 0.5).sin() * 1.02;
        let eye = center + dir * dist;
        let view = Mat4::look_at_rh(eye, center, Vec3::Y);
        let proj = Mat4::perspective_rh(fov, 1.0, (dist - radius * 1.5).max(dist * 0.01), dist + radius * 1.5);
        let vp = proj * view;

        // Lights (world space): key from upper-left of the camera, fill from the right.
        let right = dir.cross(Vec3::Y).normalize_or_zero();
        let key = (dir + Vec3::Y * 1.2 - right * 0.6).normalize();
        let fill = (dir + right * 0.8 - Vec3::Y * 0.2).normalize();

        for m in meshes {
            let mat = m.material.and_then(|i| materials.get(i));
            let base = mat.map(|m| m.base_color).unwrap_or([0.6, 0.6, 0.6, 1.0]);
            let tex = mat.and_then(|m| m.texture);
            let alpha_test = mat.map(|m| m.alpha_test).unwrap_or(false);
            let has_n = m.normals.len() == m.positions.len();
            let has_uv = m.uvs.len() == m.positions.len() && tex.is_some();
            // project
            let proj_v: Vec<(Vec3, f32)> = m
                .positions
                .iter()
                .map(|p| {
                    let c = vp * Vec4::new(p[0], p[1], p[2], 1.0);
                    let iw = if c.w.abs() > 1e-8 { 1.0 / c.w } else { 0.0 };
                    let ndc = Vec3::new(c.x * iw, c.y * iw, c.z * iw);
                    let sx = (ndc.x * 0.5 + 0.5) * w as f32;
                    let sy = (1.0 - (ndc.y * 0.5 + 0.5)) * h as f32;
                    (Vec3::new(sx, sy, ndc.z), iw)
                })
                .collect();

            for tri in m.indices.chunks_exact(3) {
                let ids = [tri[0] as usize, tri[1] as usize, tri[2] as usize];
                if ids.iter().any(|&i| i >= proj_v.len()) {
                    continue;
                }
                let (a, b, c) = (proj_v[ids[0]], proj_v[ids[1]], proj_v[ids[2]]);
                if a.1 <= 0.0 || b.1 <= 0.0 || c.1 <= 0.0 {
                    continue;
                }
                let area = edge(a.0, b.0, c.0);
                if area.abs() < 1e-9 {
                    continue;
                }
                let face_n = {
                    let pa = Vec3::from(m.positions[ids[0]]);
                    let pb = Vec3::from(m.positions[ids[1]]);
                    let pc = Vec3::from(m.positions[ids[2]]);
                    (pb - pa).cross(pc - pa).normalize_or_zero()
                };
                let minx = a.0.x.min(b.0.x).min(c.0.x).floor().max(0.0) as u32;
                let maxx = a.0.x.max(b.0.x).max(c.0.x).ceil().min(w as f32 - 1.0) as i64;
                let miny = a.0.y.min(b.0.y).min(c.0.y).floor().max(0.0) as u32;
                let maxy = a.0.y.max(b.0.y).max(c.0.y).ceil().min(h as f32 - 1.0) as i64;
                if maxx < 0 || maxy < 0 {
                    continue;
                }
                for py in miny..=maxy as u32 {
                    for px in minx..=maxx as u32 {
                        let p = Vec3::new(px as f32 + 0.5, py as f32 + 0.5, 0.0);
                        let w0 = edge(b.0, c.0, p) / area;
                        let w1 = edge(c.0, a.0, p) / area;
                        let w2 = edge(a.0, b.0, p) / area;
                        if w0 < 0.0 || w1 < 0.0 || w2 < 0.0 {
                            continue;
                        }
                        let z = w0 * a.0.z + w1 * b.0.z + w2 * c.0.z;
                        let di = (py * w + px) as usize;
                        if z >= depth[di] || !(-1.0..=1.0).contains(&z) {
                            continue;
                        }
                        // perspective-correct weights
                        let pw0 = w0 * a.1;
                        let pw1 = w1 * b.1;
                        let pw2 = w2 * c.1;
                        let s = pw0 + pw1 + pw2;
                        let (pw0, pw1, pw2) = (pw0 / s, pw1 / s, pw2 / s);

                        // base colour factor is linear (glTF convention), textures are sRGB
                        let mut albedo = [base[0], base[1], base[2], base[3]];
                        if has_uv && let Some(t) = tex {
                            let uv0 = m.uvs[ids[0]];
                            let uv1 = m.uvs[ids[1]];
                            let uv2 = m.uvs[ids[2]];
                            let u = pw0 * uv0[0] + pw1 * uv1[0] + pw2 * uv2[0];
                            let v = pw0 * uv0[1] + pw1 * uv1[1] + pw2 * uv2[1];
                            let s = crate::images::sample(t, u, v);
                            if alpha_test && s[3] < 0.4 {
                                continue;
                            }
                            albedo = [
                                albedo[0] * srgb_to_linear(s[0]),
                                albedo[1] * srgb_to_linear(s[1]),
                                albedo[2] * srgb_to_linear(s[2]),
                                albedo[3],
                            ];
                        }
                        let mut n = if has_n {
                            let n0 = Vec3::from(m.normals[ids[0]]);
                            let n1 = Vec3::from(m.normals[ids[1]]);
                            let n2 = Vec3::from(m.normals[ids[2]]);
                            (n0 * pw0 + n1 * pw1 + n2 * pw2).normalize_or_zero()
                        } else {
                            face_n
                        };
                        if n == Vec3::ZERO {
                            n = face_n;
                        }
                        // two-sided: face the camera
                        if n.dot(dir) < 0.0 {
                            n = -n;
                        }
                        let diffuse = n.dot(key).max(0.0);
                        let fill_d = n.dot(fill).max(0.0);
                        let rim = (1.0 - n.dot(dir).clamp(0.0, 1.0)).powi(3);
                        let light = 0.22 + 0.85 * diffuse + 0.25 * fill_d;
                        depth[di] = z;
                        let lin = [albedo[0], albedo[1], albedo[2]];
                        color[di] = [
                            linear_to_srgb(lin[0] * light + rim * 0.08),
                            linear_to_srgb(lin[1] * light + rim * 0.09),
                            linear_to_srgb(lin[2] * light + rim * 0.10),
                        ];
                    }
                }
            }
        }
    }

    // Downsample (box filter) for anti-aliasing.
    let mut out = RgbaImage::new(size, size);
    for y in 0..size {
        for x in 0..size {
            let mut acc = [0f32; 3];
            for dy in 0..ss {
                for dx in 0..ss {
                    let c = color[((y * ss + dy) * w + x * ss + dx) as usize];
                    acc[0] += c[0];
                    acc[1] += c[1];
                    acc[2] += c[2];
                }
            }
            let n = (ss * ss) as f32;
            out.put_pixel(
                x,
                y,
                Rgba([
                    (acc[0] / n * 255.0).clamp(0.0, 255.0) as u8,
                    (acc[1] / n * 255.0).clamp(0.0, 255.0) as u8,
                    (acc[2] / n * 255.0).clamp(0.0, 255.0) as u8,
                    255,
                ]),
            );
        }
    }
    out
}

fn edge(a: Vec3, b: Vec3, p: Vec3) -> f32 {
    (b.x - a.x) * (p.y - a.y) - (b.y - a.y) * (p.x - a.x)
}

/// Thumbnail for a plain image: letterboxed into a square on the background.
pub fn image_thumbnail(img: &RgbaImage, size: u32) -> RgbaImage {
    let (w, h) = (img.width().max(1), img.height().max(1));
    let scale = size as f32 / w.max(h) as f32;
    let nw = ((w as f32 * scale).round() as u32).clamp(1, size);
    let nh = ((h as f32 * scale).round() as u32).clamp(1, size);
    let resized = if nw <= w && nh <= h {
        image::imageops::thumbnail(img, nw, nh)
    } else {
        image::imageops::resize(img, nw, nh, image::imageops::FilterType::Triangle)
    };
    let mut out = RgbaImage::new(size, size);
    for y in 0..size {
        let bg = background(y, size);
        for x in 0..size {
            out.put_pixel(x, y, Rgba([(bg[0] * 255.0) as u8, (bg[1] * 255.0) as u8, (bg[2] * 255.0) as u8, 255]));
        }
    }
    let ox = (size - nw) / 2;
    let oy = (size - nh) / 2;
    for (x, y, p) in resized.enumerate_pixels() {
        let dst = out.get_pixel_mut(ox + x, oy + y);
        let a = p.0[3] as f32 / 255.0;
        for c in 0..3 {
            dst.0[c] = (p.0[c] as f32 * a + dst.0[c] as f32 * (1.0 - a)) as u8;
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn renders_something() {
        // A quad facing +Z.
        let m = MeshPrim {
            name: "q".into(),
            material: Some(0),
            positions: vec![[-1.0, -1.0, 0.0], [1.0, -1.0, 0.0], [1.0, 1.0, 0.0], [-1.0, 1.0, 0.0]],
            normals: vec![],
            uvs: vec![],
            indices: vec![0, 1, 2, 0, 2, 3],
            ..Default::default()
        };
        let mats = [ThumbMaterial { base_color: [1.0, 0.0, 0.0, 1.0], texture: None, alpha_test: false }];
        let img = render_thumbnail(&[m], &mats, 64);
        let c = img.get_pixel(32, 32).0;
        assert!(c[0] > 100 && c[1] < 60, "center pixel {c:?}");
    }
}
