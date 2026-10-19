//! Asset page: preview stage + version history / files / details.

use crate::api;
use crate::components::animation::AnimationPanel;
use crate::components::asset_form::EditAssetModal;
use crate::components::common::{ErrorBanner, KindBadge};
use crate::components::details::DetailsPanel;
use crate::components::files::FilesPanel;
use crate::components::materials::MaterialsPanel;
use crate::components::new_version::NewVersionDialog;
use crate::components::stage::Stage;
use crate::components::versions::VersionTimeline;
use crate::viewer::ViewerDraft;
use hangar_core::{AssetDetail, AssetKind, PreviewKind, UpdateAssetRequest, VersionDto, VersionStatus};
use leptos::prelude::*;
use leptos::task::spawn_local;
use leptos_router::components::A;
use leptos_router::hooks::{use_navigate, use_params_map, use_query_map};
use std::time::Duration;
use uuid::Uuid;

#[derive(Clone, Copy, PartialEq)]
enum Tab {
    Versions,
    Files,
    Materials,
    Animation,
    Details,
}

#[component]
pub fn AssetPage() -> impl IntoView {
    let params = use_params_map();
    let query = use_query_map();
    let aid = Memo::new(move |_| params.read().get("aid").and_then(|s| Uuid::parse_str(&s).ok()));
    let aid_or_nil = Signal::derive(move || aid.get().unwrap_or(Uuid::nil()));

    let asset = LocalResource::new(move || {
        let aid = aid.get();
        async move {
            match aid {
                Some(a) => api::asset(a).await,
                None => Err(api::ApiError { status: 404, message: "asset inválido".into() }),
            }
        }
    });
    let versions_res = LocalResource::new(move || {
        let aid = aid.get();
        async move {
            match aid {
                Some(a) => api::versions(a).await,
                None => Ok(vec![]),
            }
        }
    });

    // Keep the last good list so polling refreshes don't flash the UI.
    let versions = RwSignal::new(Vec::<VersionDto>::new());
    Effect::new(move |_| {
        if let Some(Ok(list)) = versions_res.get() {
            if versions.get_untracked() != list {
                versions.set(list);
            }
        }
    });
    let asset_data = RwSignal::new(None::<AssetDetail>);
    Effect::new(move |_| {
        if let Some(Ok(a)) = asset.get() {
            asset_data.set(Some(a));
        }
    });
    // New asset id → reset local state.
    let selected = RwSignal::new(None::<Uuid>);
    Effect::new(move |_| {
        aid.track();
        selected.set(None);
        versions.set(vec![]);
        asset_data.set(None);
    });

    // Default selection: head, else newest.
    Effect::new(move |_| {
        let list = versions.get();
        let sel = selected.get_untracked();
        if sel.map(|s| list.iter().any(|v| v.id == s)) != Some(true) {
            let pick = list.iter().find(|v| v.is_head).or(list.iter().max_by_key(|v| v.number)).map(|v| v.id);
            if pick.is_some() {
                selected.set(pick);
            }
        }
    });

    let selected_version = Signal::derive(move || {
        let s = selected.get()?;
        versions.with(|l| l.iter().find(|v| v.id == s).cloned())
    });
    let versions_sig = Signal::derive(move || versions.get());
    let asset_sig = Signal::derive(move || asset_data.get());
    let project_id = Signal::derive(move || asset_sig.with(|a| a.as_ref().map(|a| a.summary.project_id)));

    // Poll while something is being processed.
    let busy = Memo::new(move |_| {
        versions.with(|l| l.iter().any(|v| matches!(v.status, VersionStatus::Pending | VersionStatus::Processing)))
    });
    if let Ok(handle) = set_interval_with_handle(
        move || {
            if busy.get_untracked() {
                versions_res.refetch();
                asset.refetch();
            }
        },
        Duration::from_millis(2500),
    ) {
        on_cleanup(move || handle.clear());
    }

    let can_write = Signal::derive(move || {
        asset_data.get().and_then(|a| a.my_role).map(|r| r.can_write()).unwrap_or(false)
    });

    let tab = RwSignal::new(Tab::Versions);
    // Unsaved state of the materials tab, previewed by the viewer.
    let draft = RwSignal::new(None::<ViewerDraft>);
    let draft_sig = Signal::derive(move || draft.get());
    let on_material_commit = Callback::new(move |v: VersionDto| {
        selected.set(Some(v.id));
        versions_res.refetch();
        asset.refetch();
    });
    let show_upload = RwSignal::new(query.get_untracked().get("upload").is_some());
    // The upload dialog opened from the Files tab: add / replace files on the selected version.
    let adding_files = RwSignal::new(false);
    let show_edit = RwSignal::new(false);
    let action_error = RwSignal::new(None::<String>);

    let on_set_head = Callback::new(move |vid: Uuid| {
        let Some(a) = aid.get_untracked() else { return };
        spawn_local(async move {
            let req = UpdateAssetRequest { head_version_id: Some(vid), ..Default::default() };
            match api::update_asset(a, req).await {
                Ok(d) => {
                    asset_data.set(Some(d));
                    versions_res.refetch();
                }
                Err(e) => action_error.set(Some(e.to_string())),
            }
        });
    });
    let on_reprocess = Callback::new(move |vid: Uuid| {
        spawn_local(async move {
            match api::reprocess(vid).await {
                Ok(_) => versions_res.refetch(),
                Err(e) => action_error.set(Some(e.to_string())),
            }
        });
    });
    let open_upload = Callback::new(move |_| {
        adding_files.set(false);
        show_upload.set(true);
    });
    let open_add_files = Callback::new(move |_| {
        adding_files.set(true);
        show_upload.set(true);
    });

    // Two-step delete (owners only): first click arms, second deletes.
    let delete_armed = RwSignal::new(false);
    let is_owner = Signal::derive(move || {
        asset_data.get().and_then(|a| a.my_role).map(|r| r.can_admin()).unwrap_or(false)
    });
    let navigate = use_navigate();
    let on_delete = move |_| {
        if !delete_armed.get_untracked() {
            delete_armed.set(true);
            return;
        }
        let Some(a) = asset_data.get_untracked() else { return };
        let navigate = navigate.clone();
        spawn_local(async move {
            match api::delete_asset(a.summary.id).await {
                Ok(_) => navigate(&format!("/p/{}", a.summary.project_id), Default::default()),
                Err(e) => {
                    delete_armed.set(false);
                    action_error.set(Some(e.to_string()));
                }
            }
        });
    };

    view! {
        <section class="page asset-page">
            {move || match asset.get() {
                Some(Err(e)) if asset_data.get().is_none() => view! { <ErrorBanner error=e.to_string() /> }.into_any(),
                _ => ().into_any(),
            }}
            <header class="asset-head">
                {move || {
                    asset_data
                        .get()
                        .map(|a| {
                            let s = a.summary.clone();
                            view! {
                                <div class="asset-title">
                                    <p class="eyebrow">
                                        <A href="/">"Projetos"</A>
                                        " / "
                                        <A href=format!("/p/{}", s.project_id)>"biblioteca"</A>
                                        " / "
                                        <span class="mono">{s.slug.clone()}</span>
                                    </p>
                                    <h1>{s.name.clone()}</h1>
                                    <div class="asset-meta">
                                        <KindBadge kind=s.kind />
                                        {a.source.vendor.clone().map(|v| view! { <span class="pill">{v}</span> })}
                                        {a.source.license.clone().map(|l| view! { <span class="pill pill-outline">{l}</span> })}
                                        {s.tags.iter().map(|t| view! { <span class="tag static">{t.clone()}</span> }).collect_view()}
                                    </div>
                                </div>
                            }
                        })
                }}
                <div class="head-actions">
                    <Show when=move || is_owner.get()>
                        <button
                            class="btn btn-ghost"
                            class:btn-danger=move || delete_armed.get()
                            on:click=on_delete.clone()
                            on:mouseleave=move |_| delete_armed.set(false)
                        >
                            {move || if delete_armed.get() { "Confirmar exclusão" } else { "Excluir" }}
                        </button>
                    </Show>
                    <Show when=move || can_write.get()>
                        <button class="btn btn-ghost" on:click=move |_| show_edit.set(true)>"Editar"</button>
                        <button class="btn btn-primary" on:click=move |_| open_upload.run(())>"+ Nova versão"</button>
                    </Show>
                </div>
            </header>
            {move || action_error.get().map(|e| view! { <ErrorBanner error=e /> })}

            <div class="asset-layout">
                <div class="stage-col">
                    <Stage
                        version=selected_version
                        asset_id=aid_or_nil
                        can_write=can_write
                        on_reprocess=on_reprocess
                        on_upload=open_upload
                        draft=draft_sig
                    />
                </div>
                <aside class="side-col">
                    <nav class="tabs" role="tablist">
                        <button class="tab" class:active=move || tab.get() == Tab::Versions on:click=move |_| tab.set(Tab::Versions)>
                            "Versões " <span class="count">{move || versions.with(|l| l.len())}</span>
                        </button>
                        <button class="tab" class:active=move || tab.get() == Tab::Files on:click=move |_| tab.set(Tab::Files)>
                            "Arquivos " <span class="count">{move || selected_version.get().map(|v| v.file_count).unwrap_or(0)}</span>
                        </button>
                        <button class="tab" class:active=move || tab.get() == Tab::Materials on:click=move |_| tab.set(Tab::Materials)>
                            "Materiais " <span class="count">{move || selected_version.get().and_then(|v| v.preview).map(|p| p.materials.len()).unwrap_or(0)}</span>
                        </button>
                        <Show when=move || {
                            tab.get() == Tab::Animation
                                || asset_data.get().is_some_and(|a| a.summary.kind == AssetKind::Animation)
                                || selected_version
                                    .get()
                                    .and_then(|v| v.preview)
                                    .is_some_and(|p| p.kind == Some(PreviewKind::Animation) || !p.animations.is_empty())
                        }>
                            <button class="tab" class:active=move || tab.get() == Tab::Animation on:click=move |_| tab.set(Tab::Animation)>
                                "Animação " <span class="count">{move || selected_version.get().and_then(|v| v.preview).map(|p| p.animations.len()).unwrap_or(0)}</span>
                            </button>
                        </Show>
                        <button class="tab" class:active=move || tab.get() == Tab::Details on:click=move |_| tab.set(Tab::Details)>
                            "Detalhes"
                        </button>
                    </nav>
                    <div class="tab-body">
                        {move || match tab.get() {
                            Tab::Versions => view! {
                                <VersionTimeline
                                    versions=versions_sig
                                    selected=selected
                                    can_write=can_write
                                    on_set_head=on_set_head
                                    on_reprocess=on_reprocess
                                    on_new=open_upload
                                />
                            }
                            .into_any(),
                            Tab::Files => view! {
                                <FilesPanel
                                    version=selected_version
                                    versions=versions_sig
                                    can_write=can_write
                                    on_add=open_add_files
                                />
                            }
                            .into_any(),
                            Tab::Materials => view! {
                                <MaterialsPanel
                                    version=selected_version
                                    asset_id=aid_or_nil
                                    project_id=project_id
                                    can_write=can_write
                                    draft=draft
                                    on_committed=on_material_commit
                                />
                            }
                            .into_any(),
                            Tab::Animation => view! {
                                <AnimationPanel
                                    version=selected_version
                                    asset_id=aid_or_nil
                                    can_write=can_write
                                    on_reprocess=on_reprocess
                                    on_committed=on_material_commit
                                    on_add=open_add_files
                                />
                            }
                            .into_any(),
                            Tab::Details => view! { <DetailsPanel asset=asset_sig version=selected_version /> }.into_any(),
                        }}
                    </div>
                </aside>
            </div>

            {move || {
                (show_upload.get() && can_write.get())
                    .then(|| aid.get())
                    .flatten()
                    .map(|a| {
                        let list = versions.get_untracked();
                        let default_parent = selected
                            .get_untracked()
                            .or_else(|| list.iter().find(|v| v.is_head).map(|v| v.id));
                        let package = asset_data.get_untracked().is_some_and(|a| a.summary.kind == AssetKind::Animation);
                        let title = match (adding_files.get_untracked(), package) {
                            (true, true) => "Adicionar animações ao pacote",
                            (true, false) => "Adicionar ou substituir arquivos",
                            (false, true) => "Nova versão do pacote de animações",
                            (false, false) => "Nova versão",
                        };
                        view! {
                            <NewVersionDialog
                                title=title
                                asset_id=a
                                versions=list
                                default_parent=default_parent
                                on_close=Callback::new(move |_| show_upload.set(false))
                                on_committed=Callback::new(move |v: VersionDto| {
                                    show_upload.set(false);
                                    selected.set(Some(v.id));
                                    versions_res.refetch();
                                    asset.refetch();
                                })
                            />
                        }
                    })
            }}
            {move || {
                (show_edit.get())
                    .then(|| asset_data.get_untracked())
                    .flatten()
                    .map(|a| {
                        view! {
                            <EditAssetModal
                                asset=a
                                on_close=Callback::new(move |_| show_edit.set(false))
                                on_saved=Callback::new(move |d: AssetDetail| {
                                    asset_data.set(Some(d));
                                    show_edit.set(false);
                                })
                            />
                        }
                    })
            }}
        </section>
    }
}
