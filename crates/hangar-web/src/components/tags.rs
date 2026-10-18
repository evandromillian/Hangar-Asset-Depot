//! Tag editor for the asset form: chips plus an inline input.

use crate::util::{event_key, parse_tags};
use leptos::prelude::*;

/// Server limits (`clean_tags` in hangar-server): 40 characters, 32 tags.
const MAX_TAG_LEN: usize = 40;
const MAX_TAGS: usize = 32;

/// Adds the tags written in `text` (split like [`parse_tags`]) that are not there yet.
fn add_tags(tags: RwSignal<Vec<String>>, text: &str) {
    tags.update(|list| {
        for t in parse_tags(text) {
            let t: String = t.trim_start_matches('#').chars().take(MAX_TAG_LEN).collect();
            if !t.is_empty() && list.len() < MAX_TAGS && !list.contains(&t) {
                list.push(t);
            }
        }
    });
}

/// Enter, comma, semicolon or space turns the typed text into a chip (pasted
/// lists are split the same way); Backspace on an empty input removes the last
/// chip; leaving the field keeps what was typed.
#[component]
pub fn TagInput(tags: RwSignal<Vec<String>>, #[prop(into, optional)] placeholder: String) -> impl IntoView {
    let draft = RwSignal::new(String::new());
    let input = NodeRef::<leptos::html::Input>::new();
    let commit = move || {
        add_tags(tags, &draft.get_untracked());
        draft.set(String::new());
    };

    view! {
        <div
            class="tag-input"
            on:click=move |_| {
                if let Some(el) = input.get() {
                    let _ = el.focus();
                }
            }
        >
            {move || {
                tags.get()
                    .into_iter()
                    .map(|t| {
                        let key = t.clone();
                        view! {
                            <span class="tag static">
                                {t.clone()}
                                <button
                                    type="button"
                                    class="tag-remove"
                                    title="Remover tag"
                                    aria-label=format!("Remover a tag {t}")
                                    on:click=move |ev| {
                                        ev.stop_propagation();
                                        tags.update(|l| l.retain(|x| *x != key));
                                    }
                                >
                                    "✕"
                                </button>
                            </span>
                        }
                    })
                    .collect_view()
            }}
            <input
                node_ref=input
                type="text"
                placeholder=move || if tags.with(|t| t.is_empty()) { placeholder.clone() } else { String::new() }
                prop:value=move || draft.get()
                on:input=move |ev| {
                    let text = event_target_value(&ev);
                    // A typed or pasted separator finishes everything before it.
                    match text.rfind([',', ';', ' ']) {
                        Some(i) => {
                            add_tags(tags, &text[..i]);
                            draft.set(text[i + 1..].to_string());
                        }
                        None => draft.set(text),
                    }
                }
                on:keydown=move |ev| match event_key(&ev).as_deref() {
                    // Enter would submit the surrounding form.
                    Some("Enter") => {
                        ev.prevent_default();
                        commit();
                    }
                    Some("Backspace") if draft.with_untracked(|d| d.is_empty()) => {
                        tags.update(|l| {
                            l.pop();
                        });
                    }
                    _ => {}
                }
                on:blur=move |_| commit()
            />
        </div>
    }
}
