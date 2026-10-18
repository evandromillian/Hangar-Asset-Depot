//! File tree of the selected version with diff markers against another version.

use crate::api;
use crate::components::common::ErrorBanner;
use crate::util::{fmt_bytes, short_hash};
use hangar_core::{VersionDiff, VersionDto, paths};
use leptos::prelude::*;
use std::collections::HashMap;
use uuid::Uuid;

#[derive(Clone, Copy, PartialEq)]
enum Change {
    Same,
    Added,
    Modified,
}

#[component]
pub fn FilesPanel(
    version: Signal<Option<VersionDto>>,
    versions: Signal<Vec<VersionDto>>,
    can_write: Signal<bool>,
    /// Opens the upload dialog to add / replace files on top of this version.
    on_add: Callback<()>,
) -> impl IntoView {
    let vid = Memo::new(move |_| version.get().map(|v| v.id));
    let parent = Memo::new(move |_| version.get().and_then(|v| v.parent_id));
    // None = compare with the parent.
    let against = RwSignal::new(None::<Uuid>);
    let only_changes = RwSignal::new(false);
    Effect::new(move |_| {
        vid.track();
        against.set(None);
    });

    let files = LocalResource::new(move || {
        let vid = vid.get();
        async move {
            match vid {
                Some(v) => api::version_files(v).await,
                None => Ok(vec![]),
            }
        }
    });
    let diff = LocalResource::new(move || {
        let vid = vid.get();
        let against = against.get();
        let has_base = against.is_some() || parent.get().is_some();
        async move {
            match vid {
                Some(v) if has_base => api::version_diff(v, against).await.map(Some),
                _ => Ok(None),
            }
        }
    });

    view! {
        <div class="files">
            <div class="files-head">
                <label class="compact-field">
                    <span>"Comparar com"</span>
                    <select on:change=move |ev| {
                        let v = event_target_value(&ev);
                        against.set(Uuid::parse_str(&v).ok());
                    }>
                        <option value="" selected=move || against.get().is_none()>
                            {move || {
                                let p = parent.get();
                                let n = p.and_then(|p| versions.get().iter().find(|v| v.id == p).map(|v| v.number));
                                match n {
                                    Some(n) => format!("pai (v{n})"),
                                    None => "— (versão raiz)".to_string(),
                                }
                            }}
                        </option>
                        {move || {
                            let current = vid.get();
                            versions
                                .get()
                                .into_iter()
                                .filter(|v| Some(v.id) != current)
                                .map(|v| {
                                    let id = v.id;
                                    view! {
                                        <option value=id.to_string() selected=move || against.get() == Some(id)>
                                            {format!("v{} — {}", v.number, v.message)}
                                        </option>
                                    }
                                })
                                .collect_view()
                        }}
                    </select>
                </label>
                <label class="check">
                    <input type="checkbox" bind:checked=only_changes />
                    "só alterações"
                </label>
                <Show when=move || can_write.get() && vid.get().is_some()>
                    <button
                        class="btn btn-sm btn-primary push-right"
                        title="Envia texturas corrigidas, animações e outros arquivos sobre esta versão"
                        on:click=move |_| on_add.run(())
                    >
                        "+ Adicionar arquivos"
                    </button>
                </Show>
                {move || {
                    vid.get()
                        .map(|v| {
                            view! {
                                <a class="btn btn-sm" class:push-right=move || !can_write.get() href=api::archive_url(v) download="">
                                    "⇩ .zip"
                                </a>
                            }
                        })
                }}
            </div>
            {move || {
                let d: Option<VersionDiff> = diff.get().and_then(|r| r.ok()).flatten();
                d.map(|d| {
                    view! {
                        <div class="diff-summary mono">
                            <span class="d-add">{format!("+{}", d.added.len())}</span>
                            <span class="d-mod">{format!("~{}", d.modified.len())}</span>
                            <span class="d-del">{format!("−{}", d.removed.len())}</span>
                            <span class="muted">{format!("{} iguais", d.unchanged)}</span>
                        </div>
                    }
                })
            }}
            {move || match files.get() {
                None => view! { <span class="spinner"></span> }.into_any(),
                Some(Err(e)) => view! { <ErrorBanner error=e.to_string() /> }.into_any(),
                Some(Ok(list)) => {
                    let Some(v) = vid.get() else { return ().into_any() };
                    let d: Option<VersionDiff> = diff.get().and_then(|r| r.ok()).flatten();
                    let mut changes: HashMap<String, Change> = HashMap::new();
                    let mut removed = vec![];
                    if let Some(d) = &d {
                        for f in &d.added {
                            changes.insert(f.path.clone(), Change::Added);
                        }
                        for m in &d.modified {
                            changes.insert(m.path.clone(), Change::Modified);
                        }
                        removed = d.removed.clone();
                    }
                    let only = only_changes.get();
                    let rows = list
                        .into_iter()
                        .filter_map(|f| {
                            let c = changes.get(&f.path).copied().unwrap_or(Change::Same);
                            if only && c == Change::Same {
                                return None;
                            }
                            let dir = paths::parent(&f.path).to_string();
                            let name = paths::file_name(&f.path).to_string();
                            let (mark, cls) = match c {
                                Change::Added => ("A", "row-add"),
                                Change::Modified => ("M", "row-mod"),
                                Change::Same => ("", ""),
                            };
                            Some(view! {
                                <li class=format!("file-row {cls}")>
                                    <span class="file-mark mono">{mark}</span>
                                    <span class="file-path">
                                        {(!dir.is_empty()).then(|| view! { <span class="file-dir">{format!("{dir}/")}</span> })}
                                        <span class="file-name">{name}</span>
                                    </span>
                                    <span class="file-size mono muted">{fmt_bytes(f.size)}</span>
                                    <span class="file-hash mono muted" title=f.hash.clone()>{short_hash(&f.hash).to_string()}</span>
                                    <a class="icon-btn" title="Baixar" href=api::raw_file_url(v, &f.path, true)>"⇩"</a>
                                </li>
                            })
                        })
                        .collect_view();
                    view! {
                        <ul class="file-list">
                            {rows}
                            {removed
                                .into_iter()
                                .map(|f| {
                                    view! {
                                        <li class="file-row row-del">
                                            <span class="file-mark mono">"D"</span>
                                            <span class="file-path"><s>{f.path.clone()}</s></span>
                                            <span class="file-size mono muted">{fmt_bytes(f.size)}</span>
                                            <span class="file-hash mono muted">{short_hash(&f.hash).to_string()}</span>
                                            <span></span>
                                        </li>
                                    }
                                })
                                .collect_view()}
                        </ul>
                    }
                        .into_any()
                }
            }}
        </div>
    }
}
