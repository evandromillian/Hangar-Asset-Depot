//! Version history: a timeline of the version DAG (newest first).

use crate::components::common::StatusBadge;
use crate::util::{fmt_ago, fmt_bytes, fmt_datetime};
use hangar_core::VersionDto;
use leptos::prelude::*;
use std::collections::HashMap;
use uuid::Uuid;

#[component]
pub fn VersionTimeline(
    versions: Signal<Vec<VersionDto>>,
    selected: RwSignal<Option<Uuid>>,
    can_write: Signal<bool>,
    on_set_head: Callback<Uuid>,
    on_reprocess: Callback<Uuid>,
    on_new: Callback<()>,
) -> impl IntoView {
    view! {
        <div class="timeline">
            <Show when=move || can_write.get()>
                <button class="btn btn-primary btn-block" on:click=move |_| on_new.run(())>
                    "+ Nova versão"
                </button>
            </Show>
            {move || {
                let list = versions.get();
                let by_id: HashMap<Uuid, i32> = list.iter().map(|v| (v.id, v.number)).collect();
                // Branch detection: the parent is not the immediately older version.
                let mut sorted = list.clone();
                sorted.sort_by(|a, b| b.number.cmp(&a.number));
                let numbers: Vec<i32> = sorted.iter().map(|v| v.number).collect();
                sorted
                    .into_iter()
                    .enumerate()
                    .map(|(i, v)| {
                        let id = v.id;
                        let prev_number = numbers.get(i + 1).copied();
                        let parent_number = v.parent_id.and_then(|p| by_id.get(&p).copied());
                        let is_branch = matches!((parent_number, prev_number), (Some(p), Some(prev)) if p != prev);
                        let is_root = v.parent_id.is_none();
                        let parent_id = v.parent_id;
                        let tooltip = fmt_datetime(&v.created_at);
                        view! {
                            <article
                                class="tl-item"
                                class:selected=move || selected.get() == Some(id)
                                class:branch=is_branch
                                on:click=move |_| selected.set(Some(id))
                            >
                                <div class="tl-rail">
                                    <span class="tl-dot" class:head=v.is_head></span>
                                </div>
                                <div class="tl-body">
                                    <div class="tl-top">
                                        <span class="tl-num mono">{format!("v{}", v.number)}</span>
                                        {v.is_head.then(|| view! { <span class="pill pill-accent">"HEAD"</span> })}
                                        <StatusBadge status=v.status />
                                        <span class="muted push-right" title=tooltip>{fmt_ago(&v.created_at)}</span>
                                    </div>
                                    <p class="tl-msg">
                                        {if v.message.is_empty() { "(sem mensagem)".to_string() } else { v.message.clone() }}
                                    </p>
                                    <div class="tl-meta muted">
                                        <span>{v.author.clone().unwrap_or_else(|| "—".into())}</span>
                                        <span>{format!("{} arq · {}", v.file_count, fmt_bytes(v.total_size))}</span>
                                        {match (is_root, parent_number) {
                                            (true, _) => view! { <span class="mono">"raiz"</span> }.into_any(),
                                            (false, Some(p)) => view! {
                                                <button
                                                    class="link-btn mono"
                                                    class:branch-link=is_branch
                                                    title="Selecionar a versão pai"
                                                    on:click=move |ev| {
                                                        ev.stop_propagation();
                                                        selected.set(parent_id);
                                                    }
                                                >
                                                    {if is_branch { format!("⑂ ramo de v{p}") } else { format!("← v{p}") }}
                                                </button>
                                            }
                                            .into_any(),
                                            (false, None) => view! { <span class="mono">"pai removido"</span> }.into_any(),
                                        }}
                                    </div>
                                    <Show when=move || can_write.get() && selected.get() == Some(id)>
                                        <div class="tl-actions">
                                            {(!v.is_head)
                                                .then(|| {
                                                    view! {
                                                        <button
                                                            class="btn btn-sm"
                                                            on:click=move |ev| {
                                                                ev.stop_propagation();
                                                                on_set_head.run(id);
                                                            }
                                                        >
                                                            "Definir como HEAD"
                                                        </button>
                                                    }
                                                })}
                                            <button
                                                class="btn btn-sm btn-ghost"
                                                on:click=move |ev| {
                                                    ev.stop_propagation();
                                                    on_reprocess.run(id);
                                                }
                                            >
                                                "Reprocessar"
                                            </button>
                                        </div>
                                    </Show>
                                </div>
                            </article>
                        }
                    })
                    .collect_view()
            }}
        </div>
    }
}
