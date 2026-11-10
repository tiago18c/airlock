//! Downloading a token's logo from where the service says it is. The host
//! can be anyone the service names, so the request carries nothing of ours:
//! no key, no cookie, no header of our own. Only the URL goes out.

use wallet_core::portfolio::LogoKind;

/// Largest logo kept. A 28 pixel avatar needs a few kilobytes; anything near
/// this is not a logo meant for a list.
pub const LOGO_MAX_BYTES: usize = 128 * 1024;

/// The format of an image, from its first bytes. The `Content-Type` a host
/// sends is not trusted: only PNG, JPEG and WebP are kept, and SVG, which can
/// carry scripts, never is.
pub fn sniff_logo(bytes: &[u8]) -> Option<LogoKind> {
    const PNG: &[u8] = &[0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A];
    if bytes.starts_with(PNG) {
        Some(LogoKind::Png)
    } else if bytes.starts_with(&[0xFF, 0xD8, 0xFF]) {
        Some(LogoKind::Jpeg)
    } else if bytes.len() >= 12 && bytes.starts_with(b"RIFF") && &bytes[8..12] == b"WEBP" {
        Some(LogoKind::Webp)
    } else {
        None
    }
}

#[cfg(not(target_arch = "wasm32"))]
pub(crate) use client::LogoClient;

#[cfg(not(target_arch = "wasm32"))]
mod client {
    use super::{sniff_logo, LOGO_MAX_BYTES};
    use std::time::Duration;
    use wallet_core::portfolio::LogoFetch;

    const TIMEOUT: Duration = Duration::from_secs(10);
    const MAX_REDIRECTS: usize = 3;

    /// A client of its own for logos. It is never the one the Tokens API
    /// calls go through, and nothing adds a header to it, so the key, which
    /// only `TokensApi::send` puts on a request of the other client, cannot
    /// reach a logo host. It keeps no cookies (reqwest is built here without
    /// a cookie store) and sends no `Referer` on a redirect.
    pub(crate) struct LogoClient {
        /// `None` when the client could not be built with the settings
        /// above: it then downloads nothing.
        client: Option<reqwest::Client>,
        /// Off only in tests, which serve logos over plain HTTP.
        https_only: bool,
    }

    impl LogoClient {
        pub(crate) fn new() -> Self {
            Self::build(true)
        }

        /// A client that also fetches `http://`, for a test server.
        #[cfg(test)]
        pub(crate) fn plain_http_for_tests() -> Self {
            Self::build(false)
        }

        fn build(https_only: bool) -> Self {
            // Not a default client in its place: that one would follow
            // plain HTTP redirects and wait without end.
            let client = reqwest::Client::builder()
                .https_only(https_only)
                .redirect(reqwest::redirect::Policy::limited(MAX_REDIRECTS))
                // A redirect would otherwise tell the next host the URL
                // before it.
                .referer(false)
                // Another crate of the app turns reqwest's decompression on,
                // which would add an `Accept-Encoding` header. Images are
                // compressed already, and the size limit is then on the bytes
                // that come over the wire.
                .no_gzip()
                .no_brotli()
                .no_deflate()
                .no_zstd()
                .timeout(TIMEOUT)
                .build()
                .ok();
            Self { client, https_only }
        }

        pub(crate) async fn fetch(&self, url: &str) -> LogoFetch {
            let scheme_ok =
                url.starts_with("https://") || (!self.https_only && url.starts_with("http://"));
            if !scheme_ok {
                return LogoFetch::Unusable;
            }
            let Some(client) = &self.client else {
                return LogoFetch::Unreachable;
            };
            let mut response = match client.get(url).send().await {
                Ok(response) => response,
                Err(err) => return failed(&err),
            };
            match response.status().as_u16() {
                200..=299 => {}
                429 | 500..=599 => return LogoFetch::Unreachable,
                _ => return LogoFetch::Unusable,
            }
            if response
                .content_length()
                .is_some_and(|length| length > LOGO_MAX_BYTES as u64)
            {
                return LogoFetch::Unusable;
            }
            let mut bytes: Vec<u8> = Vec::new();
            loop {
                match response.chunk().await {
                    Ok(Some(chunk)) => {
                        if bytes.len() + chunk.len() > LOGO_MAX_BYTES {
                            return LogoFetch::Unusable;
                        }
                        bytes.extend_from_slice(&chunk);
                    }
                    Ok(None) => break,
                    Err(err) => return failed(&err),
                }
            }
            match sniff_logo(&bytes) {
                Some(kind) => LogoFetch::Image { kind, bytes },
                None => LogoFetch::Unusable,
            }
        }
    }

    /// A request that failed. Too many redirects, or one to plain HTTP, is an
    /// answer, and not a logo; anything else may go better later.
    fn failed(err: &reqwest::Error) -> LogoFetch {
        if err.is_redirect() || err.is_builder() {
            LogoFetch::Unusable
        } else {
            LogoFetch::Unreachable
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_png_jpeg_and_webp_are_logos() {
        let png = [0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A, 0, 0, 0, 13];
        assert_eq!(sniff_logo(&png), Some(LogoKind::Png));
        assert_eq!(
            sniff_logo(&[0xFF, 0xD8, 0xFF, 0xE0, 0, 16]),
            Some(LogoKind::Jpeg)
        );
        assert_eq!(
            sniff_logo(b"RIFF\x24\x00\x00\x00WEBPVP8 "),
            Some(LogoKind::Webp)
        );
        // RIFF of another kind: a WAV file.
        assert_eq!(sniff_logo(b"RIFF\x24\x00\x00\x00WAVEfmt "), None);
        assert_eq!(
            sniff_logo(b"<svg xmlns=\"http://www.w3.org/2000/svg\"></svg>"),
            None
        );
        assert_eq!(sniff_logo(b"<?xml version=\"1.0\"?><svg/>"), None);
        assert_eq!(sniff_logo(b"GIF89a\x01\x00\x01\x00"), None);
        assert_eq!(sniff_logo(b"<!DOCTYPE html><html></html>"), None);
        assert_eq!(sniff_logo(b""), None);
        // Too short to be what they start like.
        assert_eq!(sniff_logo(&png[..7]), None);
        assert_eq!(sniff_logo(&[0xFF, 0xD8]), None);
        assert_eq!(sniff_logo(b"RIFF\x24\x00\x00\x00WEB"), None);
    }
}
