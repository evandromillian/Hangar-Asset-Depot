//! Root component: session handling, routes and the top bar.

use crate::api;
use crate::pages::{
    asset::AssetPage, devices::DevicesPage, library::LibraryPage, login::LoginPage, projects::ProjectsPage,
    skeletons::SkeletonsPage,
};
use hangar_core::UserDto;
use leptos::prelude::*;
use leptos::task::spawn_local;
use leptos_router::components::{A, Route, Router, Routes};
use leptos_router::hooks::use_navigate;
use leptos_router::path;

/// Logged-in user, shared through context.
#[derive(Clone, Copy)]
pub struct Session {
    pub user: RwSignal<Option<UserDto>>,
    /// True once `/api/auth/me` has answered (successfully or not).
    pub checked: RwSignal<bool>,
}

pub fn use_session() -> Session {
    expect_context::<Session>()
}

#[component]
pub fn App() -> impl IntoView {
    let session = Session { user: RwSignal::new(None), checked: RwSignal::new(false) };
    provide_context(session);

    spawn_local(async move {
        if let Ok(user) = api::me().await {
            session.user.set(Some(user));
        }
        session.checked.set(true);
    });

    view! {
        <Router>
            <TopBar />
            <main class="app-main">
                <Routes fallback=|| view! { <NotFound /> }>
                    <Route path=path!("/login") view=LoginPage />
                    <Route path=path!("/") view=|| view! { <RequireAuth><ProjectsPage /></RequireAuth> } />
                    <Route path=path!("/p/:pid") view=|| view! { <RequireAuth><LibraryPage /></RequireAuth> } />
                    <Route path=path!("/a/:aid") view=|| view! { <RequireAuth><AssetPage /></RequireAuth> } />
                    <Route path=path!("/admin/skeletons") view=|| view! { <RequireAuth><SkeletonsPage /></RequireAuth> } />
                    <Route path=path!("/dispositivos") view=|| view! { <RequireAuth><DevicesPage /></RequireAuth> } />
                </Routes>
            </main>
        </Router>
    }
}

/// Renders children only for authenticated users; otherwise goes to /login,
/// which comes back here afterwards.
#[component]
fn RequireAuth(children: ChildrenFn) -> impl IntoView {
    let session = use_session();
    let navigate = use_navigate();
    Effect::new(move |_| {
        if session.checked.get() && session.user.get().is_none() {
            let here = web_sys::window()
                .map(|w| {
                    let loc = w.location();
                    format!("{}{}", loc.pathname().unwrap_or_default(), loc.search().unwrap_or_default())
                })
                .unwrap_or_default();
            let login = match here.as_str() {
                "" | "/" => "/login".to_string(),
                _ => format!("/login?volta={}", api::encode(&here)),
            };
            navigate(&login, Default::default());
        }
    });
    move || {
        if session.user.get().is_some() {
            children().into_any()
        } else {
            view! { <div class="page-loading"><span class="spinner"></span></div> }.into_any()
        }
    }
}

#[component]
fn TopBar() -> impl IntoView {
    let session = use_session();
    let navigate = use_navigate();
    let logout = move |_| {
        let navigate = navigate.clone();
        spawn_local(async move {
            let _ = api::logout().await;
            session.user.set(None);
            navigate("/login", Default::default());
        });
    };
    view! {
        <header class="topbar">
            <A href="/" attr:class="brand">
                <span class="brand-mark" aria-hidden="true"></span>
                <span class="brand-name">"HANGAR"</span>
                <span class="brand-sub">"asset depot"</span>
            </A>
            <div class="topbar-spacer"></div>
            {move || {
                session
                    .user
                    .get()
                    .map(|u| {
                        let initials: String = u
                            .display_name
                            .split_whitespace()
                            .filter_map(|w| w.chars().next())
                            .take(2)
                            .collect::<String>()
                            .to_uppercase();
                        view! {
                            <A href="/dispositivos" attr:class="topbar-link">"Dispositivos"</A>
                            {u.is_admin.then(|| view! { <A href="/admin/skeletons" attr:class="topbar-link">"Esqueletos"</A> })}
                            <div class="userbox">
                                <span class="avatar" title=u.username.clone()>{initials}</span>
                                <span class="user-name">{u.display_name.clone()}</span>
                                {u.is_admin.then(|| view! { <span class="pill pill-accent">"admin"</span> })}
                                <button class="btn btn-ghost btn-sm" on:click=logout.clone()>"Sair"</button>
                            </div>
                        }
                    })
            }}
        </header>
    }
}

#[component]
fn NotFound() -> impl IntoView {
    view! {
        <section class="empty-state">
            <div class="empty-code">"404"</div>
            <p>"Nada estacionado neste hangar."</p>
            <A href="/" attr:class="btn">"Voltar aos projetos"</A>
        </section>
    }
}
