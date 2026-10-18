//! Small helpers on top of summer-web's error type.
//!
//! Handlers return `summer_web::error::Result<T>`; `KnownWebError` carries an
//! HTTP status + message, anything else (`anyhow::Error`) becomes a 500.

pub use summer_web::error::{KnownWebError, Result, WebError};

pub fn bad_request(msg: impl Into<String>) -> WebError {
    KnownWebError::bad_request(msg).into()
}
pub fn unauthorized(msg: impl Into<String>) -> WebError {
    KnownWebError::unauthorized(msg).into()
}
pub fn forbidden(msg: impl Into<String>) -> WebError {
    KnownWebError::forbidden(msg).into()
}
pub fn not_found(msg: impl Into<String>) -> WebError {
    KnownWebError::not_found(msg).into()
}
pub fn conflict(msg: impl Into<String>) -> WebError {
    KnownWebError::conflict(msg).into()
}

/// `.ctx("...")?` on database / storage errors → 500 with context.
pub trait Ctx<T> {
    fn ctx(self, what: &'static str) -> Result<T>;
}

impl<T, E> Ctx<T> for std::result::Result<T, E>
where
    E: std::error::Error + Send + Sync + 'static,
{
    fn ctx(self, what: &'static str) -> Result<T> {
        self.map_err(|e| WebError::ServerError(anyhow::Error::new(e).context(what)))
    }
}

/// Turns a free-form name into a URL slug: "M1A2 Abrams (Desert)" → "m1a2-abrams-desert".
pub fn slugify(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut dash = false;
    for c in s.chars() {
        let c = fold_accent(c).to_ascii_lowercase();
        if c.is_ascii_alphanumeric() {
            out.push(c);
            dash = false;
        } else if !dash && !out.is_empty() {
            out.push('-');
            dash = true;
        }
    }
    while out.ends_with('-') {
        out.pop();
    }
    out.truncate(80);
    if out.is_empty() { "item".into() } else { out }
}

fn fold_accent(c: char) -> char {
    match c {
        'á' | 'à' | 'â' | 'ã' | 'ä' | 'Á' | 'À' | 'Â' | 'Ã' | 'Ä' => 'a',
        'é' | 'è' | 'ê' | 'ë' | 'É' | 'È' | 'Ê' | 'Ë' => 'e',
        'í' | 'ì' | 'î' | 'ï' | 'Í' | 'Ì' | 'Î' | 'Ï' => 'i',
        'ó' | 'ò' | 'ô' | 'õ' | 'ö' | 'Ó' | 'Ò' | 'Ô' | 'Õ' | 'Ö' => 'o',
        'ú' | 'ù' | 'û' | 'ü' | 'Ú' | 'Ù' | 'Û' | 'Ü' => 'u',
        'ç' | 'Ç' => 'c',
        'ñ' | 'Ñ' => 'n',
        other => other,
    }
}

#[cfg(test)]
mod tests {
    use super::slugify;

    #[test]
    fn slugs() {
        assert_eq!(slugify("M1A2 Abrams (Desert)"), "m1a2-abrams-desert");
        assert_eq!(slugify("  Caça  F-35B  "), "caca-f-35b");
        assert_eq!(slugify("!!!"), "item");
    }
}
