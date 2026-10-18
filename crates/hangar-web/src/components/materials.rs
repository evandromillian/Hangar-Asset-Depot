//! Materials tab: assign the version's textures to material slots by hand and
//! author skins. Edits are previewed live in the viewer (as a draft) and saved
//! as `.hangar/materials.json` in a new version, which the pipeline bakes.
//! Per-slot checkboxes hide textures in the preview only; they are never saved.

use crate::api;
use crate::components::common::ErrorBanner;
use crate::viewer::{HiddenTexture, ViewerDraft};
use hangar_core::{
    CommitVersionRequest, EDITABLE_SLOTS, MATERIAL_CONFIG_PATH, MaterialConfig, MaterialInfo, PreviewKind,
    PreviewManifest, SkinConfig, TextureOverride, TextureSlot, VersionDto, VersionStatus, blob_url, paths,
};
use leptos::prelude::*;
use leptos::task::spawn_local;
use std::collections::{BTreeMap, HashMap, HashSet};
use uuid::Uuid;

/// Which look the slot selects edit.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Target {
    Base,
    Skin(usize),
}

/// Select value for "automatic" (base) / "inherit from base" (skin).
const AUTO: &str = "";
/// Select value forcing an empty slot. Stored paths are normalised and never
/// start with `/`, so this cannot collide with a file.
const NONE: &str = "/none";

fn editable_model(v: &VersionDto) -> Option<&PreviewManifest> {
    v.preview
        .as_ref()
        .filter(|p| v.status == VersionStatus::Ready && p.kind == Some(PreviewKind::Model) && !p.materials.is_empty())
}

/// Texture currently baked into the GLB for a slot.
fn baked(m: &MaterialInfo, slot: TextureSlot) -> Option<&str> {
    m.textures.iter().find(|t| t.slot == slot).and_then(|t| t.path.as_deref())
}

/// The config key used for `material` (keys match case-insensitively).
fn material_key(map_keys: impl Iterator<Item = String>, material: &str) -> String {
    let mut keys = map_keys;
    keys.find(|k| k.eq_ignore_ascii_case(material)).unwrap_or_else(|| material.to_string())
}

/// Drops empty entries so "nothing set" compares equal to a missing file.
fn pruned(cfg: &MaterialConfig) -> MaterialConfig {
    let mut c = cfg.clone();
    c.materials.retain(|_, slots| !slots.is_empty());
    for s in &mut c.skins {
        s.name = s.name.trim().to_string();
        s.materials.retain(|_, slots| !slots.is_empty());
    }
    c.skins.retain(|s| !s.materials.is_empty());
    c
}

/// Texture a slot shows for the edited target: skin override, else the base
/// assignment (`None` when forced empty), else what is baked into the GLB.
fn effective(m: &MaterialInfo, cfg: &MaterialConfig, target: Target, slot: TextureSlot) -> Option<String> {
    if let Target::Skin(i) = target
        && let Some(path) = cfg
            .skins
            .get(i)
            .and_then(|s| s.materials.iter().find(|(k, _)| k.eq_ignore_ascii_case(&m.name)))
            .and_then(|(_, slots)| slots.get(&slot))
    {
        return Some(path.clone());
    }
    match cfg.assignments(&m.name).and_then(|a| a.get(&slot)) {
        Some(v) => v.clone(),
        None => baked(m, slot).map(str::to_string),
    }
}

/// Material + slot switched off in the preview.
type HiddenSet = HashSet<(String, TextureSlot)>;

/// What the viewer should show for the edited target: base assignments that
/// differ from the baked textures, the skin's overrides, and hidden textures.
fn compute_draft(p: &PreviewManifest, cfg: &MaterialConfig, target: Target, hidden: &HiddenSet) -> Option<ViewerDraft> {
    let previews: HashMap<&str, &str> =
        p.textures.iter().filter_map(|t| t.preview_hash.as_deref().map(|h| (t.path.as_str(), h))).collect();
    let mut out = Vec::new();
    let mut hidden_out = Vec::new();
    for m in &p.materials {
        let is_hidden = |slot: TextureSlot| hidden.contains(&(m.name.clone(), slot));
        let mut slots: BTreeMap<TextureSlot, String> = BTreeMap::new();
        if let Some(a) = cfg.assignments(&m.name) {
            for (&slot, path) in a {
                if let Some(path) = path.as_deref().filter(|path| baked(m, slot) != Some(path)) {
                    slots.insert(slot, path.to_string());
                }
            }
        }
        if let Target::Skin(i) = target
            && let Some(skin) = cfg.skins.get(i)
            && let Some((_, ms)) = skin.materials.iter().find(|(k, _)| k.eq_ignore_ascii_case(&m.name))
        {
            slots.extend(ms.iter().map(|(s, p)| (*s, p.clone())));
        }
        // ORM / metal-rough / roughness / metallic are baked into one metal/rough
        // texture. Hiding any of them makes the viewer drop it and rebuild from
        // the visible maps (same priority as the GLB export), so send them all.
        let family = [TextureSlot::Orm, TextureSlot::MetallicRoughness, TextureSlot::Roughness, TextureSlot::Metallic];
        if family.iter().any(|s| is_hidden(*s)) {
            for slot in family.into_iter().chain([TextureSlot::Occlusion]) {
                if !is_hidden(slot)
                    && let Some(path) = effective(m, cfg, target, slot)
                {
                    slots.entry(slot).or_insert(path);
                }
            }
        }
        for (slot, path) in slots {
            if is_hidden(slot) {
                continue;
            }
            if let Some(h) = previews.get(path.as_str()) {
                out.push(TextureOverride { material: m.name.clone(), slot, path, preview_hash: h.to_string() });
            }
        }
        for &slot in EDITABLE_SLOTS {
            if is_hidden(slot) && effective(m, cfg, target, slot).is_some() {
                hidden_out.push(HiddenTexture {
                    material: m.name.clone(),
                    slot,
                    alpha: m.base_color[3],
                    metallic: m.metallic,
                    roughness: m.roughness,
                });
            }
        }
    }
    (target != Target::Base || !out.is_empty() || !hidden_out.is_empty())
        .then_some(ViewerDraft { overrides: out, hidden: hidden_out })
}

#[component]
pub fn MaterialsPanel(
    version: Signal<Option<VersionDto>>,
    asset_id: Signal<Uuid>,
    can_write: Signal<bool>,
    /// Shared with the viewer: unsaved assignments and hidden textures to preview.
    draft: RwSignal<Option<ViewerDraft>>,
    on_committed: Callback<VersionDto>,
) -> impl IntoView {
    let preview = Memo::new(move |_| version.with(|v| v.as_ref().and_then(editable_model).cloned()));
    let vid = Memo::new(move |_| version.with(|v| v.as_ref().filter(|v| editable_model(v).is_some()).map(|v| v.id)));

    // None while loading; Some(config) once the version's file (or its absence) is known.
    let baseline = RwSignal::new(None::<MaterialConfig>);
    let had_file = RwSignal::new(false);
    let edit = RwSignal::new(MaterialConfig::default());
    let target = RwSignal::new(Target::Base);
    let open = RwSignal::new(HashSet::<String>::new());
    // Preview-only toggles: never part of the saved configuration.
    let hidden = RwSignal::new(HiddenSet::new());
    let filter = RwSignal::new(String::new());
    let message = RwSignal::new(String::new());
    let saving = RwSignal::new(false);
    let error = RwSignal::new(None::<String>);

    Effect::new(move |_| {
        let Some(v) = vid.get() else {
            baseline.set(None);
            return;
        };
        baseline.set(None);
        error.set(None);
        target.set(Target::Base);
        hidden.set(HiddenSet::new());
        spawn_local(async move {
            let res = api::material_config(v).await;
            if vid.get_untracked() != Some(v) {
                return;
            }
            let cfg = match res {
                Ok(c) => {
                    had_file.set(c.is_some());
                    c.unwrap_or_default()
                }
                Err(e) => {
                    error.set(Some(e.to_string()));
                    had_file.set(true);
                    MaterialConfig::default()
                }
            };
            edit.set(cfg.clone());
            baseline.set(Some(cfg));
            // Few materials: show them all; many: start collapsed.
            let names = preview.get_untracked().map(|p| p.materials.iter().map(|m| m.name.clone()).collect());
            open.set(names.filter(|n: &HashSet<String>| n.len() <= 3).unwrap_or_default());
        });
    });

    // Keep the viewer's draft in sync with the edits.
    Effect::new(move |_| {
        let d = match (preview.get(), baseline.with(|b| b.is_some())) {
            (Some(p), true) => edit.with(|cfg| hidden.with(|h| compute_draft(&p, cfg, target.get(), h))),
            _ => None,
        };
        if draft.get_untracked() != d {
            draft.set(d);
        }
    });
    on_cleanup(move || draft.set(None));

    let dirty = Memo::new(move |_| baseline.with(|b| b.as_ref().map(pruned)) != Some(edit.with(pruned)));
    let skin_names = Memo::new(move |_| edit.with(|c| c.skins.iter().map(|s| s.name.clone()).collect::<Vec<_>>()));

    let set_slot = move |material: String, slot: TextureSlot, value: String| {
        edit.update(|cfg| match target.get_untracked() {
            Target::Base => {
                let key = material_key(cfg.materials.keys().cloned(), &material);
                let slots = cfg.materials.entry(key.clone()).or_default();
                match value.as_str() {
                    AUTO => {
                        slots.remove(&slot);
                    }
                    NONE => {
                        slots.insert(slot, None);
                    }
                    path => {
                        slots.insert(slot, Some(path.to_string()));
                    }
                }
                if slots.is_empty() {
                    cfg.materials.remove(&key);
                }
            }
            Target::Skin(i) => {
                let Some(skin) = cfg.skins.get_mut(i) else { return };
                let key = material_key(skin.materials.keys().cloned(), &material);
                let slots = skin.materials.entry(key.clone()).or_default();
                if value.is_empty() || value == NONE {
                    slots.remove(&slot);
                } else {
                    slots.insert(slot, value);
                }
                if slots.is_empty() {
                    skin.materials.remove(&key);
                }
            }
        });
    };

    let reset_material = move |material: String| {
        edit.update(|cfg| match target.get_untracked() {
            Target::Base => cfg.materials.retain(|k, _| !k.eq_ignore_ascii_case(&material)),
            Target::Skin(i) => {
                if let Some(s) = cfg.skins.get_mut(i) {
                    s.materials.retain(|k, _| !k.eq_ignore_ascii_case(&material));
                }
            }
        });
    };

    let add_skin = move |_| {
        edit.update(|cfg| {
            let mut n = cfg.skins.len() + 1;
            while cfg.skins.iter().any(|s| s.name.eq_ignore_ascii_case(&format!("Skin {n}"))) {
                n += 1;
            }
            cfg.skins.push(SkinConfig { name: format!("Skin {n}"), materials: BTreeMap::new() });
            target.set(Target::Skin(cfg.skins.len() - 1));
        });
    };

    let delete_skin = move |_| {
        if let Target::Skin(i) = target.get_untracked() {
            target.set(Target::Base);
            edit.update(|cfg| {
                if i < cfg.skins.len() {
                    cfg.skins.remove(i);
                }
            });
        }
    };

    let discard = move |_| {
        if let Some(b) = baseline.get_untracked() {
            target.set(Target::Base);
            edit.set(b);
        }
    };

    let save = move |_| {
        let Some(v) = version.get_untracked() else { return };
        let cfg = edit.with_untracked(pruned);
        // Skin names must be unique and not shadow the default look.
        let default_name = preview.get_untracked().and_then(|p| p.variants.into_iter().find(|v| v.is_default)).map(|v| v.name);
        let mut seen = HashSet::new();
        for s in &cfg.skins {
            let lower = s.name.to_lowercase();
            if s.name.is_empty() || !seen.insert(lower.clone()) || default_name.as_deref().map(str::to_lowercase) == Some(lower) {
                error.set(Some(format!("Nome de skin inválido ou repetido: \"{}\"", s.name)));
                return;
            }
        }
        let msg = message.get_untracked().trim().to_string();
        let msg = if msg.is_empty() { "Materiais: texturas atribuídas manualmente".to_string() } else { msg };
        let aid = asset_id.get_untracked();
        let remove_file = cfg.is_empty() && had_file.get_untracked();
        saving.set(true);
        error.set(None);
        spawn_local(async move {
            let res = async {
                let (upserts, deletes) = if remove_file {
                    (vec![], vec![MATERIAL_CONFIG_PATH.to_string()])
                } else {
                    (vec![api::upload_bytes(MATERIAL_CONFIG_PATH, &cfg.to_json()).await?], vec![])
                };
                let req = CommitVersionRequest {
                    parent_id: Some(v.id),
                    message: msg,
                    inherit: true,
                    upserts,
                    deletes,
                    set_head: v.is_head,
                };
                api::commit_version(aid, &req).await
            }
            .await;
            saving.set(false);
            match res {
                Ok(nv) => {
                    message.set(String::new());
                    draft.set(None);
                    on_committed.run(nv);
                }
                Err(e) => error.set(Some(e.to_string())),
            }
        });
    };

    // Material cards: re-rendered on every edit (selects keep no state of their own).
    let cards = move || {
        let p = preview.get()?;
        let cfg = edit.get();
        let t = target.get();
        let opened = open.get();
        let off = hidden.get();
        let needle = filter.get().to_lowercase();
        let writable = can_write.get() && !saving.get();
        let previews: HashMap<String, String> =
            p.textures.iter().filter_map(|t| t.preview_hash.clone().map(|h| (t.path.clone(), h))).collect();
        let usable: Vec<(String, Option<TextureSlot>)> =
            p.textures.iter().filter(|t| t.preview_hash.is_some()).map(|t| (t.path.clone(), t.slot)).collect();
        let slots: Vec<TextureSlot> =
            EDITABLE_SLOTS.iter().copied().filter(|s| !(matches!(t, Target::Skin(_)) && *s == TextureSlot::Opacity)).collect();

        let shown: Vec<&MaterialInfo> =
            p.materials.iter().filter(|m| needle.is_empty() || m.name.to_lowercase().contains(&needle)).collect();
        if shown.is_empty() {
            return Some(view! { <p class="muted">"Nenhum material com esse nome."</p> }.into_any());
        }
        let list = shown
            .into_iter()
            .map(|m| {
                let name = m.name.clone();
                let base = cfg.assignments(&m.name).cloned().unwrap_or_default();
                let skin = match t {
                    Target::Skin(i) => cfg
                        .skins
                        .get(i)
                        .and_then(|s| s.materials.iter().find(|(k, _)| k.eq_ignore_ascii_case(&m.name)))
                        .map(|(_, v)| v.clone())
                        .unwrap_or_default(),
                    Target::Base => BTreeMap::new(),
                };
                let effective = |slot: TextureSlot| effective(m, &cfg, t, slot);
                let edits = if t == Target::Base { base.len() } else { skin.len() };
                let hidden_count =
                    off.iter().filter(|(mat, slot)| *mat == m.name && effective(*slot).is_some()).count();
                let is_open = opened.contains(&name);
                let thumbs: Vec<String> = [TextureSlot::BaseColor, TextureSlot::Normal]
                    .into_iter()
                    .filter_map(|s| effective(s).and_then(|p| previews.get(&p).cloned()))
                    .collect();
                let rows = is_open.then(|| {
                    slots
                        .iter()
                        .map(|&slot| {
                            let current = effective(slot);
                            let selected: String = match t {
                                Target::Base => match base.get(&slot) {
                                    Some(Some(p)) => p.clone(),
                                    Some(None) => NONE.to_string(),
                                    None => AUTO.to_string(),
                                },
                                Target::Skin(_) => skin.get(&slot).cloned().unwrap_or_default(),
                            };
                            let auto_label = match t {
                                Target::Base => {
                                    let auto = m
                                        .textures
                                        .iter()
                                        .find(|x| x.slot == slot && x.resolved_by != "manual")
                                        .and_then(|x| x.path.as_deref());
                                    match auto {
                                        Some(p) => format!("Automático · {}", paths::file_name(p)),
                                        None => "Automático".to_string(),
                                    }
                                }
                                Target::Skin(_) => match base.get(&slot).cloned().unwrap_or_else(|| baked(m, slot).map(str::to_string)) {
                                    Some(p) => format!("Herdar da base · {}", paths::file_name(&p)),
                                    None => "Herdar da base · vazia".to_string(),
                                },
                            };
                            let option = |path: &String| {
                                let label = path.clone();
                                view! { <option value=path.clone() selected=selected == *path>{label}</option> }
                            };
                            let suggested: Vec<_> = usable.iter().filter(|(_, s)| *s == Some(slot)).map(|(p, _)| option(p)).collect();
                            let others: Vec<_> = usable.iter().filter(|(_, s)| *s != Some(slot)).map(|(p, _)| option(p)).collect();
                            let thumb = current.as_ref().and_then(|p| previews.get(p)).map(|h| blob_url(h));
                            let title = current.clone().unwrap_or_else(|| "sem textura".into());
                            let mat = name.clone();
                            let key = (name.clone(), slot);
                            let is_off = off.contains(&key);
                            view! {
                                <div class="mat-slot-row" class:edited=!selected.is_empty() class:off=is_off>
                                    <input
                                        type="checkbox"
                                        title="Mostrar esta textura na pré-visualização (não é salvo)"
                                        prop:checked=current.is_some() && !is_off
                                        disabled=current.is_none()
                                        on:change=move |_| hidden.update(|h| {
                                            if !h.remove(&key) {
                                                h.insert(key.clone());
                                            }
                                        })
                                    />
                                    <span class="slot-name">{slot.label()}</span>
                                    <span class="mat-thumb checker" title=title>
                                        {thumb.map(|src| view! { <img src=src alt="" loading="lazy" /> })}
                                    </span>
                                    <select
                                        disabled=!writable
                                        on:change=move |ev| set_slot(mat.clone(), slot, event_target_value(&ev))
                                    >
                                        <option value=AUTO selected=selected.is_empty()>{auto_label}</option>
                                        {(t == Target::Base).then(|| view! {
                                            <option value=NONE selected=selected == NONE>"Nenhuma (deixar vazia)"</option>
                                        })}
                                        {(!suggested.is_empty()).then(|| view! { <optgroup label="Sugeridas">{suggested}</optgroup> })}
                                        {(!others.is_empty()).then(|| view! { <optgroup label="Outras texturas">{others}</optgroup> })}
                                    </select>
                                </div>
                            }
                        })
                        .collect_view()
                });
                let toggle_name = name.clone();
                let reset_name = name.clone();
                view! {
                    <li class="mat-card" class:open=is_open>
                        <button
                            class="mat-card-head"
                            on:click=move |_| open.update(|o| {
                                if !o.remove(&toggle_name) {
                                    o.insert(toggle_name.clone());
                                }
                            })
                        >
                            <span class="chev">{if is_open { "▾" } else { "▸" }}</span>
                            <b class="clamp-1">{name.clone()}</b>
                            {(edits > 0).then(|| view! { <span class="pill pill-accent">{format!("{edits} manual")}</span> })}
                            {(hidden_count > 0).then(|| view! {
                                <span class="pill pill-outline" title="Texturas ocultas na pré-visualização">
                                    {format!("{hidden_count} oculta{}", if hidden_count > 1 { "s" } else { "" })}
                                </span>
                            })}
                            <span class="mat-mini push-right">
                                {thumbs.into_iter().map(|h| view! { <img src=blob_url(&h) alt="" /> }).collect_view()}
                            </span>
                        </button>
                        {rows}
                        {(is_open && edits > 0 && writable).then(|| view! {
                            <button class="link-btn mat-reset" on:click=move |_| reset_material(reset_name.clone())>
                                {if t == Target::Base { "Restaurar automático" } else { "Remover desta skin" }}
                            </button>
                        })}
                    </li>
                }
            })
            .collect_view();
        Some(view! { <ul class="mat-list">{list}</ul> }.into_any())
    };

    view! {
        <div class="mat-editor">
            {move || error.get().map(|e| view! { <ErrorBanner error=e /> })}
            {move || {
                if version.with(|v| v.as_ref().is_some_and(|v| matches!(v.status, VersionStatus::Pending | VersionStatus::Processing))) {
                    Some(view! { <p class="muted">"Processando a versão… os materiais aparecem quando terminar."</p> }.into_any())
                } else if vid.get().is_none() {
                    Some(view! { <p class="muted">"Selecione uma versão processada de um modelo 3D com materiais."</p> }.into_any())
                } else if baseline.with(|b| b.is_none()) {
                    Some(view! { <p class="muted">"Carregando…"</p> }.into_any())
                } else {
                    None
                }
            }}
            <Show when=move || vid.get().is_some() && baseline.with(|b| b.is_some())>
                <div class="mat-targets tool-group">
                    <button class="seg" class:active=move || target.get() == Target::Base on:click=move |_| target.set(Target::Base)>
                        <span class="seg-main">"Base"</span>
                    </button>
                    {move || {
                        skin_names
                            .get()
                            .into_iter()
                            .enumerate()
                            .map(|(i, n)| {
                                view! {
                                    <button
                                        class="seg"
                                        class:active=move || target.get() == Target::Skin(i)
                                        on:click=move |_| target.set(Target::Skin(i))
                                    >
                                        <span class="seg-main">{if n.trim().is_empty() { "(sem nome)".to_string() } else { n }}</span>
                                    </button>
                                }
                            })
                            .collect_view()
                    }}
                    <Show when=move || can_write.get()>
                        <button class="seg" on:click=add_skin title="Criar um conjunto de texturas alternativo">
                            <span class="seg-main">"+ Skin"</span>
                        </button>
                    </Show>
                </div>
                {move || match target.get() {
                    Target::Skin(i) => Some(view! {
                        <div class="skin-bar">
                            <label class="field">
                                <span>"Nome da skin"</span>
                                <input
                                    type="text"
                                    prop:value=move || edit.with(|c| c.skins.get(i).map(|s| s.name.clone()).unwrap_or_default())
                                    disabled=move || !can_write.get()
                                    on:input=move |ev| {
                                        let v = event_target_value(&ev);
                                        edit.update(|c| if let Some(s) = c.skins.get_mut(i) { s.name = v });
                                    }
                                />
                            </label>
                            <Show when=move || can_write.get()>
                                <button class="btn btn-ghost btn-sm" on:click=delete_skin>"Excluir skin"</button>
                            </Show>
                        </div>
                        <p class="muted hint">"Escolha só o que muda nesta skin; o resto vem da base."</p>
                    }),
                    Target::Base => None,
                }}
                <Show when=move || preview.with(|p| p.as_ref().is_some_and(|p| p.materials.len() > 6))>
                    <input
                        type="search"
                        class="mat-filter"
                        placeholder="Filtrar materiais…"
                        prop:value=move || filter.get()
                        on:input=move |ev| filter.set(event_target_value(&ev))
                    />
                </Show>
                <Show when=move || hidden.with(|h| !h.is_empty())>
                    <div class="mat-hidden-bar">
                        <span class="muted">"Texturas ocultas só na pré-visualização; nada disso é salvo."</span>
                        <button class="link-btn" on:click=move |_| hidden.set(HiddenSet::new())>"Mostrar todas"</button>
                    </div>
                </Show>
                {cards}
                <Show when=move || !can_write.get()>
                    <p class="muted hint">"Somente leitura: você precisa ser editor do projeto para alterar materiais."</p>
                </Show>
                <Show when=move || can_write.get() && dirty.get()>
                    <div class="mat-footer">
                        <input
                            type="text"
                            placeholder="Materiais: texturas atribuídas manualmente"
                            prop:value=move || message.get()
                            on:input=move |ev| message.set(event_target_value(&ev))
                        />
                        <div class="mat-footer-actions">
                            <button class="btn btn-ghost btn-sm" disabled=move || saving.get() on:click=discard>"Descartar"</button>
                            <button class="btn btn-primary btn-sm" disabled=move || saving.get() on:click=save>
                                {move || if saving.get() { "Salvando…" } else { "Salvar nova versão" }}
                            </button>
                        </div>
                        <p class="muted hint">"Cria uma versão nova com " <code>{MATERIAL_CONFIG_PATH}</code> "; as texturas são aplicadas aos GLBs e à miniatura no reprocessamento."</p>
                    </div>
                </Show>
            </Show>
        </div>
    }
}
