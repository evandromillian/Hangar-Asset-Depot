//! First-run setup (admin registration) and login.

use crate::api;
use crate::app::use_session;
use hangar_core::RegisterRequest;
use leptos::prelude::*;
use leptos::task::spawn_local;
use leptos_router::hooks::use_navigate;

#[component]
pub fn LoginPage() -> impl IntoView {
    let session = use_session();
    let navigate = use_navigate();
    let setup = LocalResource::new(api::setup_status);

    // Already logged in → go home.
    {
        let navigate = navigate.clone();
        Effect::new(move |_| {
            if session.user.get().is_some() {
                navigate("/", Default::default());
            }
        });
    }

    let register_mode = RwSignal::new(false);
    let username = RwSignal::new(String::new());
    let display_name = RwSignal::new(String::new());
    let password = RwSignal::new(String::new());
    let error = RwSignal::new(None::<String>);
    let busy = RwSignal::new(false);

    // First run: force the registration form.
    Effect::new(move |_| {
        if let Some(Ok(s)) = setup.get() {
            if s.needs_setup {
                register_mode.set(true);
            }
        }
    });

    let submit = move |ev: leptos::ev::SubmitEvent| {
        ev.prevent_default();
        if busy.get_untracked() {
            return;
        }
        let navigate = navigate.clone();
        busy.set(true);
        error.set(None);
        let (u, p, d, reg) = (
            username.get_untracked().trim().to_string(),
            password.get_untracked(),
            display_name.get_untracked().trim().to_string(),
            register_mode.get_untracked(),
        );
        spawn_local(async move {
            let res = if reg {
                api::register(RegisterRequest {
                    username: u,
                    password: p,
                    display_name: (!d.is_empty()).then_some(d),
                })
                .await
            } else {
                api::login(u, p).await
            };
            busy.set(false);
            match res {
                Ok(r) => {
                    session.user.set(Some(r.user));
                    navigate("/", Default::default());
                }
                Err(e) => error.set(Some(if e.status == 401 {
                    "Usuário ou senha inválidos.".into()
                } else {
                    e.to_string()
                })),
            }
        });
    };

    let needs_setup = move || matches!(setup.get(), Some(Ok(s)) if s.needs_setup);
    let allow_signup = move || matches!(setup.get(), Some(Ok(s)) if s.allow_signup);

    view! {
        <section class="auth-wrap">
            <div class="auth-card">
                <div class="auth-hero">
                    <span class="brand-mark big" aria-hidden="true"></span>
                    <h1>"HANGAR"</h1>
                    <p class="muted">"Depósito versionado de modelos, texturas e áudio."</p>
                </div>
                {move || {
                    setup
                        .get()
                        .and_then(|r| r.err())
                        .map(|e| {
                            view! {
                                <div class="banner banner-error">
                                    "Servidor indisponível — " {e.to_string()}
                                </div>
                            }
                        })
                }}
                <Show when=needs_setup>
                    <div class="banner banner-accent">
                        "Primeira execução: a conta criada agora será a de administrador."
                    </div>
                </Show>
                <form class="form" on:submit=submit>
                    <h2>{move || if register_mode.get() { "Criar conta" } else { "Entrar" }}</h2>
                    <label class="field">
                        <span>"Usuário"</span>
                        <input type="text" autocomplete="username" required=true bind:value=username />
                    </label>
                    <Show when=move || register_mode.get()>
                        <label class="field">
                            <span>"Nome de exibição"</span>
                            <input type="text" placeholder="opcional" bind:value=display_name />
                        </label>
                    </Show>
                    <label class="field">
                        <span>"Senha"</span>
                        <input
                            type="password"
                            required=true
                            minlength="8"
                            autocomplete=move || if register_mode.get() { "new-password" } else { "current-password" }
                            bind:value=password
                        />
                    </label>
                    {move || error.get().map(|e| view! { <div class="banner banner-error">{e}</div> })}
                    <button class="btn btn-primary btn-block" type="submit" disabled=move || busy.get()>
                        {move || {
                            if busy.get() {
                                "Aguarde…"
                            } else if register_mode.get() {
                                "Criar conta"
                            } else {
                                "Entrar"
                            }
                        }}
                    </button>
                    <Show when=move || allow_signup() && !needs_setup()>
                        <button
                            type="button"
                            class="btn btn-link"
                            on:click=move |_| register_mode.update(|m| *m = !*m)
                        >
                            {move || if register_mode.get() { "Já tenho conta" } else { "Criar uma conta" }}
                        </button>
                    </Show>
                </form>
            </div>
        </section>
    }
}
