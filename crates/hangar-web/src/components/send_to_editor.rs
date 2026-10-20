//! "Enviar para Unreal": sends the selected version to an editor connected
//! through the Hangar plugin and follows the import it reports.

use crate::api;
use hangar_core::{BridgeClientDto, DeliveryDto, DeliveryState, VersionDto, VersionStatus};
use leptos::prelude::*;
use leptos::task::spawn_local;
use leptos_router::components::A;
use std::time::Duration;
use uuid::Uuid;

#[component]
pub fn SendToEditor(version: Signal<Option<VersionDto>>) -> impl IntoView {
    let clients = RwSignal::new(Vec::<BridgeClientDto>::new());
    let refresh = move || {
        spawn_local(async move {
            if let Ok(list) = api::bridge_clients().await {
                clients.set(list);
            }
        })
    };
    refresh();
    // Editors come and go; the list is cheap (in-memory on the server).
    if let Ok(h) = set_interval_with_handle(refresh, Duration::from_secs(10)) {
        on_cleanup(move || h.clear());
    }

    let delivery = RwSignal::new(None::<DeliveryDto>);
    let error = RwSignal::new(None::<String>);
    let picking = RwSignal::new(false);
    if let Ok(h) = set_interval_with_handle(
        move || {
            let Some(d) = delivery.get_untracked().filter(|d| !d.state.is_final()) else { return };
            spawn_local(async move {
                match api::bridge_delivery(d.id).await {
                    Ok(d) => delivery.set(Some(d)),
                    Err(e) => error.set(Some(e.to_string())),
                }
            });
        },
        Duration::from_millis(1000),
    ) {
        on_cleanup(move || h.clear());
    }

    let send_to = move |client: Uuid| {
        let Some(v) = version.get_untracked() else { return };
        picking.set(false);
        error.set(None);
        delivery.set(None);
        spawn_local(async move {
            match api::bridge_send(client, v.id).await {
                Ok(d) => delivery.set(Some(d)),
                Err(e) => {
                    error.set(Some(e.to_string()));
                    refresh();
                }
            }
        });
    };
    let ready = move || version.with(|v| v.as_ref().is_some_and(|v| v.status == VersionStatus::Ready));
    let on_click = move |_| {
        let list = clients.get_untracked();
        match list.as_slice() {
            [only] => send_to(only.id),
            [] => {}
            _ => picking.update(|p| *p = !*p),
        }
    };
    let title = move || {
        let n = clients.with(Vec::len);
        if n == 0 {
            "Nenhum Unreal conectado: abra o Unreal com o plugin Hangar e conecte-o (veja Dispositivos)".to_string()
        } else if !ready() {
            "A versão precisa terminar de processar".to_string()
        } else {
            let v = version.with(|v| v.as_ref().map(|v| v.number).unwrap_or_default());
            match clients.with(|c| c.first().map(|c| c.name.clone())) {
                Some(name) if n == 1 => format!("Importa a v{v} no Unreal de {name}"),
                _ => format!("Importa a v{v} em um dos {n} editores conectados"),
            }
        }
    };

    view! {
        <div class="send-editor">
            <button
                class="btn"
                class:btn-ghost=move || clients.with(Vec::is_empty)
                disabled=move || clients.with(Vec::is_empty) || !ready()
                title=title
                on:click=on_click
            >
                <span class="device-dot" class:online=move || !clients.with(Vec::is_empty)></span>
                "Enviar para Unreal"
            </button>
            <Show when=move || picking.get()>
                <ul class="send-editor-menu" role="menu">
                    {move || {
                        clients
                            .get()
                            .into_iter()
                            .map(|c| {
                                let id = c.id;
                                view! {
                                    <li>
                                        <button class="menu-item" on:click=move |_| send_to(id)>
                                            <span>{c.name.clone()}</span>
                                            <span class="muted">{c.detail.clone().unwrap_or_default()}</span>
                                        </button>
                                    </li>
                                }
                            })
                            .collect_view()
                    }}
                </ul>
            </Show>
            {move || error.get().map(|e| view! { <span class="send-status send-failed" title=e.clone()>{e.clone()}</span> })}
            {move || delivery.get().map(|d| view! { <DeliveryStatus delivery=d /> })}
            <Show when=move || clients.with(Vec::is_empty)>
                <A href="/dispositivos" attr:class="send-help">"Como conectar?"</A>
            </Show>
        </div>
    }
}

#[component]
fn DeliveryStatus(delivery: DeliveryDto) -> impl IntoView {
    let pct = delivery.progress.map(|p| format!(" {:.0}%", p * 100.0)).unwrap_or_default();
    let (cls, text) = match delivery.state {
        DeliveryState::Sent => ("send-busy", format!("Enviado para {}…", delivery.client_name)),
        DeliveryState::Downloading => ("send-busy", format!("Baixando{pct}")),
        DeliveryState::Importing => ("send-busy", format!("Importando{pct}")),
        DeliveryState::Done => ("send-done", format!("Importado em {}", delivery.client_name)),
        DeliveryState::Failed => ("send-failed", "Falhou no Unreal".to_string()),
    };
    let detail = delivery.message.clone().or_else(|| (!delivery.imported.is_empty()).then(|| delivery.imported.join("\n")));
    view! { <span class=format!("send-status {cls}") title=detail.unwrap_or_default()>{text}</span> }
}
