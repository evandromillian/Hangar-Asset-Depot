//! Asset metadata + processing details of the selected version.

use crate::util::{fmt_bytes, fmt_datetime, fmt_int};
use hangar_core::{AssetDetail, VersionDto, blob_url};
use leptos::prelude::*;

#[component]
pub fn DetailsPanel(asset: Signal<Option<AssetDetail>>, version: Signal<Option<VersionDto>>) -> impl IntoView {
    view! {
        <div class="details">
            {move || {
                asset
                    .get()
                    .map(|a| {
                        let s = a.source.clone();
                        let rows: Vec<(&str, Option<String>)> = vec![
                            ("Loja", s.vendor.clone()),
                            ("Autor", s.author.clone()),
                            ("Licença", s.license.clone()),
                            ("Pedido", s.order_ref.clone()),
                            ("Compra", s.purchased_at.map(|d| d.format("%d/%m/%Y").to_string())),
                            ("Notas", s.notes.clone()),
                        ];
                        view! {
                            <section class="detail-block">
                                <h3>"Asset"</h3>
                                <p class="prose">
                                    {if a.description.is_empty() { "Sem descrição.".to_string() } else { a.description.clone() }}
                                </p>
                                <dl class="kv">
                                    {rows
                                        .into_iter()
                                        .filter_map(|(k, v)| v.map(|v| view! { <dt>{k}</dt><dd>{v}</dd> }))
                                        .collect_view()}
                                    {s.url.clone().map(|u| view! {
                                        <dt>"URL"</dt>
                                        <dd><a href=u.clone() target="_blank" rel="noopener noreferrer">{u.clone()}</a></dd>
                                    })}
                                    <dt>"Criado"</dt>
                                    <dd>{fmt_datetime(&a.created_at)} {a.created_by.map(|c| format!(" · {c}"))}</dd>
                                    <dt>"Slug"</dt>
                                    <dd class="mono">{a.summary.slug.clone()}</dd>
                                </dl>
                            </section>
                        }
                    })
            }}
            {move || {
                version
                    .get()
                    .and_then(|v| v.preview.clone().map(|p| (v, p)))
                    .map(|(v, p)| {
                        view! {
                            <section class="detail-block">
                                <h3>{format!("Processamento v{}", v.number)}</h3>
                                <dl class="kv">
                                    {p.primary_file.clone().map(|f| view! { <dt>"Arquivo principal"</dt><dd class="mono">{f}</dd> })}
                                    {p.importer.clone().map(|i| view! { <dt>"Importador"</dt><dd class="mono">{i}</dd> })}
                                    {v.processed_at.map(|t| view! { <dt>"Processado"</dt><dd>{fmt_datetime(&t)}</dd> })}
                                    {p.stats.clone().map(|s| {
                                        let d = s.dimensions();
                                        view! {
                                            <dt>"Triângulos"</dt><dd class="mono">{fmt_int(s.triangles)}</dd>
                                            <dt>"Vértices"</dt><dd class="mono">{fmt_int(s.vertices)}</dd>
                                            <dt>"Malhas / materiais"</dt><dd class="mono">{format!("{} / {}", s.meshes, s.materials)}</dd>
                                            <dt>"Dimensões"</dt>
                                            <dd class="mono">{format!("{:.3} × {:.3} × {:.3} m", d[0], d[1], d[2])}</dd>
                                            <dt>"Atributos"</dt>
                                            <dd class="flags">
                                                <span class:on=s.has_uvs>"UV"</span>
                                                <span class:on=s.has_normals>"normais"</span>
                                                <span class:on=s.has_skin>"skin"</span>
                                                <span class:on={s.animations > 0}>{format!("{} anim", s.animations)}</span>
                                            </dd>
                                        }
                                    })}
                                </dl>
                                {(!p.warnings.is_empty()).then(|| view! {
                                    <ul class="warnings">
                                        {p.warnings.iter().map(|w| view! { <li>{w.clone()}</li> }).collect_view()}
                                    </ul>
                                })}
                            </section>
                            {(!p.materials.is_empty()).then(|| view! {
                                <section class="detail-block">
                                    <h3>"Materiais"</h3>
                                    <ul class="materials">
                                        {p.materials.iter().map(|m| {
                                            let c = m.base_color;
                                            let swatch = format!(
                                                "background: rgb({}, {}, {})",
                                                (c[0].clamp(0.0, 1.0) * 255.0) as u8,
                                                (c[1].clamp(0.0, 1.0) * 255.0) as u8,
                                                (c[2].clamp(0.0, 1.0) * 255.0) as u8,
                                            );
                                            view! {
                                                <li>
                                                    <div class="mat-head">
                                                        <span class="swatch" style=swatch></span>
                                                        <b>{m.name.clone()}</b>
                                                        <span class="muted mono push-right">
                                                            {format!("M {:.2} · R {:.2}", m.metallic, m.roughness)}
                                                        </span>
                                                    </div>
                                                    <ul class="slots">
                                                        {m.textures.iter().map(|t| {
                                                            let cls = format!("slot res-{}", t.resolved_by);
                                                            let title = t.original.clone().unwrap_or_default();
                                                            view! {
                                                                <li class=cls title=title>
                                                                    <span class="slot-name">{t.slot.label()}</span>
                                                                    <span class="mono slot-path">
                                                                        {t.path.clone().unwrap_or_else(|| "— não encontrada".into())}
                                                                    </span>
                                                                    <span class="slot-res">{t.resolved_by.clone()}</span>
                                                                </li>
                                                            }
                                                        }).collect_view()}
                                                    </ul>
                                                </li>
                                            }
                                        }).collect_view()}
                                    </ul>
                                </section>
                            })}
                            {(!p.textures.is_empty()).then(|| view! {
                                <section class="detail-block">
                                    <h3>{format!("Texturas ({})", p.textures.len())}</h3>
                                    <div class="tex-grid">
                                        {p.textures.iter().map(|t| {
                                            let src = t.preview_hash.as_deref().map(blob_url).unwrap_or_default();
                                            view! {
                                                <figure class="tex">
                                                    <div class="checker"><img src=src alt="" loading="lazy" /></div>
                                                    <figcaption>
                                                        <span class="mono clamp-1" title=t.path.clone()>{hangar_core::paths::file_name(&t.path).to_string()}</span>
                                                        <span class="muted mono">
                                                            {format!("{}×{} · {}", t.width, t.height, fmt_bytes(t.size as i64))}
                                                        </span>
                                                    </figcaption>
                                                </figure>
                                            }
                                        }).collect_view()}
                                    </div>
                                </section>
                            })}
                        }
                    })
            }}
        </div>
    }
}
