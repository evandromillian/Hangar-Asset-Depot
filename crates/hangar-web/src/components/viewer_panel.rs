//! 3D viewer panel: hosts the Bevy canvas and the LOD / variant / display toolbar.

use crate::api;
use crate::util::{fmt_bytes, fmt_compact, fmt_int};
use crate::viewer::{self, CANVAS_ID, ClipInfo, VariantTexture, ViewerCommand, ViewerDraft, ViewerEvent};
use crate::components::animation::by_folder;
use hangar_core::{AnimationInfo, LodSource, PreviewKind, VersionDto};
use leptos::prelude::*;
use leptos::task::spawn_local;
use leptos_bevy_canvas::prelude::*;
use std::collections::VecDeque;
use std::sync::Arc;
use uuid::Uuid;

/// Small LRU of downloaded blobs (GLBs and variant textures).
#[derive(Default)]
struct BlobCache {
    entries: VecDeque<(String, Arc<Vec<u8>>)>,
    bytes: usize,
}

impl BlobCache {
    const MAX_BYTES: usize = 192 * 1024 * 1024;

    fn get(&mut self, hash: &str) -> Option<Arc<Vec<u8>>> {
        let pos = self.entries.iter().position(|(h, _)| h == hash)?;
        let entry = self.entries.remove(pos)?;
        let data = entry.1.clone();
        self.entries.push_back(entry);
        Some(data)
    }

    fn put(&mut self, hash: String, data: Arc<Vec<u8>>) {
        self.bytes += data.len();
        self.entries.push_back((hash, data));
        while self.bytes > Self::MAX_BYTES && self.entries.len() > 1 {
            if let Some((_, d)) = self.entries.pop_front() {
                self.bytes -= d.len();
            }
        }
    }
}

async fn fetch_blob(cache: StoredValue<BlobCache>, hash: String) -> Result<Arc<Vec<u8>>, api::ApiError> {
    if let Some(d) = cache.try_update_value(|c| c.get(&hash)).flatten() {
        return Ok(d);
    }
    let bytes = Arc::new(api::blob_bytes(&hash).await?);
    cache.update_value(|c| c.put(hash, bytes.clone()));
    Ok(bytes)
}

fn sniff_ext(bytes: &[u8]) -> &'static str {
    match bytes {
        [0x89, b'P', b'N', b'G', ..] => "png",
        [0xFF, 0xD8, ..] => "jpg",
        [b'R', b'I', b'F', b'F', _, _, _, _, b'W', b'E', b'B', b'P', ..] => "webp",
        _ => "png",
    }
}

#[component]
pub fn ViewerPanel(
    /// Selected (ready) version.
    version: Signal<Option<VersionDto>>,
    asset_id: Signal<Uuid>,
    /// Unsaved state of the materials panel; while set, it replaces the
    /// selected skin in the viewer.
    #[prop(into)]
    draft: Signal<Option<ViewerDraft>>,
) -> impl IntoView {
    let (cmd_tx, cmd_rx) = message_l2b::<ViewerCommand>();
    let (evt_rx, evt_tx) = message_b2l::<ViewerEvent>();

    let cache = StoredValue::new(BlobCache::default());
    let lod = RwSignal::new(0u32);
    let variant = RwSignal::new(None::<String>);
    let wireframe = RwSignal::new(false);
    let auto_rotate = RwSignal::new(false);
    let grid = RwSignal::new(true);
    let loading = RwSignal::new(false);
    let error = RwSignal::new(None::<String>);
    let loaded_tris = RwSignal::new(None::<usize>);
    let request_seq = StoredValue::new(0u64);
    let variant_seq = StoredValue::new(0u64);
    // Animation playback (clip index into `preview.animations`).
    let anim_sel = RwSignal::new(None::<usize>);
    let anim_paused = RwSignal::new(false);
    let anim_speed = RwSignal::new(1.0f32);
    let anim_time = RwSignal::new(0.0f32);

    // Animation-only versions: the reference skeletons they were built on; the
    // selected one is shown as if it were the model.
    let raw = Memo::new(move |_| version.get().and_then(|v| v.preview));
    let rigs = Memo::new(move |_| {
        raw.get()
            .filter(|p| p.kind == Some(PreviewKind::Animation))
            .map(|p| p.skeletons.into_iter().filter(|s| s.glb_hash.is_some()).collect::<Vec<_>>())
            .unwrap_or_default()
    });
    let skeleton = RwSignal::new(None::<String>);
    Effect::new(move |_| {
        let list = rigs.get();
        if !skeleton.get_untracked().is_some_and(|s| list.iter().any(|r| r.skeleton == s)) {
            skeleton.set(list.first().map(|r| r.skeleton.clone()));
        }
    });
    let preview = Memo::new(move |_| {
        let p = raw.get()?;
        if p.kind != Some(PreviewKind::Animation) {
            return Some(p);
        }
        p.on_skeleton(&skeleton.get()?)
    });

    // Keep the LOD level / variant name when switching versions if still valid.
    Effect::new(move |_| {
        let Some(p) = preview.get() else { return };
        if !p.lods.iter().any(|l| l.level == lod.get_untracked()) {
            lod.set(p.lods.first().map(|l| l.level).unwrap_or(0));
        }
        let current = variant.get_untracked();
        if let Some(name) = current {
            if !p.variants.iter().any(|v| v.name == name) {
                variant.set(None);
            }
        }
    });

    // New version: keep the selected clip if it still exists, else play the first.
    Effect::new(move |_| {
        let Some(p) = preview.get() else { return };
        let n = p.animations.len();
        let current = anim_sel.get_untracked();
        if n == 0 {
            anim_sel.set(None);
        } else if current.is_none_or(|i| i >= n) {
            anim_sel.set(Some(0));
        }
    });

    // Load the GLB of the selected LOD.
    let tx = cmd_tx.clone();
    Effect::new(move |_| {
        let cmd_tx = tx.clone();
        let Some(p) = preview.get() else { return };
        let level = lod.get();
        let Some(info) = p.lods.iter().find(|l| l.level == level).or(p.lods.first()).cloned() else {
            cmd_tx.send(ViewerCommand::Clear).ok();
            return;
        };
        let asset = asset_id.get_untracked().to_string();
        let animations: Vec<ClipInfo> =
            p.animations.iter().map(|a| ClipInfo { duration: a.duration_secs, fps: a.fps }).collect();
        let seq = request_seq.get_value() + 1;
        request_seq.set_value(seq);
        loading.set(true);
        error.set(None);
        spawn_local(async move {
            match fetch_blob(cache, info.glb_hash.clone()).await {
                Ok(bytes) => {
                    if request_seq.get_value() == seq {
                        cmd_tx.send(ViewerCommand::LoadModel { key: info.glb_hash, asset, bytes, animations }).ok();
                    }
                }
                Err(e) => {
                    if request_seq.get_value() == seq {
                        loading.set(false);
                        error.set(Some(e.to_string()));
                    }
                }
            }
        });
    });

    // Apply / reset the texture variant, or the unsaved draft of the materials panel.
    let tx = cmd_tx.clone();
    Effect::new(move |_| {
        let cmd_tx = tx.clone();
        let Some(p) = preview.get() else { return };
        let wanted = match draft.get() {
            Some(d) => Some(d),
            None => variant
                .get()
                .and_then(|n| p.variants.into_iter().find(|v| v.name == n))
                .filter(|v| !v.is_default && !v.overrides.is_empty())
                .map(|v| ViewerDraft { overrides: v.overrides, hidden: vec![] }),
        };
        // Downloads finish out of order; only the latest request may apply.
        let seq = variant_seq.get_value() + 1;
        variant_seq.set_value(seq);
        match wanted.filter(|d| !d.overrides.is_empty() || !d.hidden.is_empty()) {
            Some(ViewerDraft { overrides, hidden }) => {
                spawn_local(async move {
                    let mut textures = Vec::new();
                    for o in overrides {
                        match fetch_blob(cache, o.preview_hash.clone()).await {
                            Ok(bytes) => textures.push(VariantTexture {
                                material: o.material,
                                slot: o.slot,
                                ext: sniff_ext(&bytes).to_string(),
                                bytes,
                            }),
                            Err(e) => error.set(Some(format!("Textura da variante: {e}"))),
                        }
                    }
                    if variant_seq.get_value() == seq {
                        cmd_tx.send(ViewerCommand::ApplyVariant { textures, hidden }).ok();
                    }
                });
            }
            None => {
                cmd_tx.send(ViewerCommand::ResetVariant).ok();
            }
        }
    });

    let tx = cmd_tx.clone();
    Effect::new(move |_| {
        tx.send(ViewerCommand::SetWireframe(wireframe.get())).ok();
    });
    let tx = cmd_tx.clone();
    Effect::new(move |_| {
        tx.send(ViewerCommand::SetAutoRotate(auto_rotate.get())).ok();
    });
    let tx = cmd_tx.clone();
    Effect::new(move |_| {
        tx.send(ViewerCommand::SetGrid(grid.get())).ok();
    });
    let tx = cmd_tx.clone();
    Effect::new(move |_| {
        anim_time.set(0.0);
        tx.send(ViewerCommand::SelectAnimation(anim_sel.get())).ok();
    });
    let tx = cmd_tx.clone();
    Effect::new(move |_| {
        tx.send(ViewerCommand::SetAnimationPaused(anim_paused.get())).ok();
    });
    let tx = cmd_tx.clone();
    Effect::new(move |_| {
        tx.send(ViewerCommand::SetAnimationSpeed(anim_speed.get())).ok();
    });

    Effect::new(move |_| match evt_rx.get() {
        Some(ViewerEvent::Loaded { triangles, .. }) => {
            loading.set(false);
            loaded_tris.set(Some(triangles));
        }
        Some(ViewerEvent::Error(e)) => {
            loading.set(false);
            error.set(Some(e));
        }
        Some(ViewerEvent::AnimationTime { clip, seconds }) if anim_sel.get_untracked() == Some(clip) => {
            anim_time.set(seconds);
        }
        Some(ViewerEvent::AnimationTime { .. }) | None => {}
    });

    let clips = Memo::new(move |_| preview.get().map(|p| p.animations).unwrap_or_default());
    let current_clip = move || anim_sel.get().and_then(|i| clips.with(|c| c.get(i).cloned()));
    let cycle = move |delta: isize| {
        let n = clips.with(|c| c.len()) as isize;
        if n > 0 {
            let next = anim_sel.get_untracked().map_or(0, |i| (i as isize + delta).rem_euclid(n));
            anim_sel.set(Some(next as usize));
        }
    };
    let tx = cmd_tx.clone();
    let step = Callback::new(move |frames: i32| {
        anim_paused.set(true);
        tx.send(ViewerCommand::StepAnimation(frames)).ok();
    });

    let current_lod = move || {
        preview.get().and_then(|p| p.lods.iter().find(|l| l.level == lod.get()).cloned())
    };

    view! {
        <div class="viewer">
            <div class="viewer-canvas" on:contextmenu=|ev| ev.prevent_default()>
                <BevyCanvas
                    init=move || viewer::build_app(cmd_rx, evt_tx)
                    canvas_id=CANVAS_ID
                />
                <Show when=move || loading.get()>
                    <div class="viewer-overlay">
                        <span class="spinner"></span>
                        <span>"Carregando malha…"</span>
                    </div>
                </Show>
                {move || error.get().map(|e| view! { <div class="viewer-error">{e}</div> })}
                <div class="viewer-hint mono">"LMB orbita · RMB/MMB move · roda zoom · F enquadra"</div>
            </div>
            <Show when=move || clips.with(|c| !c.is_empty())>
                <div class="viewer-toolbar anim-bar">
                    <div class="tool-group">
                        <span class="tool-label">"Animação"</span>
                        <button class="toggle anim-btn" title="Animação anterior" on:click=move |_| cycle(-1)>"‹"</button>
                        <select
                            class="anim-select"
                            on:change=move |ev| anim_sel.set(event_target_value(&ev).parse::<usize>().ok())
                        >
                            <option value="" selected=move || anim_sel.get().is_none()>"Pose de repouso"</option>
                            {move || {
                                let option = move |(i, a): (usize, AnimationInfo)| {
                                    view! {
                                        <option
                                            value=i.to_string()
                                            title=a.source.clone().unwrap_or_default()
                                            selected=move || anim_sel.get() == Some(i)
                                        >
                                            {format!("{} ({:.1} s)", a.name, a.duration_secs)}
                                        </option>
                                    }
                                };
                                // Packages spread over folders: one group per folder.
                                let groups = by_folder(&clips.get());
                                if groups.len() > 1 {
                                    groups
                                        .into_iter()
                                        .map(|(folder, list)| {
                                            let label = if folder.is_empty() { "/".to_string() } else { folder };
                                            view! { <optgroup label=label>{list.into_iter().map(option).collect_view()}</optgroup> }
                                        })
                                        .collect_view()
                                        .into_any()
                                } else {
                                    groups.into_iter().flat_map(|(_, list)| list).map(option).collect_view().into_any()
                                }
                            }}
                        </select>
                        <button class="toggle anim-btn" title="Próxima animação" on:click=move |_| cycle(1)>"›"</button>
                    </div>
                    <div class="tool-group">
                        <button
                            class="toggle anim-btn"
                            title="Quadro anterior"
                            disabled=move || anim_sel.get().is_none()
                            on:click=move |_| step.run(-1)
                        >
                            "|◀"
                        </button>
                        <button
                            class="toggle anim-btn anim-play"
                            class:on=move || !anim_paused.get() && anim_sel.get().is_some()
                            title=move || if anim_paused.get() { "Reproduzir" } else { "Pausar" }
                            disabled=move || anim_sel.get().is_none()
                            on:click=move |_| anim_paused.update(|p| *p = !*p)
                        >
                            {move || if anim_paused.get() { "▶" } else { "❚❚" }}
                        </button>
                        <button
                            class="toggle anim-btn"
                            title="Próximo quadro"
                            disabled=move || anim_sel.get().is_none()
                            on:click=move |_| step.run(1)
                        >
                            "▶|"
                        </button>
                    </div>
                    <label class="tool-group anim-rate" title="Velocidade de reprodução (negativa toca ao contrário)">
                        <span class="tool-label">"Velocidade"</span>
                        <input
                            type="number"
                            step="0.1"
                            min="-10"
                            max="10"
                            disabled=move || anim_sel.get().is_none()
                            prop:value=move || anim_speed.get().to_string()
                            on:change=move |ev| {
                                // Invalid input re-sets the old value, which rewrites the field.
                                match event_target_value(&ev).replace(',', ".").parse::<f32>() {
                                    Ok(v) if v.is_finite() => anim_speed.set(((v * 100.0).round() / 100.0).clamp(-10.0, 10.0)),
                                    _ => anim_speed.set(anim_speed.get_untracked()),
                                }
                            }
                        />
                        <span class="muted">"×"</span>
                    </label>
                    <span class="anim-time mono push-right">
                        {move || {
                            current_clip()
                                .map(|a| {
                                    let t = anim_time.get();
                                    let frame = ((t * a.fps).round() as u32 + 1).min(a.frames);
                                    format!("quadro {frame}/{} · {t:.2} / {:.2} s", a.frames, a.duration_secs)
                                })
                                .unwrap_or_else(|| "pose de repouso".to_string())
                        }}
                    </span>
                </div>
            </Show>
            <div class="viewer-toolbar">
                <Show when=move || rigs.with(|r| !r.is_empty())>
                    <div class="tool-group">
                        <span class="tool-label">"Esqueleto"</span>
                        {move || {
                            rigs.get()
                                .into_iter()
                                .map(|r| {
                                    let slug = r.skeleton.clone();
                                    let active = r.skeleton.clone();
                                    let tris = r.stats.as_ref().map_or(0, |s| s.triangles);
                                    view! {
                                        <button
                                            class="seg"
                                            class:active=move || skeleton.get().as_deref() == Some(active.as_str())
                                            title=format!(
                                                "{} de {} ossos encontrados · {} clipes · {}",
                                                r.bones_matched,
                                                r.bones_total,
                                                r.animations.len(),
                                                fmt_bytes(r.glb_size as i64),
                                            )
                                            on:click=move |_| skeleton.set(Some(slug.clone()))
                                        >
                                            <span class="seg-main">{r.name.clone()}</span>
                                            <span class="seg-sub">
                                                {r.version_number.map(|n| format!("v{n} · ")).unwrap_or_default()}
                                                {fmt_compact(tris)} "▲"
                                            </span>
                                        </button>
                                    }
                                })
                                .collect_view()
                        }}
                    </div>
                </Show>
                <div class="tool-group" class:hidden=move || rigs.with(|r| !r.is_empty())>
                    <span class="tool-label">"LOD"</span>
                    {move || {
                        preview
                            .get()
                            .map(|p| {
                                p.lods
                                    .into_iter()
                                    .map(|l| {
                                        let level = l.level;
                                        let src = match l.source {
                                            LodSource::Authored => "autoral",
                                            LodSource::Generated => "gerado",
                                        };
                                        view! {
                                            <button
                                                class="seg"
                                                class:active=move || lod.get() == level
                                                title=format!(
                                                    "{} triângulos · {} · {}",
                                                    fmt_int(l.triangles),
                                                    src,
                                                    fmt_bytes(l.glb_size as i64),
                                                )
                                                on:click=move |_| lod.set(level)
                                            >
                                                <span class="seg-main">{format!("LOD{level}")}</span>
                                                <span class="seg-sub">
                                                    {fmt_compact(l.triangles)} "▲"
                                                    <i class=format!("src src-{}", l.source.as_str())>{src}</i>
                                                </span>
                                            </button>
                                        }
                                    })
                                    .collect_view()
                            })
                    }}
                </div>
                <Show when=move || draft.with(|d| d.is_some())>
                    <div class="tool-group">
                        <span class="pill pill-accent" title="Alterações de materiais ainda não salvas">"pré-visualização"</span>
                    </div>
                </Show>
                <Show when=move || {
                    draft.with(|d| d.is_none()) && preview.get().map(|p| p.variants.len() > 1).unwrap_or(false)
                }>
                    <div class="tool-group">
                        <span class="tool-label">"Skin"</span>
                        {move || {
                            preview
                                .get()
                                .map(|p| {
                                    p.variants
                                        .into_iter()
                                        .map(|v| {
                                            let name = v.name.clone();
                                            let is_default = v.is_default;
                                            let name_for_active = name.clone();
                                            view! {
                                                <button
                                                    class="seg"
                                                    class:active=move || {
                                                        match variant.get() {
                                                            Some(n) => n == name_for_active,
                                                            None => is_default,
                                                        }
                                                    }
                                                    title=format!("detectado por: {}", v.detected_by)
                                                    on:click=move |_| {
                                                        variant.set(if is_default { None } else { Some(name.clone()) })
                                                    }
                                                >
                                                    <span class="seg-main">{v.name.clone()}</span>
                                                    <span class="seg-sub">
                                                        {if v.is_default {
                                                            "padrão".to_string()
                                                        } else {
                                                            format!("{} tex", v.overrides.len())
                                                        }}
                                                    </span>
                                                </button>
                                            }
                                        })
                                        .collect_view()
                                })
                        }}
                    </div>
                </Show>
                <div class="tool-group push-right">
                    <button class="toggle" class:on=move || wireframe.get() on:click=move |_| wireframe.update(|w| *w = !*w)>
                        "Wireframe"
                    </button>
                    <button class="toggle" class:on=move || auto_rotate.get() on:click=move |_| auto_rotate.update(|w| *w = !*w)>
                        "Girar"
                    </button>
                    <button class="toggle" class:on=move || grid.get() on:click=move |_| grid.update(|w| *w = !*w)>
                        "Grade"
                    </button>
                    <button class="toggle" on:click=move |_| { cmd_tx.send(ViewerCommand::ResetCamera).ok(); }>
                        "Enquadrar"
                    </button>
                </div>
            </div>
            <div class="stat-strip mono">
                {move || {
                    let p = preview.get();
                    let stats = p.as_ref().and_then(|p| p.stats.clone());
                    let l = current_lod();
                    let tris = l.as_ref().map(|l| l.triangles).or(loaded_tris.get().map(|t| t as u64)).unwrap_or(0);
                    let verts = l.as_ref().map(|l| l.vertices).unwrap_or(0);
                    let dims = stats.as_ref().map(|s| s.dimensions());
                    view! {
                        <span><b>{fmt_int(tris)}</b>" tris"</span>
                        <span><b>{fmt_int(verts)}</b>" verts"</span>
                        {stats
                            .as_ref()
                            .map(|s| {
                                view! {
                                    <span><b>{s.meshes}</b>" malhas"</span>
                                    <span><b>{s.materials}</b>" materiais"</span>
                                }
                            })}
                        {dims
                            .map(|d| {
                                view! {
                                    <span title="largura × altura × profundidade (m)">
                                        <b>{format!("{:.2} × {:.2} × {:.2}", d[0], d[1], d[2])}</b>
                                        " m"
                                    </span>
                                }
                            })}
                        {l.map(|l| view! { <span class="muted">{format!("GLB {}", fmt_bytes(l.glb_size as i64))}</span> })}
                    }
                }}
            </div>
        </div>
    }
}
