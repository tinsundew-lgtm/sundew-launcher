//! Test-only software painter: turns the shapes of a frame into a PNG, so the
//! look of a page can be checked without a window or a GPU.

use std::collections::HashMap;

use eframe::egui::{self, Color32, ColorImage, TextureId};
use egui::epaint::{ImageData, Primitive, textures::TexturesDelta};

#[derive(Default)]
pub struct Canvas {
    textures: HashMap<TextureId, ColorImage>,
}

impl Canvas {
    /// Take in the texture uploads of a frame (and empty the delta).
    pub fn apply(&mut self, delta: &mut TexturesDelta) {
        for (id, list) in delta.set.drain() {
            for d in list {
                let ImageData::Color(img) = &d.image;
                match d.pos {
                    None => {
                        self.textures.insert(id, (**img).clone());
                    }
                    Some([x0, y0]) => {
                        let Some(tex) = self.textures.get_mut(&id) else { continue };
                        let [w, h] = img.size;
                        for y in 0..h {
                            for x in 0..w {
                                let (tx, ty) = (x0 + x, y0 + y);
                                if tx < tex.size[0] && ty < tex.size[1] {
                                    tex.pixels[ty * tex.size[0] + tx] = img.pixels[y * w + x];
                                }
                            }
                        }
                    }
                }
            }
        }
        for id in delta.free.drain() {
            self.textures.remove(&id);
        }
    }

    /// Paint a frame's output into a PNG file.
    pub fn save(&self, ctx: &egui::Context, out: egui::FullOutput, size: egui::Vec2, path: &std::path::Path) {
        let ppp = out.pixels_per_point;
        let (w, h) = ((size.x * ppp) as usize, (size.y * ppp) as usize);
        let mut px = vec![[0.0f32; 4]; w * h];
        for clipped in ctx.tessellate(out.shapes, ppp) {
            let Primitive::Mesh(mesh) = clipped.primitive else { continue };
            let tex = self.textures.get(&mesh.texture_id);
            let clip = clipped.clip_rect;
            let (cx0, cy0) = ((clip.left() * ppp).max(0.0) as usize, (clip.top() * ppp).max(0.0) as usize);
            let (cx1, cy1) = (((clip.right() * ppp).ceil() as usize).min(w), ((clip.bottom() * ppp).ceil() as usize).min(h));
            for tri in mesh.indices.chunks_exact(3) {
                let v = [mesh.vertices[tri[0] as usize], mesh.vertices[tri[1] as usize], mesh.vertices[tri[2] as usize]];
                let p: Vec<(f32, f32)> = v.iter().map(|v| (v.pos.x * ppp, v.pos.y * ppp)).collect();
                let area = (p[1].0 - p[0].0) * (p[2].1 - p[0].1) - (p[2].0 - p[0].0) * (p[1].1 - p[0].1);
                if area.abs() < 1e-6 {
                    continue;
                }
                let minx = (p.iter().map(|q| q.0).fold(f32::MAX, f32::min).floor().max(cx0 as f32)) as usize;
                let maxx = (p.iter().map(|q| q.0).fold(f32::MIN, f32::max).ceil().min(cx1 as f32)) as usize;
                let miny = (p.iter().map(|q| q.1).fold(f32::MAX, f32::min).floor().max(cy0 as f32)) as usize;
                let maxy = (p.iter().map(|q| q.1).fold(f32::MIN, f32::max).ceil().min(cy1 as f32)) as usize;
                let col: Vec<[f32; 4]> = v.iter().map(|v| rgba(v.color)).collect();
                for y in miny..maxy {
                    for x in minx..maxx {
                        let (fx, fy) = (x as f32 + 0.5, y as f32 + 0.5);
                        let e = |a: (f32, f32), b: (f32, f32)| (b.0 - a.0) * (fy - a.1) - (fx - a.0) * (b.1 - a.1);
                        let w0 = e(p[1], p[2]) / area;
                        let w1 = e(p[2], p[0]) / area;
                        let w2 = 1.0 - w0 - w1;
                        if w0 < -1e-4 || w1 < -1e-4 || w2 < -1e-4 {
                            continue;
                        }
                        let mut src = [0.0f32; 4];
                        for k in 0..4 {
                            src[k] = col[0][k] * w0 + col[1][k] * w1 + col[2][k] * w2;
                        }
                        if let Some(t) = tex {
                            let u = v[0].uv.x * w0 + v[1].uv.x * w1 + v[2].uv.x * w2;
                            let vv = v[0].uv.y * w0 + v[1].uv.y * w1 + v[2].uv.y * w2;
                            let s = sample(t, u, vv);
                            for k in 0..4 {
                                src[k] *= s[k];
                            }
                        }
                        let d = &mut px[y * w + x];
                        for k in 0..4 {
                            d[k] = src[k] + d[k] * (1.0 - src[3]);
                        }
                    }
                }
            }
        }
        let bytes: Vec<u8> = px.iter().flat_map(|c| [to8(c[0]), to8(c[1]), to8(c[2]), 255]).collect();
        image::save_buffer(path, &bytes, w as u32, h as u32, image::ColorType::Rgba8).expect("save snapshot");
    }
}

fn rgba(c: Color32) -> [f32; 4] {
    [c.r() as f32 / 255.0, c.g() as f32 / 255.0, c.b() as f32 / 255.0, c.a() as f32 / 255.0]
}

fn to8(v: f32) -> u8 {
    (v.clamp(0.0, 1.0) * 255.0).round() as u8
}

/// Bilinear sample of a premultiplied texture.
fn sample(t: &ColorImage, u: f32, v: f32) -> [f32; 4] {
    let [w, h] = t.size;
    let x = (u * w as f32 - 0.5).clamp(0.0, (w - 1) as f32);
    let y = (v * h as f32 - 0.5).clamp(0.0, (h - 1) as f32);
    let (x0, y0) = (x.floor() as usize, y.floor() as usize);
    let (x1, y1) = ((x0 + 1).min(w - 1), (y0 + 1).min(h - 1));
    let (fx, fy) = (x - x0 as f32, y - y0 as f32);
    let at = |x: usize, y: usize| rgba(t.pixels[y * w + x]);
    let (a, b, c, d) = (at(x0, y0), at(x1, y0), at(x0, y1), at(x1, y1));
    let mut out = [0.0; 4];
    for k in 0..4 {
        out[k] = (a[k] * (1.0 - fx) + b[k] * fx) * (1.0 - fy) + (c[k] * (1.0 - fx) + d[k] * fx) * fy;
    }
    out
}
