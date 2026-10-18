//! Main preview area of the asset page: 3D viewer, texture gallery, audio
//! player, or the processing status of the selected version.

use crate::api;
use crate::components::viewer_panel::ViewerPanel;
use crate::util::fmt_bytes;
use hangar_core::{PreviewKind, VersionDto, VersionStatus, blob_url};
use leptos::prelude::*;
use uuid::Uuid;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Mode {
    Empty,
    Processing,
    Failed,
    Model,
    Image,
    Audio,
    NoPreview,
}

fn mode_of(v: &Option<VersionDto>) -> Mode {
    let Some(v) = v else { return Mode::Empty };
    match v.status {
        VersionStatus::Pending | VersionStatus::Processing => Mode::Processing,
        VersionStatus::Failed => Mode::Failed,
        VersionStatus::Ready => match v.preview.as_ref().and_then(|p| p.kind) {
            Some(PreviewKind::Model) if v.preview.as_ref().is_some_and(|p| !p.lods.is_empty()) => Mode::Model,
            Some(PreviewKind::Image) => Mode::Image,
            Some(PreviewKind::Audio) => Mode::Audio,
            _ => Mode::NoPreview,
        },
    }
}

#[component]
pub fn Stage(
    version: Signal<Option<VersionDto>>,
    asset_id: Signal<Uuid>,
    can_write: Signal<bool>,
    on_reprocess: Callback<Uuid>,
    on_upload: Callback<()>,
) -> impl IntoView {
    // Only re-render the stage when the *kind* of preview changes, so the Bevy
    // canvas survives switching between versions / LODs of the same asset.
    let mode = Memo::new(move |_| mode_of(&version.get()));

    move || match mode.get() {
        Mode::Model => view! { <ViewerPanel version=version asset_id=asset_id /> }.into_any(),
        Mode::Image => view! { <TextureGallery version=version /> }.into_any(),
        Mode::Audio => view! { <AudioPlayer version=version /> }.into_any(),
        Mode::Processing => view! {
            <div class="stage-placeholder">
                <div class="radar"></div>
                <h3>{move || version.get().map(|v| format!("Processando v{}…", v.number))}</h3>
                <p class="muted">"Convertendo, gerando LODs, variantes e thumbnail. A página atualiza sozinha."</p>
            </div>
        }
        .into_any(),
        Mode::Failed => view! {
            <div class="stage-placeholder failed">
                <div class="empty-code">"!"</div>
                <h3>"O processamento falhou"</h3>
                <pre class="error-pre">{move || version.get().and_then(|v| v.error).unwrap_or_default()}</pre>
                <Show when=move || can_write.get()>
                    <button
                        class="btn"
                        on:click=move |_| {
                            if let Some(v) = version.get_untracked() {
                                on_reprocess.run(v.id)
                            }
                        }
                    >
                        "Reprocessar"
                    </button>
                </Show>
            </div>
        }
        .into_any(),
        Mode::NoPreview => view! {
            <div class="stage-placeholder">
                <div class="empty-code">"◇"</div>
                <h3>"Sem pré-visualização"</h3>
                <p class="muted">
                    {move || {
                        version
                            .get()
                            .map(|v| {
                                format!(
                                    "{} arquivos · {} — nenhum modelo, imagem ou áudio reconhecido.",
                                    v.file_count,
                                    fmt_bytes(v.total_size),
                                )
                            })
                    }}
                </p>
            </div>
        }
        .into_any(),
        Mode::Empty => view! {
            <div class="stage-placeholder dropzone-hint">
                <div class="hazard"></div>
                <h3>"Nenhuma versão ainda"</h3>
                <p class="muted">"Envie os arquivos comprados (FBX, glTF, OBJ, texturas…) para criar a v1."</p>
                <Show when=move || can_write.get()>
                    <button class="btn btn-primary" on:click=move |_| on_upload.run(())>
                        "Enviar arquivos"
                    </button>
                </Show>
            </div>
        }
        .into_any(),
    }
}

#[component]
fn TextureGallery(version: Signal<Option<VersionDto>>) -> impl IntoView {
    let selected = RwSignal::new(0usize);
    let textures = Memo::new(move |_| version.get().and_then(|v| v.preview).map(|p| p.textures).unwrap_or_default());
    view! {
        <div class="gallery">
            <div class="gallery-main checker">
                {move || {
                    let list = textures.get();
                    let t = list.get(selected.get()).or(list.first()).cloned();
                    t.map(|t| {
                        let src = t.preview_hash.as_deref().map(blob_url).unwrap_or_default();
                        view! {
                            <img src=src alt=t.path.clone() />
                            <div class="gallery-caption mono">
                                {t.path.clone()} " · " {format!("{}×{}", t.width, t.height)} " · "
                                {t.format.clone()} " · " {fmt_bytes(t.size as i64)}
                                {t.slot.map(|s| view! { <span class="pill">{s.label()}</span> })}
                            </div>
                        }
                    })
                }}
            </div>
            <div class="gallery-strip">
                {move || {
                    textures
                        .get()
                        .into_iter()
                        .enumerate()
                        .map(|(i, t)| {
                            let src = t.preview_hash.as_deref().map(blob_url).unwrap_or_default();
                            view! {
                                <button
                                    class="gallery-thumb checker"
                                    class:active=move || selected.get() == i
                                    title=t.path.clone()
                                    on:click=move |_| selected.set(i)
                                >
                                    <img src=src alt="" loading="lazy" />
                                </button>
                            }
                        })
                        .collect_view()
                }}
            </div>
        </div>
    }
}

#[component]
fn AudioPlayer(version: Signal<Option<VersionDto>>) -> impl IntoView {
    let info = Memo::new(move |_| version.get().and_then(|v| v.preview).and_then(|p| p.audio));
    view! {
        <div class="audio-stage">
            {move || {
                let v = version.get();
                info.get()
                    .map(|a| {
                        let src = v.as_ref().map(|v| api::raw_file_url(v.id, &a.path, false)).unwrap_or_default();
                        let peaks = a.peaks.clone();
                        let n = peaks.len().max(1) as f32;
                        let bars = peaks
                            .iter()
                            .enumerate()
                            .map(|(i, p)| {
                                let h = (p.clamp(0.0, 1.0) * 90.0).max(1.0);
                                view! {
                                    <rect
                                        x=format!("{:.3}", i as f32 / n * 1000.0)
                                        y=format!("{:.2}", 50.0 - h / 2.0)
                                        width=format!("{:.3}", (1000.0 / n * 0.7).max(0.5))
                                        height=format!("{h:.2}")
                                        rx="1"
                                    />
                                }
                            })
                            .collect_view();
                        view! {
                            <svg class="waveform" viewBox="0 0 1000 100" preserveAspectRatio="none" aria-hidden="true">
                                {bars}
                            </svg>
                            <audio controls=true preload="metadata" src=src></audio>
                            <div class="audio-meta mono">
                                <span>{a.path.clone()}</span>
                                <span>{a.format.to_uppercase()}</span>
                                {a.sample_rate.map(|r| view! { <span>{format!("{:.1} kHz", r as f32 / 1000.0)}</span> })}
                                {a.channels.map(|c| view! { <span>{if c == 1 { "mono".to_string() } else { format!("{c} canais") }}</span> })}
                                {a.duration_secs.map(|d| view! { <span>{format!("{}:{:04.1}", (d / 60.0) as u32, d % 60.0)}</span> })}
                            </div>
                        }
                    })
            }}
        </div>
    }
}
