//! Small shared UI pieces.

use crate::util::{event_key, kind_glyph, kind_label, status_label};
use hangar_core::{AssetKind, Role, VersionStatus};
use leptos::prelude::*;

#[component]
pub fn Modal(#[prop(into)] title: String, on_close: Callback<()>, children: Children, #[prop(optional)] wide: bool) -> impl IntoView {
    // Esc closes the dialog.
    let handle = window_event_listener(leptos::ev::keydown, move |ev| {
        if event_key(&ev).as_deref() == Some("Escape") {
            on_close.run(());
        }
    });
    on_cleanup(move || handle.remove());
    view! {
        <div class="modal-backdrop" on:click=move |_| on_close.run(())>
            <div
                class="modal"
                class:modal-wide=wide
                role="dialog"
                aria-modal="true"
                on:click=|ev| ev.stop_propagation()
            >
                <header class="modal-head">
                    <h2>{title}</h2>
                    <button class="icon-btn" aria-label="Fechar" on:click=move |_| on_close.run(())>
                        "✕"
                    </button>
                </header>
                <div class="modal-body">{children()}</div>
            </div>
        </div>
    }
}

#[component]
pub fn ErrorBanner(#[prop(into)] error: String) -> impl IntoView {
    view! { <div class="banner banner-error" role="alert">{error}</div> }
}

#[component]
pub fn RoleBadge(role: Role) -> impl IntoView {
    let label = match role {
        Role::Owner => "dono",
        Role::Editor => "editor",
        Role::Viewer => "leitura",
    };
    view! { <span class=format!("pill role-{}", role.as_str())>{label}</span> }
}

#[component]
pub fn StatusBadge(status: VersionStatus) -> impl IntoView {
    view! {
        <span class=format!("status status-{}", status.as_str())>
            <span class="status-dot"></span>
            {status_label(status)}
        </span>
    }
}

#[component]
pub fn KindBadge(kind: AssetKind) -> impl IntoView {
    view! {
        <span class=format!("kind kind-{}", kind.as_str())>
            <span class="kind-glyph">{kind_glyph(kind)}</span>
            {kind_label(kind)}
        </span>
    }
}

#[component]
pub fn Spinner() -> impl IntoView {
    view! { <span class="spinner" aria-label="carregando"></span> }
}
