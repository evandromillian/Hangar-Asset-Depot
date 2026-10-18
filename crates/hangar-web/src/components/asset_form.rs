//! Create / edit asset metadata (name, kind, tags and purchase source).

use crate::api;
use crate::components::common::{ErrorBanner, Modal};
use crate::components::tags::TagInput;
use crate::util::{kind_label, slugify};
use chrono::NaiveDate;
use hangar_core::{AssetDetail, AssetKind, AssetSource, CreateAssetRequest, UpdateAssetRequest};
use leptos::prelude::*;
use leptos::task::spawn_local;
use uuid::Uuid;

#[derive(Clone, Copy)]
struct Fields {
    name: RwSignal<String>,
    slug: RwSignal<String>,
    slug_touched: RwSignal<bool>,
    kind: RwSignal<AssetKind>,
    description: RwSignal<String>,
    tags: RwSignal<Vec<String>>,
    vendor: RwSignal<String>,
    url: RwSignal<String>,
    author: RwSignal<String>,
    license: RwSignal<String>,
    order_ref: RwSignal<String>,
    purchased_at: RwSignal<String>,
    notes: RwSignal<String>,
}

fn opt(s: String) -> Option<String> {
    let t = s.trim().to_string();
    (!t.is_empty()).then_some(t)
}

impl Fields {
    fn new(initial: Option<&AssetDetail>) -> Self {
        let src = initial.map(|a| a.source.clone()).unwrap_or_default();
        let s = |v: Option<String>| RwSignal::new(v.unwrap_or_default());
        Self {
            name: RwSignal::new(initial.map(|a| a.summary.name.clone()).unwrap_or_default()),
            slug: RwSignal::new(initial.map(|a| a.summary.slug.clone()).unwrap_or_default()),
            slug_touched: RwSignal::new(initial.is_some()),
            kind: RwSignal::new(initial.map(|a| a.summary.kind).unwrap_or(AssetKind::Model)),
            description: RwSignal::new(initial.map(|a| a.description.clone()).unwrap_or_default()),
            tags: RwSignal::new(initial.map(|a| a.summary.tags.clone()).unwrap_or_default()),
            vendor: s(src.vendor),
            url: s(src.url),
            author: s(src.author),
            license: s(src.license),
            order_ref: s(src.order_ref),
            purchased_at: s(src.purchased_at.map(|d| d.format("%Y-%m-%d").to_string())),
            notes: s(src.notes),
        }
    }

    fn source(&self) -> AssetSource {
        AssetSource {
            vendor: opt(self.vendor.get_untracked()),
            url: opt(self.url.get_untracked()),
            author: opt(self.author.get_untracked()),
            license: opt(self.license.get_untracked()),
            order_ref: opt(self.order_ref.get_untracked()),
            purchased_at: NaiveDate::parse_from_str(self.purchased_at.get_untracked().trim(), "%Y-%m-%d").ok(),
            notes: opt(self.notes.get_untracked()),
        }
    }
}

#[component]
fn FieldsView(f: Fields, creating: bool) -> impl IntoView {
    Effect::new(move |_| {
        let n = f.name.get();
        if !f.slug_touched.get_untracked() {
            f.slug.set(slugify(&n));
        }
    });
    view! {
        <div class="form-grid">
            <label class="field span-2">
                <span>"Nome"</span>
                <input type="text" required=true placeholder="Ex.: M1A2 Abrams" bind:value=f.name />
            </label>
            <Show when=move || creating>
                <label class="field">
                    <span>"Slug"</span>
                    <input
                        type="text"
                        class="mono"
                        prop:value=move || f.slug.get()
                        on:input=move |ev| {
                            f.slug_touched.set(true);
                            f.slug.set(event_target_value(&ev));
                        }
                    />
                </label>
            </Show>
            <label class="field">
                <span>"Tipo"</span>
                <select on:change=move |ev| {
                    if let Ok(k) = event_target_value(&ev).parse::<AssetKind>() {
                        f.kind.set(k);
                    }
                }>
                    {AssetKind::ALL
                        .iter()
                        .map(|k| {
                            let k = *k;
                            view! {
                                <option value=k.as_str() selected=move || f.kind.get() == k>
                                    {kind_label(k)}
                                </option>
                            }
                        })
                        .collect_view()}
                </select>
            </label>
            // Not a <label>: a click on it would be forwarded to the first chip's remove button.
            <div class="field span-2">
                <span>"Tags " <small class="muted">"(Enter, vírgula ou espaço para adicionar)"</small></span>
                <TagInput tags=f.tags placeholder="tanque, ww2, deserto" />
            </div>
            <label class="field span-2">
                <span>"Descrição"</span>
                <textarea rows="2" bind:value=f.description></textarea>
            </label>
        </div>
        <fieldset class="fieldset">
            <legend>"Origem / licença"</legend>
            <div class="form-grid">
                <label class="field">
                    <span>"Loja / fornecedor"</span>
                    <input type="text" list="vendors" placeholder="Fab, CGTrader…" bind:value=f.vendor />
                    <datalist id="vendors">
                        <option value="Fab"></option>
                        <option value="CGTrader"></option>
                        <option value="TurboSquid"></option>
                        <option value="Sketchfab"></option>
                        <option value="ArtStation"></option>
                        <option value="Unity Asset Store"></option>
                        <option value="interno"></option>
                    </datalist>
                </label>
                <label class="field">
                    <span>"Autor"</span>
                    <input type="text" bind:value=f.author />
                </label>
                <label class="field span-2">
                    <span>"URL do produto"</span>
                    <input type="url" placeholder="https://" bind:value=f.url />
                </label>
                <label class="field">
                    <span>"Licença"</span>
                    <input type="text" placeholder="Standard, Royalty-free, CC-BY…" bind:value=f.license />
                </label>
                <label class="field">
                    <span>"Pedido / nota fiscal"</span>
                    <input type="text" class="mono" bind:value=f.order_ref />
                </label>
                <label class="field">
                    <span>"Data da compra"</span>
                    <input type="date" bind:value=f.purchased_at />
                </label>
                <label class="field span-2">
                    <span>"Notas"</span>
                    <input type="text" bind:value=f.notes />
                </label>
            </div>
        </fieldset>
    }
}

/// Modal to create an asset in a project. Calls `on_created` with the new id.
#[component]
pub fn NewAssetModal(project_id: Uuid, on_close: Callback<()>, on_created: Callback<Uuid>) -> impl IntoView {
    let f = Fields::new(None);
    let error = RwSignal::new(None::<String>);
    let busy = RwSignal::new(false);
    let submit = move |ev: leptos::ev::SubmitEvent| {
        ev.prevent_default();
        busy.set(true);
        let req = CreateAssetRequest {
            name: f.name.get_untracked().trim().to_string(),
            slug: opt(f.slug.get_untracked()),
            kind: f.kind.get_untracked(),
            description: f.description.get_untracked(),
            tags: f.tags.get_untracked(),
            source: f.source(),
        };
        spawn_local(async move {
            match api::create_asset(project_id, req).await {
                Ok(a) => on_created.run(a.summary.id),
                Err(e) => {
                    busy.set(false);
                    error.set(Some(e.to_string()));
                }
            }
        });
    };
    view! {
        <Modal title="Novo asset" on_close=on_close wide=true>
            <form class="form" on:submit=submit>
                <FieldsView f=f creating=true />
                {move || error.get().map(|e| view! { <ErrorBanner error=e /> })}
                <div class="form-actions">
                    <button type="button" class="btn btn-ghost" on:click=move |_| on_close.run(())>
                        "Cancelar"
                    </button>
                    <button type="submit" class="btn btn-primary" disabled=move || busy.get()>
                        "Criar e enviar arquivos →"
                    </button>
                </div>
            </form>
        </Modal>
    }
}

/// Modal to edit an existing asset.
#[component]
pub fn EditAssetModal(asset: AssetDetail, on_close: Callback<()>, on_saved: Callback<AssetDetail>) -> impl IntoView {
    let f = Fields::new(Some(&asset));
    let aid = asset.summary.id;
    let error = RwSignal::new(None::<String>);
    let busy = RwSignal::new(false);
    let submit = move |ev: leptos::ev::SubmitEvent| {
        ev.prevent_default();
        busy.set(true);
        let req = UpdateAssetRequest {
            name: Some(f.name.get_untracked().trim().to_string()),
            description: Some(f.description.get_untracked()),
            kind: Some(f.kind.get_untracked()),
            tags: Some(f.tags.get_untracked()),
            source: Some(f.source()),
            head_version_id: None,
        };
        spawn_local(async move {
            match api::update_asset(aid, req).await {
                Ok(a) => on_saved.run(a),
                Err(e) => {
                    busy.set(false);
                    error.set(Some(e.to_string()));
                }
            }
        });
    };
    view! {
        <Modal title="Editar asset" on_close=on_close wide=true>
            <form class="form" on:submit=submit>
                <FieldsView f=f creating=false />
                {move || error.get().map(|e| view! { <ErrorBanner error=e /> })}
                <div class="form-actions">
                    <button type="button" class="btn btn-ghost" on:click=move |_| on_close.run(())>
                        "Cancelar"
                    </button>
                    <button type="submit" class="btn btn-primary" disabled=move || busy.get()>
                        "Salvar"
                    </button>
                </div>
            </form>
        </Modal>
    }
}
