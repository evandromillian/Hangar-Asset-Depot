//! "New version" dialog: pick files/folders, hash them in wasm (BLAKE3),
//! upload only blobs the server does not have, then commit the version.
//! On top of an existing version it adds or replaces files: a file named like
//! exactly one existing file replaces it in place, and a destination folder
//! places new content (e.g. `Animations/`) anywhere in the tree.

use crate::api;
use crate::components::common::{ErrorBanner, Modal};
use crate::upload::{self, PickedFile};
use crate::util::fmt_bytes;
use hangar_core::{CommitVersionRequest, FileEntry, VersionDto, paths};
use leptos::prelude::*;
use leptos::task::spawn_local;
use std::collections::{HashMap, HashSet};
use uuid::Uuid;

#[derive(Clone, PartialEq)]
enum Phase {
    Waiting,
    Hashing(f64),
    Hashed,
    Uploading,
    Uploaded,
    Deduplicated,
    Failed(String),
}

#[derive(Clone, PartialEq)]
struct RowState {
    phase: Phase,
    hash: Option<String>,
}

/// `\\a\\b/` → `a/b`.
fn clean_folder(s: &str) -> String {
    s.trim().replace('\\', "/").trim_matches('/').to_string()
}

/// Where a picked file goes in the new version, and whether that is an automatic
/// match with the only existing file of the same name (a corrected texture picked
/// on its own replaces `Texture1/…` instead of landing in the root).
fn target_of(picked: &str, dest: &str, by_name: &HashMap<String, Vec<String>>, keep_new: bool) -> (String, bool) {
    if !dest.is_empty() {
        return (format!("{dest}/{picked}"), false);
    }
    if !picked.contains('/')
        && !keep_new
        && let Some([only]) = by_name.get(&picked.to_lowercase()).map(Vec::as_slice)
    {
        return (only.clone(), true);
    }
    (picked.to_string(), false)
}

#[component]
pub fn NewVersionDialog(
    asset_id: Uuid,
    versions: Vec<VersionDto>,
    default_parent: Option<Uuid>,
    on_close: Callback<()>,
    on_committed: Callback<VersionDto>,
    #[prop(optional, into)] title: Option<String>,
) -> impl IntoView {
    let picked: RwSignal<Vec<PickedFile>, LocalStorage> = RwSignal::new_local(Vec::new());
    let rows = RwSignal::new(Vec::<RowState>::new());
    let parent = RwSignal::new(default_parent);
    let inherit = RwSignal::new(true);
    let message = RwSignal::new(String::new());
    let deletes = RwSignal::new(HashSet::<String>::new());
    let dragging = RwSignal::new(false);
    let busy = RwSignal::new(false);
    let error = RwSignal::new(None::<String>);
    let summary = RwSignal::new(None::<(f64, f64)>); // (uploaded, deduplicated)
    // Destination folder for picked files ("" = as picked).
    let dest = RwSignal::new(String::new());
    // Picked paths whose automatic "replace the file with this name" is undone.
    let keep_new = RwSignal::new(HashSet::<String>::new());
    let folder_input = NodeRef::<leptos::html::Input>::new();

    Effect::new(move |_| {
        if let Some(el) = folder_input.get() {
            el.set_webkitdirectory(true);
        }
    });

    let parent_files = LocalResource::new(move || {
        let p = parent.get();
        async move {
            match p {
                Some(p) => api::version_files(p).await,
                None => Ok(vec![]),
            }
        }
    });

    // Paths of the base version when its files are inherited.
    let existing = Memo::new(move |_| {
        if !inherit.get() || parent.get().is_none() {
            return Vec::<String>::new();
        }
        parent_files.get().and_then(|r| r.ok()).map(|l| l.into_iter().map(|f| f.path).collect()).unwrap_or_default()
    });
    let targets = Memo::new(move |_| {
        let mut by_name: HashMap<String, Vec<String>> = HashMap::new();
        for p in existing.get() {
            by_name.entry(paths::file_name(&p).to_lowercase()).or_default().push(p);
        }
        let dest = clean_folder(&dest.get());
        let opted = keep_new.get();
        picked.with(|l| l.iter().map(|f| target_of(&f.path, &dest, &by_name, opted.contains(&f.path))).collect::<Vec<_>>())
    });

    let add_files = move |mut files: Vec<PickedFile>| {
        // A dropped folder is normally the asset's own folder: drop its name. On top
        // of an existing tree keep it (`Animations/…`), unless stripping is what lines
        // the paths up with existing files (the asset folder dropped again).
        let tree: HashSet<String> = existing.get_untracked().into_iter().collect();
        let mut stripped = files.clone();
        upload::strip_common_top(&mut stripped);
        let hits = |l: &[PickedFile]| l.iter().filter(|f| tree.contains(&f.path)).count();
        if tree.is_empty() || hits(&stripped) > hits(&files) {
            files = stripped;
        }
        picked.update(|list| {
            for f in files {
                if let Some(existing) = list.iter_mut().find(|e| e.path == f.path) {
                    *existing = f;
                } else {
                    list.push(f);
                }
            }
            list.sort_by(|a, b| a.path.cmp(&b.path));
        });
        let n = picked.with_untracked(|l| l.len());
        rows.set(vec![RowState { phase: Phase::Waiting, hash: None }; n]);
        summary.set(None);
    };

    let on_pick = move |ev: leptos::ev::Event| {
        let input: web_sys::HtmlInputElement = event_target(&ev);
        if let Some(list) = input.files() {
            add_files(upload::from_file_list(&list));
        }
        input.set_value("");
    };

    let on_drop = move |ev: leptos::ev::DragEvent| {
        ev.prevent_default();
        dragging.set(false);
        let Some(dt) = ev.data_transfer() else { return };
        let entries = upload::drop_entries(&dt);
        if entries.is_empty() {
            if let Some(list) = dt.files() {
                add_files(upload::from_file_list(&list));
            }
            return;
        }
        spawn_local(async move {
            match upload::collect_entries(entries).await {
                Ok(files) => add_files(files),
                Err(e) => error.set(Some(format!("Não foi possível ler a pasta: {e}"))),
            }
        });
    };

    let total_bytes = move || picked.with(|l| l.iter().map(|f| f.size()).sum::<f64>());

    let set_phase = move |i: usize, phase: Phase| {
        rows.update(|r| {
            if let Some(row) = r.get_mut(i) {
                row.phase = phase;
            }
        })
    };

    let submit = move |ev: leptos::ev::SubmitEvent| {
        ev.prevent_default();
        if busy.get_untracked() {
            return;
        }
        let files = picked.get_untracked();
        let deletes_now: Vec<String> = deletes.get_untracked().into_iter().collect();
        if files.is_empty() && deletes_now.is_empty() {
            error.set(Some("Adicione arquivos ou marque algo para remover.".into()));
            return;
        }
        let msg = message.get_untracked().trim().to_string();
        if msg.is_empty() {
            error.set(Some("Descreva o que mudou nesta versão.".into()));
            return;
        }
        let target_paths: Vec<String> = targets.get_untracked().into_iter().map(|(t, _)| t).collect();
        let mut seen = HashSet::new();
        if let Some(dup) = target_paths.iter().find(|t| !seen.insert(t.to_lowercase())) {
            error.set(Some(format!("Dois arquivos iriam para o mesmo caminho: {dup}")));
            return;
        }
        busy.set(true);
        error.set(None);
        let parent_id = parent.get_untracked();
        let inherit_now = inherit.get_untracked() && parent_id.is_some();
        spawn_local(async move {
            let result: Result<VersionDto, String> = async {
                // 1. hash
                let mut hashes = Vec::with_capacity(files.len());
                for (i, f) in files.iter().enumerate() {
                    set_phase(i, Phase::Hashing(0.0));
                    let h = upload::hash_file(&f.file, |p| set_phase(i, Phase::Hashing(p)))
                        .await
                        .inspect_err(|e| set_phase(i, Phase::Failed(e.clone())))?;
                    rows.update(|r| {
                        if let Some(row) = r.get_mut(i) {
                            row.hash = Some(h.clone());
                            row.phase = Phase::Hashed;
                        }
                    });
                    hashes.push(h);
                }
                // 2. which blobs are missing on the server?
                let unique: Vec<String> = hashes.iter().cloned().collect::<HashSet<_>>().into_iter().collect();
                let missing: HashSet<String> = if unique.is_empty() {
                    HashSet::new()
                } else {
                    api::missing_blobs(unique).await.map_err(|e| e.to_string())?.into_iter().collect()
                };
                // 3. upload missing blobs once each
                let mut uploaded: HashMap<String, bool> = HashMap::new();
                let (mut up_bytes, mut dedup_bytes) = (0.0, 0.0);
                for (i, f) in files.iter().enumerate() {
                    let h = &hashes[i];
                    if !missing.contains(h) || uploaded.contains_key(h) {
                        set_phase(i, Phase::Deduplicated);
                        dedup_bytes += f.size();
                        continue;
                    }
                    set_phase(i, Phase::Uploading);
                    match api::put_blob(h, &f.file).await {
                        Ok(_) => {
                            uploaded.insert(h.clone(), true);
                            up_bytes += f.size();
                            set_phase(i, Phase::Uploaded);
                        }
                        Err(e) => {
                            set_phase(i, Phase::Failed(e.to_string()));
                            return Err(format!("{}: {e}", f.path));
                        }
                    }
                }
                summary.set(Some((up_bytes, dedup_bytes)));
                // 4. commit
                let req = CommitVersionRequest {
                    parent_id,
                    message: msg,
                    inherit: inherit_now,
                    upserts: files
                        .iter()
                        .zip(hashes.iter().zip(&target_paths))
                        .map(|(f, (h, path))| FileEntry { path: path.clone(), hash: h.clone(), size: f.size() as i64 })
                        .collect(),
                    deletes: if inherit_now { deletes_now } else { vec![] },
                    set_head: true,
                };
                api::commit_version(asset_id, &req).await.map_err(|e| e.to_string())
            }
            .await;
            busy.set(false);
            match result {
                Ok(v) => on_committed.run(v),
                Err(e) => error.set(Some(e)),
            }
        });
    };

    let version_options = versions.clone();

    view! {
        <Modal title=title.unwrap_or_else(|| "Nova versão".to_string()) on_close=on_close wide=true>
            <form class="form upload" on:submit=submit>
                <div class="form-grid">
                    <label class="field">
                        <span>"Baseada em"</span>
                        <select on:change=move |ev| parent.set(Uuid::parse_str(&event_target_value(&ev)).ok())>
                            <option value="" selected=move || parent.get().is_none()>"— nenhuma (árvore vazia)"</option>
                            {version_options
                                .iter()
                                .map(|v| {
                                    let id = v.id;
                                    view! {
                                        <option value=id.to_string() selected=move || parent.get() == Some(id)>
                                            {format!(
                                                "v{}{} — {}",
                                                v.number,
                                                if v.is_head { " (HEAD)" } else { "" },
                                                v.message,
                                            )}
                                        </option>
                                    }
                                })
                                .collect_view()}
                        </select>
                    </label>
                    <label class="check field-check">
                        <input
                            type="checkbox"
                            prop:checked=move || inherit.get()
                            prop:disabled=move || parent.get().is_none()
                            on:change=move |ev| inherit.set(event_target_checked(&ev))
                        />
                        "Herdar arquivos da versão base (envie só o que mudou)"
                    </label>
                    <label class="field span-2">
                        <span>"Pasta de destino"</span>
                        <input
                            type="text"
                            class="mono"
                            list="dest-folders"
                            placeholder="como escolhido (raiz da versão); ex.: Animations"
                            bind:value=dest
                        />
                        <datalist id="dest-folders">
                            {move || {
                                let mut folders: Vec<String> = existing
                                    .get()
                                    .iter()
                                    .map(|p| paths::parent(p).to_string())
                                    // `.hangar/` holds app settings, not content.
                                    .filter(|p| !p.is_empty() && !p.starts_with('.'))
                                    .collect();
                                folders.sort();
                                folders.dedup();
                                folders.into_iter().map(|f| view! { <option value=f.clone()></option> }).collect_view()
                            }}
                        </datalist>
                    </label>
                    <label class="field span-2">
                        <span>"Mensagem"</span>
                        <textarea
                            rows="2"
                            required=true
                            placeholder="Ex.: preenche a parte inferior do casco visível de baixo"
                            bind:value=message
                        ></textarea>
                    </label>
                </div>

                <div
                    class="dropzone"
                    class:dragging=move || dragging.get()
                    on:dragover=move |ev: leptos::ev::DragEvent| {
                        ev.prevent_default();
                        dragging.set(true);
                    }
                    on:dragleave=move |_| dragging.set(false)
                    on:drop=on_drop
                >
                    <div class="hazard thin"></div>
                    <p>
                        <b>"Arraste arquivos ou pastas aqui"</b>
                        <br />
                        <span class="muted">"FBX, glTF/GLB, OBJ, .blend, texturas (PNG/JPG/TGA…), áudio"</span>
                    </p>
                    <div class="dz-actions">
                        <label class="btn btn-sm">
                            "Escolher arquivos"
                            <input type="file" multiple=true class="visually-hidden" on:change=on_pick />
                        </label>
                        <label class="btn btn-sm">
                            "Escolher pasta"
                            <input type="file" multiple=true class="visually-hidden" node_ref=folder_input on:change=on_pick />
                        </label>
                    </div>
                </div>

                {move || {
                    let files = picked.get();
                    if files.is_empty() {
                        return ().into_any();
                    }
                    let existing: HashSet<String> = existing.get().into_iter().collect();
                    let targets = targets.get();
                    let opted = keep_new.get();
                    let states = rows.get();
                    view! {
                        <div class="upload-list-head">
                            <b>{format!("{} arquivos", files.len())}</b>
                            <span class="muted mono">{fmt_bytes(total_bytes() as i64)}</span>
                            <button
                                type="button"
                                class="btn btn-ghost btn-sm push-right"
                                disabled=move || busy.get()
                                on:click=move |_| {
                                    picked.set(vec![]);
                                    rows.set(vec![]);
                                }
                            >
                                "Limpar"
                            </button>
                        </div>
                        <ul class="upload-list">
                            {files
                                .into_iter()
                                .enumerate()
                                .map(|(i, f)| {
                                    let state = states.get(i).cloned();
                                    let (target, auto) = targets.get(i).cloned().unwrap_or_else(|| (f.path.clone(), false));
                                    let replaces = existing.contains(&target);
                                    // The automatic match can be undone (and redone) per file.
                                    let toggle = (auto || opted.contains(&f.path)).then(|| {
                                        let key = f.path.clone();
                                        let label = if auto { "manter como novo" } else { "substituir o existente" };
                                        view! {
                                            <button
                                                type="button"
                                                class="link-btn up-toggle"
                                                disabled=move || busy.get()
                                                on:click=move |_| keep_new.update(|k| {
                                                    if !k.remove(&key) {
                                                        k.insert(key.clone());
                                                    }
                                                })
                                            >
                                                {label}
                                            </button>
                                        }
                                    });
                                    let shown = if target == f.path { target.clone() } else { format!("{} → {target}", f.path) };
                                    let shown_title = shown.clone();
                                    let (label, cls, pct) = match state.map(|s| s.phase) {
                                        None | Some(Phase::Waiting) => ("aguardando".to_string(), "", 0.0),
                                        Some(Phase::Hashing(p)) => (format!("hash {:.0}%", p * 100.0), "busy", p),
                                        Some(Phase::Hashed) => ("hash ok".into(), "", 1.0),
                                        Some(Phase::Uploading) => ("enviando…".into(), "busy", 1.0),
                                        Some(Phase::Uploaded) => ("enviado".into(), "ok", 1.0),
                                        Some(Phase::Deduplicated) => ("já no servidor".into(), "dedup", 1.0),
                                        Some(Phase::Failed(e)) => (format!("erro: {e}"), "err", 1.0),
                                    };
                                    let path = f.path.clone();
                                    view! {
                                        <li class=format!("up-row {cls}")>
                                            <span class="up-badge mono" class:replace=replaces>
                                                {if replaces { "subst." } else { "novo" }}
                                            </span>
                                            <span class="up-path mono" title=shown_title>
                                                <span class="up-path-text">{shown}</span>
                                                {toggle}
                                            </span>
                                            <span class="up-size mono muted">{fmt_bytes(f.size() as i64)}</span>
                                            <span class="up-state">{label}</span>
                                            <span class="up-bar"><i style=format!("width:{:.0}%", pct * 100.0)></i></span>
                                            <button
                                                type="button"
                                                class="icon-btn"
                                                title="Remover da lista"
                                                disabled=move || busy.get()
                                                on:click=move |_| {
                                                    picked.update(|l| l.retain(|x| x.path != path));
                                                    rows.update(|r| {
                                                        if i < r.len() {
                                                            r.remove(i);
                                                        }
                                                    });
                                                }
                                            >
                                                "✕"
                                            </button>
                                        </li>
                                    }
                                })
                                .collect_view()}
                        </ul>
                    }
                        .into_any()
                }}

                <Show when=move || inherit.get() && parent.get().is_some()>
                    <details class="inherited">
                        <summary>
                            "Arquivos herdados "
                            <span class="muted">"(marque para remover nesta versão)"</span>
                            {move || {
                                let n = deletes.with(|d| d.len());
                                (n > 0).then(|| view! { <span class="pill pill-danger">{format!("{n} a remover")}</span> })
                            }}
                        </summary>
                        {move || match parent_files.get() {
                            None => view! { <span class="spinner"></span> }.into_any(),
                            Some(Err(e)) => view! { <ErrorBanner error=e.to_string() /> }.into_any(),
                            Some(Ok(list)) => {
                                let incoming: HashSet<String> = targets.get().into_iter().map(|(t, _)| t).collect();
                                view! {
                                    <ul class="inherit-list">
                                        {list
                                            .into_iter()
                                            .filter(|f| !incoming.contains(&f.path))
                                            .map(|f| {
                                                let p = f.path.clone();
                                                let p2 = f.path.clone();
                                                view! {
                                                    <li>
                                                        <label class="check">
                                                            <input
                                                                type="checkbox"
                                                                prop:checked=move || deletes.with(|d| d.contains(&p))
                                                                on:change=move |ev| {
                                                                    let on = event_target_checked(&ev);
                                                                    deletes.update(|d| {
                                                                        if on {
                                                                            d.insert(p2.clone());
                                                                        } else {
                                                                            d.remove(&p2);
                                                                        }
                                                                    });
                                                                }
                                                            />
                                                            <span class="mono">{f.path.clone()}</span>
                                                        </label>
                                                        <span class="muted mono">{fmt_bytes(f.size)}</span>
                                                    </li>
                                                }
                                            })
                                            .collect_view()}
                                    </ul>
                                }
                                    .into_any()
                            }
                        }}
                    </details>
                </Show>

                {move || {
                    summary
                        .get()
                        .map(|(up, dedup)| {
                            view! {
                                <div class="banner banner-ok mono">
                                    {format!(
                                        "Enviado {} · reaproveitado {} (deduplicação)",
                                        fmt_bytes(up as i64),
                                        fmt_bytes(dedup as i64),
                                    )}
                                </div>
                            }
                        })
                }}
                {move || error.get().map(|e| view! { <ErrorBanner error=e /> })}
                <div class="form-actions">
                    <button type="button" class="btn btn-ghost" on:click=move |_| on_close.run(())>
                        "Cancelar"
                    </button>
                    <button type="submit" class="btn btn-primary" disabled=move || busy.get()>
                        {move || if busy.get() { "Enviando…" } else { "Criar versão" }}
                    </button>
                </div>
            </form>
        </Modal>
    }
}
