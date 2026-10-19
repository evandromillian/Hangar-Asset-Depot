//! Asset library of a project: search, filters, grid, pagination.

use crate::api;
use crate::components::asset_form::NewAssetModal;
use crate::components::common::{ErrorBanner, KindBadge, StatusBadge};
use crate::components::members::MembersModal;
use crate::util::{fmt_ago, kind_glyph, kind_label, pref_get, pref_set};
use hangar_core::{AssetKind, AssetQuery, AssetSummary};
use leptos::prelude::*;
use leptos_router::components::A;
use leptos_router::hooks::{use_navigate, use_params_map};
use std::time::Duration;
use uuid::Uuid;

const PAGE_SIZE: u64 = 24;
/// localStorage key of the card size: "small" (default) or "large".
const VIEW_PREF: &str = "hangar.library.view";

#[component]
pub fn LibraryPage() -> impl IntoView {
    let params = use_params_map();
    let pid = Memo::new(move |_| params.read().get("pid").and_then(|s| Uuid::parse_str(&s).ok()));

    let project = LocalResource::new(move || {
        let pid = pid.get();
        async move {
            match pid {
                Some(p) => api::project(p).await,
                None => Err(api::ApiError { status: 404, message: "projeto inválido".into() }),
            }
        }
    });

    // Filters. `q_input` follows the text box; `q` is the debounced value.
    let q_input = RwSignal::new(String::new());
    let q = RwSignal::new(String::new());
    let kind = RwSignal::new(None::<AssetKind>);
    let tag = RwSignal::new(String::new());
    let page = RwSignal::new(0u64);
    let large = RwSignal::new(pref_get(VIEW_PREF).as_deref() == Some("large"));
    let set_large = move |v: bool| {
        large.set(v);
        pref_set(VIEW_PREF, if v { "large" } else { "small" });
    };

    let debounce = StoredValue::new(None::<TimeoutHandle>);
    Effect::new(move |_| {
        let v = q_input.get();
        if let Some(h) = debounce.get_value() {
            h.clear();
        }
        let h = set_timeout_with_handle(
            move || {
                q.set(v);
                page.set(0);
            },
            Duration::from_millis(300),
        )
        .ok();
        debounce.set_value(h);
    });

    let assets = LocalResource::new(move || {
        let query = AssetQuery {
            q: Some(q.get()).filter(|s| !s.is_empty()),
            kind: kind.get(),
            tag: Some(tag.get()).filter(|s| !s.is_empty()),
            page: Some(page.get()),
            size: Some(PAGE_SIZE),
        };
        let pid = pid.get();
        async move {
            match pid {
                Some(p) => api::assets(p, &query).await,
                None => Err(api::ApiError { status: 404, message: "projeto inválido".into() }),
            }
        }
    });

    let show_new = RwSignal::new(false);
    let show_members = RwSignal::new(false);
    let navigate = use_navigate();

    let can_write = move || {
        project.get().and_then(|r| r.ok()).and_then(|p| p.my_role).map(|r| r.can_write()).unwrap_or(false)
    };
    let can_admin = move || {
        project.get().and_then(|r| r.ok()).and_then(|p| p.my_role).map(|r| r.can_admin()).unwrap_or(false)
    };

    view! {
        <section class="page">
            <header class="page-head">
                <div>
                    <p class="eyebrow">
                        <A href="/">"Projetos"</A>
                        " / "
                        {move || project.get().and_then(|r| r.ok()).map(|p| p.slug)}
                    </p>
                    <h1>{move || project.get().and_then(|r| r.ok()).map(|p| p.name).unwrap_or_default()}</h1>
                </div>
                <div class="head-actions">
                    <button class="btn btn-ghost" on:click=move |_| show_members.set(true)>"Membros"</button>
                    <Show when=can_write>
                        <button class="btn btn-primary" on:click=move |_| show_new.set(true)>"+ Novo asset"</button>
                    </Show>
                </div>
            </header>

            <div class="filters">
                <div class="search">
                    <span class="search-icon" aria-hidden="true">"⌕"</span>
                    <input type="search" placeholder="Buscar por nome ou slug…" bind:value=q_input />
                </div>
                <div class="chip-row" role="group" aria-label="Tipo">
                    <button
                        class="chip"
                        class:active=move || kind.get().is_none()
                        on:click=move |_| {
                            kind.set(None);
                            page.set(0);
                        }
                    >
                        "Todos"
                    </button>
                    {AssetKind::ALL
                        .iter()
                        .map(|k| {
                            let k = *k;
                            view! {
                                <button
                                    class="chip"
                                    class:active=move || kind.get() == Some(k)
                                    on:click=move |_| {
                                        kind.set(Some(k));
                                        page.set(0);
                                    }
                                >
                                    <span class="kind-glyph">{kind_glyph(k)}</span>
                                    {kind_label(k)}
                                </button>
                            }
                        })
                        .collect_view()}
                </div>
                <div class="filter-end">
                    <div class="view-toggle" role="group" aria-label="Tamanho dos cards">
                        <button
                            class="view-btn"
                            class:active=move || !large.get()
                            title="Cards pequenos"
                            aria-label="Cards pequenos"
                            aria-pressed=move || (!large.get()).to_string()
                            on:click=move |_| set_large(false)
                        >
                            <svg viewBox="0 0 16 16" aria-hidden="true">
                                <rect x="1" y="1" width="4" height="4" rx="0.6" />
                                <rect x="6" y="1" width="4" height="4" rx="0.6" />
                                <rect x="11" y="1" width="4" height="4" rx="0.6" />
                                <rect x="1" y="6" width="4" height="4" rx="0.6" />
                                <rect x="6" y="6" width="4" height="4" rx="0.6" />
                                <rect x="11" y="6" width="4" height="4" rx="0.6" />
                                <rect x="1" y="11" width="4" height="4" rx="0.6" />
                                <rect x="6" y="11" width="4" height="4" rx="0.6" />
                                <rect x="11" y="11" width="4" height="4" rx="0.6" />
                            </svg>
                        </button>
                        <button
                            class="view-btn"
                            class:active=move || large.get()
                            title="Cards grandes"
                            aria-label="Cards grandes"
                            aria-pressed=move || large.get().to_string()
                            on:click=move |_| set_large(true)
                        >
                            <svg viewBox="0 0 16 16" aria-hidden="true">
                                <rect x="1" y="1" width="6.5" height="6.5" rx="0.8" />
                                <rect x="8.5" y="1" width="6.5" height="6.5" rx="0.8" />
                                <rect x="1" y="8.5" width="6.5" height="6.5" rx="0.8" />
                                <rect x="8.5" y="8.5" width="6.5" height="6.5" rx="0.8" />
                            </svg>
                        </button>
                    </div>
                    <input
                        class="tag-filter"
                        type="text"
                        placeholder="filtrar por tag"
                        prop:value=move || tag.get()
                        on:change=move |ev| {
                            tag.set(event_target_value(&ev).trim().trim_start_matches('#').to_lowercase());
                            page.set(0);
                        }
                    />
                </div>
            </div>

            {move || match assets.get() {
                None => view! { <div class="page-loading"><span class="spinner"></span></div> }.into_any(),
                Some(Err(e)) => view! { <ErrorBanner error=e.to_string() /> }.into_any(),
                Some(Ok(p)) if p.items.is_empty() => {
                    view! {
                        <div class="empty-state">
                            <div class="empty-code">"∅"</div>
                            <p>"Nenhum asset encontrado com esses filtros."</p>
                        </div>
                    }
                        .into_any()
                }
                Some(Ok(p)) => {
                    let total = p.total;
                    let pages = total.div_ceil(PAGE_SIZE).max(1);
                    view! {
                        <div class="asset-grid" class:large=move || large.get()>
                            {p
                                .items
                                .into_iter()
                                .map(|a| view! { <AssetCard asset=a tag=tag page=page /> })
                                .collect_view()}
                        </div>
                        <nav class="pager">
                            <button
                                class="btn btn-ghost btn-sm"
                                disabled=move || page.get() == 0
                                on:click=move |_| page.update(|p| *p = p.saturating_sub(1))
                            >
                                "← Anterior"
                            </button>
                            <span class="mono muted">
                                {move || format!("página {} de {pages} · {total} assets", page.get() + 1)}
                            </span>
                            <button
                                class="btn btn-ghost btn-sm"
                                // Braces: a bare `>=` ends the tag inside view!.
                                disabled=move || { page.get() + 1 >= pages }
                                on:click=move |_| page.update(|p| *p += 1)
                            >
                                "Próxima →"
                            </button>
                        </nav>
                    }
                        .into_any()
                }
            }}

            {move || {
                (show_new.get())
                    .then(|| pid.get())
                    .flatten()
                    .map(|p| {
                        let navigate = navigate.clone();
                        view! {
                            <NewAssetModal
                                project_id=p
                                on_close=Callback::new(move |_| show_new.set(false))
                                on_created=Callback::new(move |id: Uuid| {
                                    navigate(&format!("/a/{id}?upload=1"), Default::default())
                                })
                            />
                        }
                    })
            }}
            {move || {
                (show_members.get())
                    .then(|| pid.get())
                    .flatten()
                    .map(|p| {
                        view! {
                            <MembersModal
                                project_id=p
                                can_admin=can_admin()
                                on_close=Callback::new(move |_| show_members.set(false))
                            />
                        }
                    })
            }}
        </section>
    }
}

#[component]
fn AssetCard(asset: AssetSummary, tag: RwSignal<String>, page: RwSignal<u64>) -> impl IntoView {
    let href = format!("/a/{}", asset.id);
    let thumb = asset.thumbnail_url.clone();
    let kind = asset.kind;
    let head_number = asset.head.as_ref().map(|h| h.number);
    let head_status = asset.head.as_ref().map(|h| h.status);
    view! {
        <article class="asset-card">
            <A href=href.clone() attr:class="asset-thumb">
                {match thumb {
                    Some(url) => view! { <img src=url alt="" loading="lazy" /> }.into_any(),
                    None => view! { <span class="thumb-placeholder">{kind_glyph(kind)}</span> }.into_any(),
                }}
                {head_number.map(|n| view! { <span class="version-tag mono">{format!("v{n}")}</span> })}
            </A>
            <div class="asset-card-body">
                <A href=href attr:class="asset-name">{asset.name.clone()}</A>
                <div class="asset-meta">
                    <KindBadge kind=kind />
                    {head_status.map(|s| view! { <StatusBadge status=s /> })}
                </div>
                <div class="asset-tags">
                    {asset
                        .tags
                        .iter()
                        .map(|t| {
                            let t2 = t.clone();
                            let t3 = t.clone();
                            view! {
                                <button
                                    class="tag"
                                    class:active=move || tag.with(|f| *f == t3)
                                    title="Filtrar por esta tag"
                                    on:click=move |_| {
                                        tag.set(t2.clone());
                                        page.set(0);
                                    }
                                >
                                    {t.clone()}
                                </button>
                            }
                        })
                        .collect_view()}
                </div>
                <div class="asset-foot muted">
                    <span>{format!("{} versões", asset.version_count)}</span>
                    <span>{fmt_ago(&asset.updated_at)}</span>
                </div>
            </div>
        </article>
    }
}
