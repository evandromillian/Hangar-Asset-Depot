//! Editor plugins of the logged-in user: approve a pairing started in Unreal
//! (`?codigo=KXRT-4QPM`, opened by the plugin), see which editors are online
//! and revoke access tokens.

use crate::api;
use crate::components::common::ErrorBanner;
use crate::util::{fmt_ago, fmt_date};
use hangar_core::{ApiTokenDto, PairInfo};
use leptos::prelude::*;
use leptos::task::spawn_local;
use leptos_router::hooks::use_query_map;

#[component]
pub fn DevicesPage() -> impl IntoView {
    let query = use_query_map();
    let code = Memo::new(move |_| query.with(|q| q.get("codigo")).filter(|c| !c.trim().is_empty()));
    let tokens = LocalResource::new(api::api_tokens);
    let clients = LocalResource::new(api::bridge_clients);
    let error = RwSignal::new(None::<String>);

    view! {
        <section class="page">
            <header class="page-head">
                <div>
                    <p class="eyebrow">"Sua conta"</p>
                    <h1>"Dispositivos"</h1>
                </div>
            </header>
            <p class="muted page-intro">
                "Plugins do Unreal Engine conectados à sua conta. No Unreal, use o botão Hangar da barra de ferramentas "
                "e escolha “Conectar ao Hangar…”: esta página abre com um código para você confirmar. Depois disso, "
                "“Enviar para Unreal” na página de um asset importa a versão no editor."
            </p>
            {move || error.get().map(|e| view! { <ErrorBanner error=e /> })}
            {move || code.get().map(|c| view! { <PairApproval code=c on_done=Callback::new(move |_| tokens.refetch()) /> })}

            <h2 class="section-title">"Editores abertos agora"</h2>
            {move || match clients.get() {
                None => view! { <span class="spinner"></span> }.into_any(),
                Some(Err(e)) => view! { <ErrorBanner error=e.to_string() /> }.into_any(),
                Some(Ok(list)) if list.is_empty() => {
                    view! { <p class="muted">"Nenhum editor conectado neste momento."</p> }.into_any()
                }
                Some(Ok(list)) => view! {
                    <ul class="device-list">
                        {list
                            .into_iter()
                            .map(|c| view! {
                                <li class="device-row">
                                    <span class="device-dot online" title="Conectado"></span>
                                    <span class="device-name">{c.name.clone()}</span>
                                    <span class="muted">{c.detail.clone().unwrap_or_default()}</span>
                                    <span class="muted push-right">{format!("conectado {}", fmt_ago(&c.connected_at))}</span>
                                </li>
                            })
                            .collect_view()}
                    </ul>
                }
                .into_any(),
            }}

            <h2 class="section-title">"Acessos concedidos"</h2>
            {move || match tokens.get() {
                None => view! { <span class="spinner"></span> }.into_any(),
                Some(Err(e)) => view! { <ErrorBanner error=e.to_string() /> }.into_any(),
                Some(Ok(list)) if list.is_empty() => view! { <p class="muted">"Nenhum dispositivo conectado à sua conta."</p> }.into_any(),
                Some(Ok(list)) => view! {
                    <ul class="device-list">
                        {list
                            .into_iter()
                            .map(|t| view! { <TokenRow token=t on_revoked=Callback::new(move |_| tokens.refetch()) error=error /> })
                            .collect_view()}
                    </ul>
                }
                .into_any(),
            }}
        </section>
    }
}

#[component]
fn TokenRow(token: ApiTokenDto, on_revoked: Callback<()>, error: RwSignal<Option<String>>) -> impl IntoView {
    let armed = RwSignal::new(false);
    let id = token.id;
    let revoke = move |_| {
        if !armed.get_untracked() {
            armed.set(true);
            return;
        }
        spawn_local(async move {
            match api::delete_api_token(id).await {
                Ok(()) => on_revoked.run(()),
                Err(e) => error.set(Some(e.to_string())),
            }
        });
    };
    let used = token.last_used_at.map(|t| format!("usado {}", fmt_ago(&t))).unwrap_or_else(|| "nunca usado".into());
    view! {
        <li class="device-row">
            <span class="device-name">{token.name.clone()}</span>
            <span class="muted">{format!("desde {}", fmt_date(&token.created_at))}</span>
            <span class="muted">{used}</span>
            <button
                class="btn btn-sm btn-ghost push-right"
                class:btn-danger=move || armed.get()
                on:click=revoke
                on:mouseleave=move |_| armed.set(false)
                title="O dispositivo perde o acesso e precisa ser conectado de novo"
            >
                {move || if armed.get() { "Confirmar" } else { "Revogar" }}
            </button>
        </li>
    }
}

/// The card that approves or refuses a pending pairing.
#[component]
fn PairApproval(code: String, on_done: Callback<()>) -> impl IntoView {
    let info = {
        let code = code.clone();
        LocalResource::new(move || {
            let code = code.clone();
            async move { api::pairing(&code).await }
        })
    };
    // None: undecided; Some(true): approved; Some(false): refused.
    let decided = RwSignal::new(None::<bool>);
    let busy = RwSignal::new(false);
    let error = RwSignal::new(None::<String>);
    let decide = move |approve: bool| {
        let code = code.clone();
        busy.set(true);
        spawn_local(async move {
            match api::decide_pairing(&code, approve).await {
                Ok(()) => {
                    decided.set(Some(approve));
                    on_done.run(());
                }
                Err(e) => error.set(Some(e.to_string())),
            }
            busy.set(false);
        });
    };
    let approve = {
        let decide = decide.clone();
        move |_| decide(true)
    };
    let refuse = move |_| decide(false);

    view! {
        <div class="pair-card">
            {move || match (decided.get(), info.get()) {
                (Some(true), _) => view! {
                    <p class="banner banner-ok">"Conectado. Volte ao Unreal: o plugin termina a conexão em alguns segundos."</p>
                }
                .into_any(),
                (Some(false), _) => view! { <p class="muted">"Conexão recusada. Nada foi liberado."</p> }.into_any(),
                (None, None) => view! { <span class="spinner"></span> }.into_any(),
                (None, Some(Err(e))) => view! { <ErrorBanner error=e.to_string() /> }.into_any(),
                (None, Some(Ok(p))) => view! { <PairSummary info=p /> }.into_any(),
            }}
            {move || error.get().map(|e| view! { <ErrorBanner error=e /> })}
            <Show when=move || decided.get().is_none() && matches!(info.get(), Some(Ok(_)))>
                <div class="pair-actions">
                    <button class="btn btn-primary" disabled=move || busy.get() on:click=approve.clone()>"Autorizar"</button>
                    <button class="btn btn-ghost" disabled=move || busy.get() on:click=refuse.clone()>"Recusar"</button>
                </div>
            </Show>
        </div>
    }
}

#[component]
fn PairSummary(info: PairInfo) -> impl IntoView {
    let app = match info.client.as_str() {
        "unreal" => "Unreal Engine",
        "blender" => "Blender",
        other => other,
    }
    .to_string();
    view! {
        <p class="eyebrow">"Pedido de conexão"</p>
        <h2>{format!("{app} em {}", info.device_name)}</h2>
        {info.detail.clone().map(|d| view! { <p class="muted">{d}</p> })}
        <p class="pair-code mono">{info.user_code.clone()}</p>
        <p class="muted">
            {format!("Pedido {}. ", fmt_ago(&info.created_at))}
            "Autorize só se você acabou de clicar em “Conectar ao Hangar…” e o código acima é o mesmo que o "
            "Unreal mostra. O editor poderá ler e importar os assets dos seus projetos."
        </p>
    }
}
