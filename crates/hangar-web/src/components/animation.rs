//! "Animação" tab: the clips of the selected version and, for animation-only
//! versions (an animation *package*: one or many FBX files, more can be added
//! in later versions), which reference skeletons they are previewed on. The
//! binding is saved as `.hangar/animation.json` in a new version (the pipeline
//! then builds one GLB per skeleton); added files inherit it.

use crate::api;
use crate::app::use_session;
use crate::components::common::ErrorBanner;
use hangar_core::{
    ANIMATION_CONFIG_PATH, AnimationConfig, AnimationInfo, CommitVersionRequest, PreviewKind, SkeletonDto,
    SkeletonPreview, VersionDto,
};
use leptos::prelude::*;
use leptos::task::spawn_local;
use leptos_router::components::A;
use uuid::Uuid;

/// Folder of the file a clip came from ("" for the root / the model file).
pub fn clip_folder(a: &AnimationInfo) -> &str {
    a.source.as_deref().and_then(|s| s.rsplit_once('/')).map_or("", |(dir, _)| dir)
}

/// Clips grouped by folder (in order of first appearance).
pub fn by_folder(clips: &[AnimationInfo]) -> Vec<(String, Vec<(usize, AnimationInfo)>)> {
    let mut groups: Vec<(String, Vec<(usize, AnimationInfo)>)> = Vec::new();
    for (i, a) in clips.iter().enumerate() {
        let folder = clip_folder(a);
        match groups.iter_mut().find(|(f, _)| f == folder) {
            Some((_, list)) => list.push((i, a.clone())),
            None => groups.push((folder.to_string(), vec![(i, a.clone())])),
        }
    }
    groups
}

fn clip_list(clips: Vec<AnimationInfo>) -> impl IntoView {
    let groups = by_folder(&clips);
    let titled = groups.len() > 1;
    groups
        .into_iter()
        .map(|(folder, list)| {
            view! {
                {titled.then(|| view! { <p class="clip-folder mono">{if folder.is_empty() { "/".to_string() } else { format!("{folder}/") }}</p> })}
                <ul class="clip-list">
                    {list
                        .into_iter()
                        .map(|(_, a)| {
                            let file = a.source.as_deref().map(|s| s.rsplit('/').next().unwrap_or(s).to_string());
                            view! {
                                <li class="clip-row">
                                    <span class="clip-name">{a.name.clone()}</span>
                                    <span class="muted mono">{format!("{:.2} s · {} quadros", a.duration_secs, a.frames)}</span>
                                    {file.map(|f| view! { <span class="clip-src muted mono" title=a.source.clone()>{f}</span> })}
                                </li>
                            }
                        })
                        .collect_view()}
                </ul>
            }
        })
        .collect_view()
}

fn file_count(clips: &[AnimationInfo]) -> usize {
    let mut files: Vec<&str> = clips.iter().filter_map(|a| a.source.as_deref()).collect();
    files.sort();
    files.dedup();
    files.len()
}

/// What the manifest says about one bound skeleton, for its row.
fn status_line(entry: Option<&SkeletonPreview>, skeleton: Option<&SkeletonDto>) -> (String, &'static str) {
    let Some(e) = entry else { return ("ainda não processado com este esqueleto".into(), "pending") };
    if let Some(err) = &e.error {
        return (err.clone(), "error");
    }
    let mut text = format!("{} de {} ossos · {} clipes", e.bones_matched, e.bones_total, e.animations.len());
    let head = skeleton.and_then(|s| s.head.as_ref());
    if let (Some(h), Some(n)) = (head, e.version_number)
        && e.version_id != Some(h.id)
    {
        text.push_str(&format!(" · feito com v{n}, atual v{}", h.number));
        return (text, "stale");
    }
    (text, "ok")
}

#[component]
pub fn AnimationPanel(
    version: Signal<Option<VersionDto>>,
    asset_id: Signal<Uuid>,
    can_write: Signal<bool>,
    on_reprocess: Callback<Uuid>,
    on_committed: Callback<VersionDto>,
    /// Opens the upload dialog to add files to the selected version.
    on_add: Callback<()>,
) -> impl IntoView {
    let session = use_session();
    let is_admin = move || session.user.get().is_some_and(|u| u.is_admin);
    let preview = Memo::new(move |_| version.get().and_then(|v| v.preview));
    let anim_only = Memo::new(move |_| preview.with(|p| p.as_ref().is_some_and(|p| p.kind == Some(PreviewKind::Animation))));
    let vid = Memo::new(move |_| version.with(|v| v.as_ref().map(|v| v.id)).filter(|_| anim_only.get()));

    let skeletons = LocalResource::new(api::skeletons);
    let registered = Memo::new(move |_| skeletons.get().and_then(|r| r.ok()).unwrap_or_default());

    // None while loading the version's binding file.
    let baseline = RwSignal::new(None::<Vec<String>>);
    let had_file = RwSignal::new(false);
    let edit = RwSignal::new(Vec::<String>::new());
    let message = RwSignal::new(String::new());
    let saving = RwSignal::new(false);
    let error = RwSignal::new(None::<String>);

    Effect::new(move |_| {
        let Some(v) = vid.get() else {
            baseline.set(None);
            return;
        };
        baseline.set(None);
        error.set(None);
        spawn_local(async move {
            let res = api::animation_config(v).await;
            if vid.get_untracked() != Some(v) {
                return;
            }
            let cfg = match res {
                Ok(c) => {
                    had_file.set(c.is_some());
                    c.unwrap_or_default()
                }
                Err(e) => {
                    error.set(Some(e.to_string()));
                    had_file.set(true);
                    AnimationConfig::default()
                }
            };
            edit.set(cfg.skeletons.clone());
            baseline.set(Some(cfg.skeletons));
        });
    });
    let dirty = Memo::new(move |_| baseline.with(|b| b.as_ref().is_some_and(|b| *b != edit.get())));

    let toggle = move |slug: String, on: bool| {
        edit.update(|list| {
            list.retain(|s| *s != slug);
            if on {
                list.push(slug);
            }
        });
    };

    let save = move |_| {
        let Some(v) = version.get_untracked() else { return };
        let cfg = AnimationConfig { skeletons: edit.get_untracked() };
        let msg = message.get_untracked().trim().to_string();
        let names: Vec<String> = cfg
            .skeletons
            .iter()
            .map(|slug| registered.with_untracked(|r| r.iter().find(|s| s.slug == *slug).map(|s| s.name.clone())).unwrap_or(slug.clone()))
            .collect();
        let msg = match (msg.is_empty(), names.is_empty()) {
            (false, _) => msg,
            (true, true) => "Animação: sem esqueleto de referência".to_string(),
            (true, false) => format!("Animação: pré-visualizar em {}", names.join(", ")),
        };
        let aid = asset_id.get_untracked();
        let remove_file = cfg.is_empty() && had_file.get_untracked();
        saving.set(true);
        error.set(None);
        spawn_local(async move {
            let res = async {
                let (upserts, deletes) = if remove_file {
                    (vec![], vec![ANIMATION_CONFIG_PATH.to_string()])
                } else {
                    (vec![api::upload_bytes(ANIMATION_CONFIG_PATH, &cfg.to_json()).await?], vec![])
                };
                let req = CommitVersionRequest {
                    parent_id: Some(v.id),
                    message: msg,
                    inherit: true,
                    upserts,
                    deletes,
                    set_head: v.is_head,
                };
                api::commit_version(aid, &req).await
            }
            .await;
            saving.set(false);
            match res {
                Ok(nv) => {
                    message.set(String::new());
                    on_committed.run(nv);
                }
                Err(e) => error.set(Some(e.to_string())),
            }
        });
    };

    // One row per registered skeleton, plus slugs of the file that no longer exist.
    let rows = move || {
        let p = preview.get()?;
        let list = registered.get();
        let mut slugs: Vec<(String, Option<SkeletonDto>)> = list.iter().map(|s| (s.slug.clone(), Some(s.clone()))).collect();
        for slug in edit.get() {
            if !slugs.iter().any(|(s, _)| *s == slug) {
                slugs.push((slug, None));
            }
        }
        let stale_any = slugs.iter().any(|(slug, sk)| {
            let entry = p.skeletons.iter().find(|e| e.skeleton == *slug);
            baseline.with(|b| b.as_ref().is_some_and(|b| b.contains(slug))) && status_line(entry, sk.as_ref()).1 == "stale"
        });
        let views = slugs
            .into_iter()
            .map(|(slug, sk)| {
                let entry = p.skeletons.iter().find(|e| e.skeleton == slug).cloned();
                let saved = baseline.with(|b| b.as_ref().is_some_and(|b| b.contains(&slug)));
                let (status, class) = if saved { status_line(entry.as_ref(), sk.as_ref()) } else { (String::new(), "") };
                let name = sk.as_ref().map(|s| s.name.clone()).or(entry.as_ref().map(|e| e.name.clone())).unwrap_or(slug.clone());
                let thumb = sk.as_ref().and_then(|s| s.thumbnail_url.clone());
                let slug_c = slug.clone();
                let slug_t = slug.clone();
                view! {
                    <li class="skel-row" class:missing=sk.is_none()>
                        <label class="skel-pick">
                            <input
                                type="checkbox"
                                disabled=move || !can_write.get() || saving.get()
                                prop:checked=move || edit.with(|l| l.contains(&slug_c))
                                on:change=move |ev| toggle(slug_t.clone(), event_target_checked(&ev))
                            />
                            <span class="skel-thumb checker">
                                {thumb.map(|t| view! { <img src=t alt="" loading="lazy" /> })}
                            </span>
                            <span class="skel-text">
                                <span class="skel-name">{name}</span>
                                <span class="mono muted">
                                    {slug.clone()}
                                    {sk.is_none().then_some(" · não cadastrado")}
                                </span>
                                {(!status.is_empty()).then(|| view! { <span class=format!("skel-status st-{class}")>{status}</span> })}
                            </span>
                        </label>
                    </li>
                }
            })
            .collect_view();
        Some(view! {
            <ul class="skel-list">{views}</ul>
            <Show when=move || stale_any && can_write.get() && !dirty.get()>
                <div class="mat-hidden-bar">
                    <span class="muted">"Um esqueleto mudou desde o processamento."</span>
                    <button
                        class="link-btn"
                        on:click=move |_| {
                            if let Some(v) = version.get_untracked() {
                                on_reprocess.run(v.id)
                            }
                        }
                    >
                        "Reprocessar"
                    </button>
                </div>
            </Show>
        })
    };

    view! {
        <div class="anim-panel">
            {move || {
                let p = preview.get();
                match p {
                    None => view! { <p class="muted">"Selecione uma versão processada."</p> }.into_any(),
                    Some(p) if p.kind != Some(PreviewKind::Animation) => {
                        if p.animations.is_empty() {
                            view! {
                                <p class="muted">"Esta versão não tem animações."</p>
                                <p class="muted hint">
                                    "Envie animações sem malha (só o esqueleto animado, ex.: exportadas do Unreal) para tocá-las em um esqueleto de referência."
                                </p>
                            }
                            .into_any()
                        } else {
                            view! {
                                <p class="muted hint">
                                    "As animações desta versão vieram junto com uma malha, então tocam só nela."
                                </p>
                                {clip_list(p.animations)}
                            }
                            .into_any()
                        }
                    }
                    Some(p) => view! {
                        <section class="detail-block">
                            <div class="anim-pkg-head">
                                <h3>"Pacote de animações"</h3>
                                <Show when=move || can_write.get()>
                                    <button class="btn btn-ghost btn-sm push-right" on:click=move |_| on_add.run(())>
                                        "+ Adicionar animações"
                                    </button>
                                </Show>
                            </div>
                            <p class="muted hint">
                                {format!(
                                    "{} clipe(s) em {} arquivo(s). Novas animações entram numa versão nova do mesmo pacote e herdam os esqueletos escolhidos.",
                                    p.animations.len(),
                                    file_count(&p.animations),
                                )}
                            </p>
                        </section>
                        <section class="detail-block">
                            <h3>"Esqueletos de referência"</h3>
                            <p class="muted hint">
                                "O pacote não tem malha própria. Escolha em quais esqueletos ele é pré-visualizado; os ossos são associados pelo nome."
                            </p>
                            {move || error.get().map(|e| view! { <ErrorBanner error=e /> })}
                            {move || match (baseline.get().is_some(), skeletons.get()) {
                                (false, _) | (_, None) => view! { <div class="page-loading"><span class="spinner"></span></div> }.into_any(),
                                (_, Some(Err(e))) => view! { <ErrorBanner error=e.to_string() /> }.into_any(),
                                (_, Some(Ok(list))) if list.is_empty() && edit.with(|e| e.is_empty()) => view! {
                                    <p class="muted">
                                        "Nenhum esqueleto cadastrado. "
                                        {if is_admin() {
                                            view! { <A href="/admin/skeletons">"Cadastrar esqueletos"</A> }.into_any()
                                        } else {
                                            "Peça a um administrador para cadastrar (UE4 Mannequin, UE5 Manny…).".into_any()
                                        }}
                                    </p>
                                }
                                .into_any(),
                                _ => rows.into_any(),
                            }}
                            <Show when=is_admin>
                                <A href="/admin/skeletons" attr:class="link-btn">"Gerenciar esqueletos"</A>
                            </Show>
                        </section>
                        <section class="detail-block">
                            <h3>{format!("Clipes ({})", p.animations.len())}</h3>
                            {clip_list(p.animations)}
                        </section>
                    }
                    .into_any(),
                }
            }}
            <Show when=move || can_write.get() && dirty.get()>
                <div class="mat-footer">
                    <input
                        type="text"
                        placeholder="Mensagem da versão (opcional)"
                        prop:value=move || message.get()
                        on:input=move |ev| message.set(event_target_value(&ev))
                    />
                    <div class="mat-footer-actions">
                        <button
                            class="btn btn-ghost btn-sm"
                            disabled=move || saving.get()
                            on:click=move |_| edit.set(baseline.get_untracked().unwrap_or_default())
                        >
                            "Descartar"
                        </button>
                        <button class="btn btn-primary btn-sm" disabled=move || saving.get() on:click=save>
                            {move || if saving.get() { "Salvando…" } else { "Salvar nova versão" }}
                        </button>
                    </div>
                    <p class="muted hint">
                        "Cria uma versão nova com " <code>{ANIMATION_CONFIG_PATH}</code>
                        "; a pré-visualização em cada esqueleto é gerada no processamento."
                    </p>
                </div>
            </Show>
        </div>
    }
}
