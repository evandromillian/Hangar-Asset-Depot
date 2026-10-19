//! Small formatting helpers shared by the pages.

use chrono::{DateTime, Duration, FixedOffset, Utc};
use hangar_core::{AssetKind, VersionStatus};

pub fn fmt_bytes(n: i64) -> String {
    let n = n.max(0) as f64;
    const UNITS: [&str; 5] = ["B", "KB", "MB", "GB", "TB"];
    let mut v = n;
    let mut u = 0;
    while v >= 1024.0 && u < UNITS.len() - 1 {
        v /= 1024.0;
        u += 1;
    }
    if u == 0 { format!("{} B", n as i64) } else { format!("{v:.1} {}", UNITS[u]).replace('.', ",") }
}

/// 1234567 → "1.234.567" (pt-BR grouping).
pub fn fmt_int(n: u64) -> String {
    let s = n.to_string();
    let mut out = String::with_capacity(s.len() + s.len() / 3);
    for (i, c) in s.chars().enumerate() {
        if i > 0 && (s.len() - i) % 3 == 0 {
            out.push('.');
        }
        out.push(c);
    }
    out
}

/// Compact triangle count: 12.4k, 1.2M.
pub fn fmt_compact(n: u64) -> String {
    match n {
        0..=999 => n.to_string(),
        1_000..=999_999 => format!("{:.1}k", n as f64 / 1e3),
        _ => format!("{:.2}M", n as f64 / 1e6),
    }
}

fn local_offset() -> FixedOffset {
    let minutes = -(js_sys::Date::new_0().get_timezone_offset() as i32);
    FixedOffset::east_opt(minutes * 60).unwrap_or_else(|| FixedOffset::east_opt(0).unwrap())
}

pub fn fmt_datetime(t: &DateTime<Utc>) -> String {
    t.with_timezone(&local_offset()).format("%d/%m/%Y %H:%M").to_string()
}

pub fn fmt_date(t: &DateTime<Utc>) -> String {
    t.with_timezone(&local_offset()).format("%d/%m/%Y").to_string()
}

/// "há 5 min", "há 3 dias"...
pub fn fmt_ago(t: &DateTime<Utc>) -> String {
    let now = Utc::now();
    let d = now.signed_duration_since(*t);
    if d < Duration::minutes(1) {
        "agora".into()
    } else if d < Duration::hours(1) {
        format!("há {} min", d.num_minutes())
    } else if d < Duration::days(1) {
        format!("há {} h", d.num_hours())
    } else if d < Duration::days(30) {
        let n = d.num_days();
        format!("há {n} dia{}", if n == 1 { "" } else { "s" })
    } else {
        fmt_date(t)
    }
}

pub fn kind_label(k: AssetKind) -> &'static str {
    match k {
        AssetKind::Model => "Modelo 3D",
        AssetKind::Texture => "Textura",
        AssetKind::Material => "Material",
        AssetKind::Audio => "Áudio",
        AssetKind::Animation => "Animação",
        AssetKind::Vfx => "VFX",
        AssetKind::Other => "Outro",
    }
}

pub fn kind_glyph(k: AssetKind) -> &'static str {
    match k {
        AssetKind::Model => "◆",
        AssetKind::Texture => "▦",
        AssetKind::Material => "◐",
        AssetKind::Audio => "♪",
        AssetKind::Animation => "↻",
        AssetKind::Vfx => "✶",
        AssetKind::Other => "■",
    }
}

pub fn status_label(s: VersionStatus) -> &'static str {
    match s {
        VersionStatus::Pending => "na fila",
        VersionStatus::Processing => "processando",
        VersionStatus::Ready => "pronto",
        VersionStatus::Failed => "falhou",
    }
}

/// Lowercase, ascii, dash separated — mirrors what the server does for slugs.
pub fn slugify(s: &str) -> String {
    let mut out = String::new();
    let mut dash = false;
    for c in s.chars() {
        let c = match c {
            'á' | 'à' | 'â' | 'ã' | 'ä' | 'Á' | 'À' | 'Â' | 'Ã' => 'a',
            'é' | 'ê' | 'è' | 'É' | 'Ê' => 'e',
            'í' | 'Í' => 'i',
            'ó' | 'ô' | 'õ' | 'Ó' | 'Ô' | 'Õ' => 'o',
            'ú' | 'ü' | 'Ú' => 'u',
            'ç' | 'Ç' => 'c',
            c => c,
        };
        if c.is_ascii_alphanumeric() {
            out.push(c.to_ascii_lowercase());
            dash = false;
        } else if !dash && !out.is_empty() {
            out.push('-');
            dash = true;
        }
    }
    out.trim_end_matches('-').to_string()
}

/// `KeyboardEvent.key`, read defensively: Chrome dispatches `keydown` events
/// without a `key` when a <datalist> / autofill suggestion is picked, and
/// `KeyboardEvent::key()` throws on them.
pub fn event_key(ev: &web_sys::KeyboardEvent) -> Option<String> {
    js_sys::Reflect::get(ev, &"key".into()).ok().and_then(|k| k.as_string())
}

/// Splits "tag1, tag2  tag3" into clean tags.
pub fn parse_tags(s: &str) -> Vec<String> {
    s.split([',', ' ', ';'])
        .map(|t| t.trim().to_lowercase())
        .filter(|t| !t.is_empty())
        .collect()
}

pub fn short_hash(h: &str) -> &str {
    &h[..h.len().min(10)]
}

/// Per-browser UI preference (localStorage). `None` when storage is unavailable (private mode, blocked).
pub fn pref_get(key: &str) -> Option<String> {
    web_sys::window()?.local_storage().ok()??.get_item(key).ok()?
}

pub fn pref_set(key: &str, value: &str) {
    if let Some(s) = web_sys::window().and_then(|w| w.local_storage().ok().flatten()) {
        let _ = s.set_item(key, value);
    }
}
