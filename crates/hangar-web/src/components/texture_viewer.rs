//! Fullscreen texture viewer: fit / 1:1, wheel zoom around the cursor, drag to
//! pan, ←/→ between textures. The web preview (capped by the pipeline, 2048 px
//! by default) shows at once; when the zoom goes past its resolution the
//! original file is loaded on top if the browser can decode its format
//! (PNG / JPEG / WebP…; TGA and TIF stay on the preview).

use crate::api;
use crate::util::{event_key, fmt_bytes};
use hangar_core::{TextureInfo, TextureSetUse, blob_url, paths, texture_location};
use leptos::html;
use leptos::portal::Portal;
use leptos::prelude::*;
use uuid::Uuid;
use wasm_bindgen::JsCast;

const MIN_SCALE: f64 = 0.01;
/// One texel at most this many screen pixels.
const MAX_SCALE: f64 = 32.0;

fn browser_decodes(path: &str) -> bool {
    matches!(paths::extension(path).as_str(), "png" | "jpg" | "jpeg" | "webp" | "gif" | "bmp" | "avif")
}

/// Screen pixels per texel of the original, and the original's top-left corner
/// in viewport pixels.
#[derive(Clone, Copy, PartialEq, Debug)]
struct Placement {
    scale: f64,
    x: f64,
    y: f64,
}

#[component]
pub fn TextureViewer(
    textures: Vec<TextureInfo>,
    start: usize,
    version_id: Uuid,
    /// The manifest's texture sets: their images (`@slug/…`) are files of the set's version.
    #[prop(optional)]
    texture_sets: Vec<TextureSetUse>,
    on_close: Callback<()>,
) -> impl IntoView {
    let texture_sets = StoredValue::new(texture_sets);
    let raw_url = move |path: &str, download: bool| {
        let (version, path) = texture_sets.with_value(|s| texture_location(s, version_id, path));
        api::raw_file_url(version, &path, download)
    };
    let count = textures.len();
    let textures = StoredValue::new(textures);
    let index = RwSignal::new(start.min(count.saturating_sub(1)));
    let current = Memo::new(move |_| textures.with_value(|t| t.get(index.get()).cloned()));
    let place = RwSignal::new(None::<Placement>);
    // Fit follows the window size until the user zooms or pans.
    let fitted = RwSignal::new(true);
    // Natural size of the loaded preview.
    let preview_px = RwSignal::new(None::<(f64, f64)>);
    let want_original = RwSignal::new(false);
    let original_loaded = RwSignal::new(false);
    // Pointer id and last position of a drag.
    let drag = StoredValue::new(None::<(i32, f64, f64)>);
    let viewport = NodeRef::<html::Div>::new();

    // Size of the original in texels (the manifest's, else the preview's).
    let dims = move || {
        let t = current.get_untracked()?;
        if t.width > 0 && t.height > 0 {
            Some((t.width as f64, t.height as f64))
        } else {
            preview_px.get_untracked()
        }
    };
    let viewport_rect = move || viewport.get_untracked().map(|el| el.get_bounding_client_rect());

    let fit = move || {
        let (Some((w, h)), Some(r)) = (dims(), viewport_rect()) else { return };
        let scale = ((r.width() / w).min(r.height() / h) * 0.96).clamp(MIN_SCALE, MAX_SCALE);
        place.set(Some(Placement { scale, x: (r.width() - w * scale) / 2.0, y: (r.height() - h * scale) / 2.0 }));
        fitted.set(true);
    };
    // Zoom to `scale` keeping the texel under viewport point (px, py) in place.
    let zoom_to = move |scale: f64, px: f64, py: f64| {
        place.update(|p| {
            if let Some(p) = p {
                let s = scale.clamp(MIN_SCALE, MAX_SCALE);
                let k = s / p.scale;
                *p = Placement { scale: s, x: px - (px - p.x) * k, y: py - (py - p.y) * k };
            }
        });
        fitted.set(false);
    };
    let zoom_center = move |factor: f64| {
        let (Some(p), Some(r)) = (place.get_untracked(), viewport_rect()) else { return };
        zoom_to(p.scale * factor, r.width() / 2.0, r.height() / 2.0);
    };
    let go = move |delta: isize| {
        if count > 1 {
            index.update(|i| *i = (*i as isize + delta).rem_euclid(count as isize) as usize);
        }
    };

    // New texture: start from its preview, fitted.
    Effect::new(move |_| {
        index.track();
        preview_px.set(None);
        want_original.set(false);
        original_loaded.set(false);
        place.set(None);
        fit();
    });
    // Past the preview's resolution, load the original (once per texture).
    Effect::new(move |_| {
        if want_original.get() {
            return;
        }
        let (Some(p), Some(t), Some((pw, _))) = (place.get(), current.get(), preview_px.get()) else { return };
        let w = if t.width > 0 { t.width as f64 } else { pw };
        if browser_decodes(&t.path) && w > pw && p.scale * w / pw > 1.0 {
            want_original.set(true);
        }
    });

    let resize = window_event_listener(leptos::ev::resize, move |_| {
        if fitted.get_untracked() {
            fit();
        }
    });
    let keys = window_event_listener(leptos::ev::keydown, move |ev| {
        let handled = match event_key(&ev).as_deref() {
            Some("Escape") => {
                on_close.run(());
                true
            }
            Some("ArrowLeft") => {
                go(-1);
                true
            }
            Some("ArrowRight") => {
                go(1);
                true
            }
            Some("0") => {
                fit();
                true
            }
            Some("1") => {
                zoom_center(1.0 / place.get_untracked().map_or(1.0, |p| p.scale));
                true
            }
            Some("+" | "=") => {
                zoom_center(1.25);
                true
            }
            Some("-") => {
                zoom_center(0.8);
                true
            }
            _ => false,
        };
        if handled {
            ev.prevent_default();
        }
    });
    on_cleanup(move || {
        resize.remove();
        keys.remove();
    });

    let on_wheel = move |ev: web_sys::WheelEvent| {
        ev.prevent_default();
        let (Some(p), Some(r)) = (place.get_untracked(), viewport_rect()) else { return };
        let dy = match ev.delta_mode() {
            1 => ev.delta_y() * 16.0,
            2 => ev.delta_y() * r.height(),
            _ => ev.delta_y(),
        };
        let factor = (-dy * 0.0015).clamp(-1.0, 1.0).exp();
        zoom_to(p.scale * factor, ev.client_x() as f64 - r.left(), ev.client_y() as f64 - r.top());
    };
    let on_down = move |ev: web_sys::PointerEvent| {
        if ev.button() != 0 {
            return;
        }
        if let Some(el) = ev.current_target().and_then(|t| t.dyn_into::<web_sys::Element>().ok()) {
            let _ = el.set_pointer_capture(ev.pointer_id());
        }
        drag.set_value(Some((ev.pointer_id(), ev.client_x() as f64, ev.client_y() as f64)));
    };
    let on_move = move |ev: web_sys::PointerEvent| {
        let Some((id, lx, ly)) = drag.get_value() else { return };
        if id != ev.pointer_id() {
            return;
        }
        let (x, y) = (ev.client_x() as f64, ev.client_y() as f64);
        drag.set_value(Some((id, x, y)));
        place.update(|p| {
            if let Some(p) = p {
                p.x += x - lx;
                p.y += y - ly;
            }
        });
        fitted.set(false);
    };
    let on_up = move |ev: web_sys::PointerEvent| {
        if drag.get_value().is_some_and(|(id, ..)| id == ev.pointer_id()) {
            drag.set_value(None);
        }
    };
    // Double click: fit ⇄ 1:1 at the cursor.
    let on_dblclick = move |ev: web_sys::MouseEvent| {
        let (Some(p), Some(r)) = (place.get_untracked(), viewport_rect()) else { return };
        if fitted.get_untracked() || p.scale < 0.99 {
            zoom_to(1.0, ev.client_x() as f64 - r.left(), ev.client_y() as f64 - r.top());
        } else {
            fit();
        }
    };

    // Box of the image in the viewport: both sources are stretched to it.
    let image_style = move || {
        let (Some(p), Some(t)) = (place.get(), current.get()) else { return "display: none".to_string() };
        let (w, h) = if t.width > 0 && t.height > 0 {
            (t.width as f64, t.height as f64)
        } else {
            preview_px.get().unwrap_or((0.0, 0.0))
        };
        let (sw, sh) = (w * p.scale, h * p.scale);
        format!("transform: translate({:.2}px, {:.2}px); width: {sw:.2}px; height: {sh:.2}px", p.x, p.y)
    };
    // Show texels as squares once each covers 2+ screen pixels.
    let pixelated = move || {
        let (Some(p), Some(t)) = (place.get(), current.get()) else { return false };
        let texel = match (original_loaded.get(), preview_px.get()) {
            (false, Some((pw, _))) if t.width > 0 => p.scale * t.width as f64 / pw,
            _ => p.scale,
        };
        texel >= 2.0
    };
    let source_label = move || {
        let t = current.get()?;
        let (pw, _) = preview_px.get()?;
        let full = t.width == 0 || pw >= t.width as f64;
        Some(if original_loaded.get() || full {
            "resolução total".to_string()
        } else if want_original.get() {
            "carregando original…".to_string()
        } else if browser_decodes(&t.path) {
            format!("prévia {pw:.0} px · o original carrega no zoom")
        } else {
            format!("prévia {pw:.0} px · {} não abre no navegador", t.format.to_uppercase())
        })
    };

    view! {
        // In <body>: an ancestor with its own stacking context (the side panel)
        // would otherwise cap the overlay under the page header.
        <Portal>
            <div class="texview" role="dialog" aria-modal="true" aria-label="Visualizar textura">
                <header class="texview-bar">
                    {move || current.get().map(|t| view! {
                        <div class="texview-info">
                            <b class="mono clamp-1" title=t.path.clone()>{paths::file_name(&t.path).to_string()}</b>
                            <span class="muted mono">
                                {format!("{}×{} · {} · {}", t.width, t.height, t.format, fmt_bytes(t.size as i64))}
                            </span>
                            {t.slot.map(|s| view! { <span class="pill">{s.label()}</span> })}
                            <span class="muted texview-source">{source_label}</span>
                        </div>
                    })}
                    <div class="texview-tools">
                        {(count > 1).then(|| view! {
                            <span class="muted mono">{move || format!("{} / {count}", index.get() + 1)}</span>
                        })}
                        <button
                            class="btn btn-ghost btn-sm"
                            class:active=move || fitted.get()
                            on:click=move |_| fit()
                            title="Ajustar à tela (0)"
                        >
                            "Ajustar"
                        </button>
                        <button
                            class="btn btn-ghost btn-sm"
                            on:click=move |_| zoom_center(1.0 / place.get_untracked().map_or(1.0, |p| p.scale))
                            title="Um texel por pixel (1)"
                        >
                            "1:1"
                        </button>
                        <button
                            class="icon-btn"
                            on:click=move |_| zoom_center(0.8)
                            title="Diminuir (−)"
                            aria-label="Diminuir"
                        >
                            "−"
                        </button>
                        <span class="mono texview-zoom">
                            {move || place.get().map(|p| format!("{:.0}%", p.scale * 100.0))}
                        </span>
                        <button
                            class="icon-btn"
                            on:click=move |_| zoom_center(1.25)
                            title="Aumentar (+)"
                            aria-label="Aumentar"
                        >
                            "+"
                        </button>
                        {move || current.get().map(|t| view! {
                            <a
                                class="btn btn-ghost btn-sm"
                                href=raw_url(&t.path, true)
                                title="Baixar o arquivo original"
                            >
                                "Baixar"
                            </a>
                        })}
                        <button
                            class="icon-btn"
                            on:click=move |_| on_close.run(())
                            title="Fechar (Esc)"
                            aria-label="Fechar"
                        >
                            "✕"
                        </button>
                    </div>
                </header>
                <div
                    class="texview-stage checker"
                    node_ref=viewport
                    on:wheel=on_wheel
                    on:pointerdown=on_down
                    on:pointermove=on_move
                    on:pointerup=on_up
                    on:pointercancel=on_up
                    on:dblclick=on_dblclick
                >
                    {move || current.get().map(|t| {
                        let preview = t.preview_hash.as_deref().map(blob_url).unwrap_or_default();
                        let original = raw_url(&t.path, false);
                        view! {
                            <img
                                class="texview-img"
                                class:pixelated=pixelated
                                src=preview
                                alt=t.path.clone()
                                draggable="false"
                                style=image_style
                                on:load=move |ev| {
                                    let img = ev.target().and_then(|t| t.dyn_into::<web_sys::HtmlImageElement>().ok());
                                    if let Some(img) = img {
                                        preview_px.set(Some((img.natural_width() as f64, img.natural_height() as f64)));
                                        if place.get_untracked().is_none() {
                                            fit();
                                        }
                                    }
                                }
                            />
                            <Show when=move || want_original.get()>
                                <img
                                    class="texview-img"
                                    class:pixelated=pixelated
                                    class:pending=move || !original_loaded.get()
                                    src=original.clone()
                                    alt=""
                                    draggable="false"
                                    style=image_style
                                    on:load=move |_| original_loaded.set(true)
                                />
                            </Show>
                        }
                    })}
                    {(count > 1).then(|| view! {
                        <button
                            class="texview-nav prev"
                            on:click=move |_| go(-1)
                            on:pointerdown=|ev| ev.stop_propagation()
                            aria-label="Anterior"
                        >
                            "‹"
                        </button>
                        <button
                            class="texview-nav next"
                            on:click=move |_| go(1)
                            on:pointerdown=|ev| ev.stop_propagation()
                            aria-label="Próxima"
                        >
                            "›"
                        </button>
                    })}
                </div>
                <footer class="texview-hint muted">
                    "Roda do mouse: zoom · arrastar: mover · duplo clique: 1:1 / ajustar · ← →: outra textura · \
                     Esc: fechar"
                </footer>
            </div>
        </Portal>
    }
}
