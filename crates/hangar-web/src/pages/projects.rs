//! Project list + creation.

use crate::api;
use crate::components::common::{ErrorBanner, Modal, RoleBadge};
use crate::util::{fmt_date, slugify};
use hangar_core::CreateProjectRequest;
use leptos::prelude::*;
use leptos::task::spawn_local;
use leptos_router::components::A;
use leptos_router::hooks::use_navigate;

#[component]
pub fn ProjectsPage() -> impl IntoView {
    let projects = LocalResource::new(api::projects);
    let show_new = RwSignal::new(false);

    view! {
        <section class="page">
            <header class="page-head">
                <div>
                    <p class="eyebrow">"Projetos"</p>
                    <h1>"Seus hangares"</h1>
                </div>
                <button class="btn btn-primary" on:click=move |_| show_new.set(true)>
                    "+ Novo projeto"
                </button>
            </header>
            {move || match projects.get() {
                None => view! { <div class="page-loading"><span class="spinner"></span></div> }.into_any(),
                Some(Err(e)) => view! { <ErrorBanner error=e.to_string() /> }.into_any(),
                Some(Ok(list)) if list.is_empty() => {
                    view! {
                        <div class="empty-state">
                            <div class="empty-code">"00"</div>
                            <p>"Nenhum projeto ainda. Crie o primeiro para começar a guardar assets."</p>
                            <button class="btn btn-primary" on:click=move |_| show_new.set(true)>
                                "Criar projeto"
                            </button>
                        </div>
                    }
                        .into_any()
                }
                Some(Ok(list)) => {
                    view! {
                        <div class="project-grid">
                            {list
                                .into_iter()
                                .map(|p| {
                                    view! {
                                        <A href=format!("/p/{}", p.id) attr:class="project-card">
                                            <div class="project-card-top">
                                                <span class="mono muted">{format!("/{}", p.slug)}</span>
                                                {p.my_role.map(|r| view! { <RoleBadge role=r /> })}
                                            </div>
                                            <h2>{p.name.clone()}</h2>
                                            <p class="muted clamp-2">
                                                {if p.description.is_empty() {
                                                    "Sem descrição".to_string()
                                                } else {
                                                    p.description.clone()
                                                }}
                                            </p>
                                            <div class="project-card-foot">
                                                <span class="stat-num">{p.asset_count}</span>
                                                <span class="muted">" assets"</span>
                                                <span class="muted push-right">{fmt_date(&p.created_at)}</span>
                                            </div>
                                        </A>
                                    }
                                })
                                .collect_view()}
                        </div>
                    }
                        .into_any()
                }
            }}
            <Show when=move || show_new.get()>
                <NewProjectModal on_close=Callback::new(move |_| show_new.set(false)) />
            </Show>
        </section>
    }
}

#[component]
fn NewProjectModal(on_close: Callback<()>) -> impl IntoView {
    let navigate = use_navigate();
    let name = RwSignal::new(String::new());
    let slug = RwSignal::new(String::new());
    let slug_touched = RwSignal::new(false);
    let description = RwSignal::new(String::new());
    let error = RwSignal::new(None::<String>);
    let busy = RwSignal::new(false);

    Effect::new(move |_| {
        let n = name.get();
        if !slug_touched.get_untracked() {
            slug.set(slugify(&n));
        }
    });

    let submit = move |ev: leptos::ev::SubmitEvent| {
        ev.prevent_default();
        busy.set(true);
        let navigate = navigate.clone();
        let req = CreateProjectRequest {
            name: name.get_untracked().trim().to_string(),
            slug: Some(slug.get_untracked()).filter(|s| !s.is_empty()),
            description: description.get_untracked(),
        };
        spawn_local(async move {
            match api::create_project(req).await {
                Ok(p) => navigate(&format!("/p/{}", p.id), Default::default()),
                Err(e) => {
                    busy.set(false);
                    error.set(Some(e.to_string()));
                }
            }
        });
    };

    view! {
        <Modal title="Novo projeto" on_close=on_close>
            <form class="form" on:submit=submit>
                <label class="field">
                    <span>"Nome"</span>
                    <input type="text" required=true placeholder="Ex.: Blindados WW2" bind:value=name />
                </label>
                <label class="field">
                    <span>"Slug"</span>
                    <input
                        type="text"
                        class="mono"
                        pattern="[a-z0-9-]+"
                        prop:value=move || slug.get()
                        on:input=move |ev| {
                            slug_touched.set(true);
                            slug.set(event_target_value(&ev));
                        }
                    />
                </label>
                <label class="field">
                    <span>"Descrição"</span>
                    <textarea rows="3" bind:value=description></textarea>
                </label>
                {move || error.get().map(|e| view! { <ErrorBanner error=e /> })}
                <div class="form-actions">
                    <button type="button" class="btn btn-ghost" on:click=move |_| on_close.run(())>
                        "Cancelar"
                    </button>
                    <button type="submit" class="btn btn-primary" disabled=move || busy.get()>
                        "Criar projeto"
                    </button>
                </div>
            </form>
        </Modal>
    }
}
