//! Project members (owners can add / remove).

use crate::api;
use crate::components::common::{ErrorBanner, Modal, RoleBadge};
use hangar_core::{AddMemberRequest, Role};
use leptos::prelude::*;
use leptos::task::spawn_local;
use uuid::Uuid;

#[component]
pub fn MembersModal(project_id: Uuid, can_admin: bool, on_close: Callback<()>) -> impl IntoView {
    let members = LocalResource::new(move || api::members(project_id));
    let username = RwSignal::new(String::new());
    let role = RwSignal::new(Role::Editor);
    let error = RwSignal::new(None::<String>);

    let add = move |ev: leptos::ev::SubmitEvent| {
        ev.prevent_default();
        let req = AddMemberRequest { username: username.get_untracked().trim().to_string(), role: role.get_untracked() };
        spawn_local(async move {
            match api::add_member(project_id, req).await {
                Ok(_) => {
                    username.set(String::new());
                    error.set(None);
                    members.refetch();
                }
                Err(e) => error.set(Some(e.to_string())),
            }
        });
    };

    view! {
        <Modal title="Membros do projeto" on_close=on_close>
            {move || match members.get() {
                None => view! { <span class="spinner"></span> }.into_any(),
                Some(Err(e)) => view! { <ErrorBanner error=e.to_string() /> }.into_any(),
                Some(Ok(list)) => {
                    view! {
                        <ul class="member-list">
                            {list
                                .into_iter()
                                .map(|m| {
                                    let uid = m.user.id;
                                    view! {
                                        <li>
                                            <span class="avatar sm">
                                                {m.user.display_name.chars().next().unwrap_or('?').to_uppercase().to_string()}
                                            </span>
                                            <span class="grow">
                                                {m.user.display_name.clone()} " "
                                                <span class="muted mono">"@" {m.user.username.clone()}</span>
                                            </span>
                                            <RoleBadge role=m.role />
                                            {can_admin
                                                .then(|| {
                                                    view! {
                                                        <button
                                                            class="icon-btn"
                                                            title="Remover"
                                                            on:click=move |_| {
                                                                spawn_local(async move {
                                                                    match api::remove_member(project_id, uid).await {
                                                                        Ok(_) => members.refetch(),
                                                                        Err(e) => error.set(Some(e.to_string())),
                                                                    }
                                                                });
                                                            }
                                                        >
                                                            "✕"
                                                        </button>
                                                    }
                                                })}
                                        </li>
                                    }
                                })
                                .collect_view()}
                        </ul>
                    }
                        .into_any()
                }
            }}
            <Show when=move || can_admin>
                <form class="inline-form" on:submit=add>
                    <input type="text" placeholder="usuário" required=true bind:value=username />
                    <select on:change=move |ev| {
                        if let Ok(r) = event_target_value(&ev).parse::<Role>() {
                            role.set(r);
                        }
                    }>
                        <option value="viewer">"leitura"</option>
                        <option value="editor" selected>"editor"</option>
                        <option value="owner">"dono"</option>
                    </select>
                    <button class="btn btn-primary" type="submit">"Adicionar"</button>
                </form>
            </Show>
            {move || error.get().map(|e| view! { <ErrorBanner error=e /> })}
        </Modal>
    }
}
