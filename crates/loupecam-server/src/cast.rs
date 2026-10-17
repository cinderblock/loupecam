//! Casting: content pushed to whatever display shows this server (the kiosk), the way
//! a Chromecast shows what it's sent.
//!
//! Only the latest cast is kept, in memory. Casting is open to the network unless a
//! display token is configured, and the display token grants nothing else.

use bytes::Bytes;
use serde::Serialize;
use std::sync::atomic::{AtomicU64, Ordering};
use tokio::sync::watch;

/// Largest file that can be cast.
pub const MAX_BYTES: usize = 64 << 20;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum CastKind {
    /// PNG, JPEG, GIF or WebP.
    Image,
    Svg,
    Pdf,
    Html,
    /// A web page shown by address.
    Url,
}

impl CastKind {
    /// Whether a native (non-browser) display can render this kind.
    pub fn native(self) -> bool {
        matches!(self, CastKind::Image | CastKind::Svg)
    }
}

/// What is being cast (without the content itself).
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CastInfo {
    /// Increments with every cast, so displays can tell a re-cast from the same one.
    pub id: u64,
    pub kind: CastKind,
    pub mime: String,
    pub name: Option<String>,
    pub bytes: u64,
    pub url: Option<String>,
    pub since: String,
}

#[derive(Debug, Clone)]
pub struct Cast {
    pub info: CastInfo,
    pub content: Bytes,
}

#[derive(Debug, thiserror::Error)]
pub enum CastError {
    #[error("unsupported content (cast an image, SVG, PDF, HTML page or URL)")]
    Unsupported,
    #[error("only http and https URLs can be cast")]
    BadUrl,
    #[error("empty body")]
    Empty,
}

pub struct Casting {
    pub current: watch::Sender<Option<Cast>>,
    last_id: AtomicU64,
}

impl Default for Casting {
    fn default() -> Self {
        Casting { current: watch::Sender::new(None), last_id: AtomicU64::new(0) }
    }
}

impl Casting {
    pub fn info(&self) -> Option<CastInfo> {
        self.current.borrow().as_ref().map(|c| c.info.clone())
    }

    /// Cast a file. The type comes from its content, falling back to `content_type`.
    pub fn cast_file(&self, content: Bytes, content_type: Option<&str>, name: Option<String>) -> Result<CastInfo, CastError> {
        if content.is_empty() {
            return Err(CastError::Empty);
        }
        let (kind, mime) = classify(&content, content_type).ok_or(CastError::Unsupported)?;
        Ok(self.set(kind, mime.into(), name, None, content))
    }

    pub fn cast_url(&self, url: &str) -> Result<CastInfo, CastError> {
        let url = url.trim();
        let scheme_ok = ["http://", "https://"].iter().any(|s| url.len() > s.len() && url[..s.len()].eq_ignore_ascii_case(s));
        if !scheme_ok || url.chars().any(char::is_control) {
            return Err(CastError::BadUrl);
        }
        Ok(self.set(CastKind::Url, "text/uri-list".into(), None, Some(url.to_string()), Bytes::new()))
    }

    /// Stop casting (displays go back to the camera). Returns whether anything was cast.
    pub fn clear(&self) -> bool {
        self.current.send_if_modified(|c| c.take().is_some())
    }

    fn set(&self, kind: CastKind, mime: String, name: Option<String>, url: Option<String>, content: Bytes) -> CastInfo {
        let info = CastInfo {
            id: self.last_id.fetch_add(1, Ordering::Relaxed) + 1,
            kind,
            mime,
            name,
            bytes: content.len() as u64,
            url,
            since: jiff::Timestamp::now().to_string(),
        };
        self.current.send_replace(Some(Cast { info: info.clone(), content }));
        info
    }
}

/// Identify castable content by its magic bytes, or by the declared type for text
/// formats without one.
fn classify(b: &[u8], content_type: Option<&str>) -> Option<(CastKind, &'static str)> {
    let starts = |m: &[u8]| b.starts_with(m);
    if starts(b"%PDF-") {
        return Some((CastKind::Pdf, "application/pdf"));
    }
    if starts(b"\x89PNG\r\n\x1a\n") {
        return Some((CastKind::Image, "image/png"));
    }
    if starts(b"\xff\xd8\xff") {
        return Some((CastKind::Image, "image/jpeg"));
    }
    if starts(b"GIF87a") || starts(b"GIF89a") {
        return Some((CastKind::Image, "image/gif"));
    }
    if b.len() >= 12 && &b[..4] == b"RIFF" && &b[8..12] == b"WEBP" {
        return Some((CastKind::Image, "image/webp"));
    }
    let declared = content_type.map(|t| t.split(';').next().unwrap_or("").trim().to_ascii_lowercase());
    // Text formats: look at the start of the document, past a BOM and whitespace.
    let head = String::from_utf8_lossy(&b[..b.len().min(1024)]).to_ascii_lowercase();
    let head = head.trim_start_matches('\u{feff}').trim_start();
    let svg = head.starts_with("<svg") || (head.starts_with("<?xml") && head.contains("<svg"));
    if svg || declared.as_deref() == Some("image/svg+xml") {
        return Some((CastKind::Svg, "image/svg+xml"));
    }
    if head.starts_with("<!doctype html") || head.starts_with("<html") || declared.as_deref() == Some("text/html") {
        return Some((CastKind::Html, "text/html"));
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classifies_by_content() {
        assert_eq!(classify(b"%PDF-1.7\n...", None).unwrap().0, CastKind::Pdf);
        assert_eq!(classify(b"\x89PNG\r\n\x1a\n....", Some("application/octet-stream")).unwrap().1, "image/png");
        assert_eq!(classify(b"\xef\xbb\xbf  <?xml version='1.0'?>\n<svg xmlns=''/>", None).unwrap().0, CastKind::Svg);
        assert_eq!(classify(b"<!DOCTYPE html><p>hi", None).unwrap().0, CastKind::Html);
        assert_eq!(classify(b"<p>fragment", Some("text/html; charset=utf-8")).unwrap().0, CastKind::Html);
        assert!(classify(b"hello", Some("text/plain")).is_none());
    }

    #[test]
    fn urls_and_ids() {
        let c = Casting::default();
        assert!(matches!(c.cast_url("javascript:alert(1)"), Err(CastError::BadUrl)));
        assert!(matches!(c.cast_url("file:///etc/passwd"), Err(CastError::BadUrl)));
        assert!(matches!(c.cast_url("https://"), Err(CastError::BadUrl)));
        let a = c.cast_url(" https://example.com/x ").unwrap();
        assert_eq!(a.url.as_deref(), Some("https://example.com/x"));
        let b = c.cast_file(Bytes::from_static(b"%PDF-1.4"), None, Some("w.pdf".into())).unwrap();
        assert_eq!((a.id, b.id), (1, 2));
        assert!(c.clear());
        assert!(!c.clear());
        assert!(c.info().is_none());
        // Ids keep increasing across a clear.
        assert_eq!(c.cast_url("http://a.b").unwrap().id, 3);
    }
}
