//! System settings: reference skeletons (admins only). Each one points to a
//! model asset (e.g. SKM_Manny.fbx + textures, uploaded like any asset) whose
//! mesh plays the animation-only versions bound to it.

use crate::api;
use crate::app::use_session;
use crate::components::common::{ErrorBanner, Modal};
use crate::util::{fmt_date, slugify};
use hangar_core::{AssetKind, AssetQuery, AssetSummary, CreateSkeletonRequest, SkeletonDto, UpdateSkeletonRequest};
use leptos::prelude::*;
use leptos::task::spawn_local;
use leptos_router::components::A;
use uuid::Uuid;

#[component]
pub fn SkeletonsPage() -> impl IntoView {
    let session = use_session();
    let is_admin = move || session.user.get().is_some_and(|u| u.is_admin);
    let list = LocalResource::new(api::skeletons);
    // None: closed; Some(None): new; Some(Some(s)): editing s.
    let editing = RwSignal::new(None::<Option<SkeletonDto>>);
    let notice = RwSignal::new(None::<String>);
    let error = RwSignal::new(None::<String>);
    let on_saved = Callback::new(move |_| {
        editing.set(None);
        list.refetch();
    });

    view! {
        <section class="page">
            <header class="page-head">
                <div>
                    <p class="eyebrow">"Configurações do sistema"</p>
                    <h1>"Esqueletos de referência"</h1>
                </div>
                <Show when=is_admin>
                    <button class="btn btn-primary" on:click=move |_| editing.set(Some(None))>"+ Novo esqueleto"</button>
                </Show>
            </header>
            <p class="muted page-intro">
                "Animações enviadas sem malha (só o esqueleto animado, como as exportadas do Unreal) são pré-visualizadas nestes modelos, "
                "com os ossos associados pelo nome. Envie o modelo do esqueleto como um asset comum (FBX com skin e texturas) e registre-o aqui. "
                "Animações enviadas junto com uma malha tocam só nela."
            </p>
            {move || notice.get().map(|n| view! { <div class="banner banner-ok">{n}</div> })}
            {move || error.get().map(|e| view! { <ErrorBanner error=e /> })}
            {move || {
                if !is_admin() {
                    return view! { <ErrorBanner error="Somente administradores gerenciam os esqueletos." /> }.into_any();
                }
                match list.get() {
                    None => view! { <div class="page-loading"><span class="spinner"></span></div> }.into_any(),
                    Some(Err(e)) => view! { <ErrorBanner error=e.to_string() /> }.into_any(),
                    Some(Ok(items)) if items.is_empty() => view! {
                        <div class="empty-state">
                            <div class="empty-code">"↻"</div>
                            <p>"Nenhum esqueleto cadastrado. Registre, por exemplo, o UE4 Mannequin e o UE5 Manny."</p>
                            <button class="btn btn-primary" on:click=move |_| editing.set(Some(None))>"Cadastrar esqueleto"</button>
                        </div>
                    }
                    .into_any(),
                    Some(Ok(items)) => view! {
                        <div class="skel-grid">
                            {items
                                .into_iter()
                                .map(|s| view! { <SkeletonCard skeleton=s editing=editing list=list notice=notice error=error /> })
                                .collect_view()}
                        </div>
                    }
                    .into_any(),
                }
            }}
            {move || editing.get().map(|current| view! { <SkeletonModal current=current on_close=Callback::new(move |_| editing.set(None)) on_saved=on_saved /> })}
        </section>
    }
}

#[component]
fn SkeletonCard(
    skeleton: SkeletonDto,
    editing: RwSignal<Option<Option<SkeletonDto>>>,
    list: LocalResource<Result<Vec<SkeletonDto>, api::ApiError>>,
    notice: RwSignal<Option<String>>,
    error: RwSignal<Option<String>>,
) -> impl IntoView {
    let s = skeleton.clone();
    let id = s.id;
    let armed = RwSignal::new(false);
    let busy = RwSignal::new(false);
    let delete = move |_| {
        if !armed.get_untracked() {
            armed.set(true);
            return;
        }
        spawn_local(async move {
            match api::delete_skeleton(id).await {
                Ok(()) => list.refetch(),
                Err(e) => error.set(Some(e.to_string())),
            }
        });
    };
    let reprocess = move |_| {
        busy.set(true);
        spawn_local(async move {
            match api::reprocess_skeleton(id).await {
                Ok(r) => {
                    notice.set(Some(format!("{} versão(ões) de animação na fila para reprocessar.", r.queued)));
                    list.refetch();
                }
                Err(e) => error.set(Some(e.to_string())),
            }
            busy.set(false);
        });
    };
    let edit_copy = skeleton.clone();
    let bound = s.animation_count > 0;
    view! {
        <article class="skel-card">
            <A href=format!("/a/{}", s.asset_id) attr:class="skel-card-thumb checker">
                {s.thumbnail_url.clone().map(|t| view! { <img src=t alt="" loading="lazy" /> })}
            </A>
            <div class="skel-card-body">
                <div class="skel-card-top">
                    <h2>{s.name.clone()}</h2>
                    <span class="mono muted">{s.slug.clone()}</span>
                </div>
                {(!s.description.is_empty()).then(|| view! { <p class="muted clamp-2">{s.description.clone()}</p> })}
                <dl class="kv">
                    <dt>"Modelo"</dt>
                    <dd>
                        <A href=format!("/a/{}", s.asset_id)>{s.asset_name.clone()}</A>
                        {s.head.as_ref().map(|h| format!(" · v{}", h.number))}
                    </dd>
                    <dt>"Animações"</dt>
                    <dd>
                        {format!("{} asset(s)", s.animation_count)}
                        {(s.stale_count > 0).then(|| view! { <span class="skel-stale">{format!(" · {} desatualizado(s)", s.stale_count)}</span> })}
                    </dd>
                    <dt>"Cadastrado"</dt>
                    <dd>{fmt_date(&s.created_at)}</dd>
                </dl>
                {match (s.head.is_some(), s.has_skin) {
                    (false, _) => Some(view! { <p class="skel-warn">"O asset ainda não tem versão: envie o FBX do esqueleto."</p> }.into_any()),
                    (true, Some(false)) => Some(view! { <p class="skel-warn">"A malha da versão atual não tem skin (pesos de ossos): as animações não vão deformá-la."</p> }.into_any()),
                    _ => None,
                }}
                <div class="skel-card-actions">
                    <Show when=move || bound>
                        <button
                            class="btn btn-ghost btn-sm"
                            title="Reprocessa as versões atuais das animações que usam este esqueleto, com a versão atual do modelo"
                            disabled=move || busy.get()
                            on:click=reprocess
                        >
                            "Reprocessar animações"
                        </button>
                    </Show>
                    <button class="btn btn-ghost btn-sm push-right" on:click=move |_| editing.set(Some(Some(edit_copy.clone())))>
                        "Editar"
                    </button>
                    <button
                        class="btn btn-ghost btn-sm"
                        class:btn-danger=move || armed.get()
                        on:click=delete
                        on:mouseleave=move |_| armed.set(false)
                    >
                        {move || if armed.get() { "Confirmar" } else { "Remover" }}
                    </button>
                </div>
            </div>
        </article>
    }
}

/// Create (current = None) or edit a skeleton. The model is picked by project,
/// then among its 3D model assets.
#[component]
fn SkeletonModal(current: Option<SkeletonDto>, on_close: Callback<()>, on_saved: Callback<()>) -> impl IntoView {
    let creating = current.is_none();
    let id = current.as_ref().map(|s| s.id);
    let name = RwSignal::new(current.as_ref().map(|s| s.name.clone()).unwrap_or_default());
    let slug = RwSignal::new(current.as_ref().map(|s| s.slug.clone()).unwrap_or_default());
    let slug_touched = RwSignal::new(!creating);
    let description = RwSignal::new(current.as_ref().map(|s| s.description.clone()).unwrap_or_default());
    let asset = RwSignal::new(current.as_ref().map(|s| s.asset_id));
    let current_asset = current.as_ref().map(|s| (s.asset_id, s.asset_name.clone()));
    let project = RwSignal::new(None::<Uuid>);
    let error = RwSignal::new(None::<String>);
    let busy = RwSignal::new(false);

    Effect::new(move |_| {
        let n = name.get();
        if !slug_touched.get_untracked() {
            slug.set(slugify(&n));
        }
    });

    let projects = LocalResource::new(api::projects);
    let assets = LocalResource::new(move || {
        let pid = project.get();
        async move {
            match pid {
                Some(p) => {
                    let q = AssetQuery { kind: Some(AssetKind::Model), size: Some(200), ..Default::default() };
                    api::assets(p, &q).await.map(|page| page.items)
                }
                None => Ok(Vec::<AssetSummary>::new()),
            }
        }
    });

    let submit = move |ev: leptos::ev::SubmitEvent| {
        ev.prevent_default();
        let Some(asset_id) = asset.get_untracked() else {
            error.set(Some("Escolha o asset do modelo do esqueleto.".into()));
            return;
        };
        busy.set(true);
        error.set(None);
        let name = name.get_untracked().trim().to_string();
        let description = description.get_untracked();
        let slug = slug.get_untracked();
        spawn_local(async move {
            let res = match id {
                None => api::create_skeleton(CreateSkeletonRequest {
                    name,
                    slug: Some(slug).filter(|s| !s.is_empty()),
                    description,
                    asset_id,
                })
                .await
                .map(|_| ()),
                Some(id) => api::update_skeleton(
                    id,
                    UpdateSkeletonRequest { name: Some(name), description: Some(description), asset_id: Some(asset_id) },
                )
                .await
                .map(|_| ()),
            };
            busy.set(false);
            match res {
                Ok(()) => on_saved.run(()),
                Err(e) => error.set(Some(e.to_string())),
            }
        });
    };

    view! {
        <Modal title=if creating { "Novo esqueleto" } else { "Editar esqueleto" } on_close=on_close>
            <form class="form" on:submit=submit>
                <label class="field">
                    <span>"Nome"</span>
                    <input type="text" required=true placeholder="Ex.: UE5 Manny" bind:value=name />
                </label>
                <label class="field">
                    <span>"Identificador"</span>
                    <input
                        type="text"
                        class="mono"
                        pattern="[a-z0-9-]+"
                        disabled=!creating
                        title="Usado em .hangar/animation.json; não muda depois de criado"
                        prop:value=move || slug.get()
                        on:input=move |ev| {
                            slug_touched.set(true);
                            slug.set(event_target_value(&ev));
                        }
                    />
                </label>
                <label class="field">
                    <span>"Descrição"</span>
                    <textarea rows="2" placeholder="Ex.: esqueleto padrão do UE 5.x (SK_Mannequin)" bind:value=description></textarea>
                </label>
                {current_asset.map(|(_, n)| view! { <p class="muted hint">"Modelo atual: " <b>{n}</b> " — escolha outro abaixo para trocar."</p> })}
                <div class="form-grid">
                    <label class="field">
                        <span>"Projeto"</span>
                        <select on:change=move |ev| {
                            project.set(Uuid::parse_str(&event_target_value(&ev)).ok());
                        }>
                            <option value="" selected=true>"Escolha…"</option>
                            {move || {
                                projects
                                    .get()
                                    .and_then(|r| r.ok())
                                    .unwrap_or_default()
                                    .into_iter()
                                    .map(|p| view! { <option value=p.id.to_string()>{p.name.clone()}</option> })
                                    .collect_view()
                            }}
                        </select>
                    </label>
                    <label class="field">
                        <span>"Asset do modelo"</span>
                        <select
                            disabled=move || project.get().is_none()
                            on:change=move |ev| {
                                if let Ok(a) = Uuid::parse_str(&event_target_value(&ev)) {
                                    asset.set(Some(a));
                                }
                            }
                        >
                            <option value="" selected=true disabled=true>
                                {move || if project.get().is_some() { "Escolha…" } else { "—" }}
                            </option>
                            {move || {
                                assets
                                    .get()
                                    .and_then(|r| r.ok())
                                    .unwrap_or_default()
                                    .into_iter()
                                    .map(|a| {
                                        let label = match &a.head {
                                            Some(h) => format!("{} (v{})", a.name, h.number),
                                            None => format!("{} (sem versão)", a.name),
                                        };
                                        view! { <option value=a.id.to_string() selected=move || asset.get() == Some(a.id)>{label}</option> }
                                    })
                                    .collect_view()
                            }}
                        </select>
                    </label>
                </div>
                {move || error.get().map(|e| view! { <ErrorBanner error=e /> })}
                <div class="form-actions">
                    <button type="button" class="btn btn-ghost" on:click=move |_| on_close.run(())>"Cancelar"</button>
                    <button type="submit" class="btn btn-primary" disabled=move || busy.get()>
                        {if creating { "Cadastrar" } else { "Salvar" }}
                    </button>
                </div>
            </form>
        </Modal>
    }
}
