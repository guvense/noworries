//! AWS Signature Version 4 for S3 requests.
//!
//! The `s3:` check reads objects back over the plain S3 REST API, which means
//! every request has to be signed. Signing it here (rather than shelling out to
//! `mc`/`aws`) keeps the check dependency-free and makes it work against real
//! S3 or any S3-compatible endpoint, not just the MinIO container noworries
//! starts.
//!
//! Everything in this module is a pure function of its inputs, so the whole
//! signature is unit-testable against a vector produced by a reference
//! implementation — which is how it is tested, because a signature that is
//! subtly wrong fails with an opaque 403 at run time.

use hmac::{Hmac, Mac};
use sha2::{Digest, Sha256};

type HmacSha256 = Hmac<Sha256>;

/// Percent-encode per RFC 3986 as SigV4 requires: unreserved characters pass
/// through, everything else becomes `%XX` (uppercase hex). Path segments keep
/// `/`; query components do not.
pub fn uri_encode(s: &str, keep_slash: bool) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        let c = b as char;
        if c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.' | '~') || (keep_slash && c == '/')
        {
            out.push(c);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

/// `(20260817T101112Z, 20260817)` for an epoch second — SigV4 wants both the
/// full timestamp and the date alone, and they must agree.
///
/// Days are converted with the civil-from-days algorithm rather than a date
/// crate; the arithmetic is exact for any epoch second.
pub fn amz_date(epoch_secs: u64) -> (String, String) {
    let days = (epoch_secs / 86_400) as i64;
    let secs_of_day = epoch_secs % 86_400;
    let (y, m, d) = civil_from_days(days);
    let (hh, mm, ss) = (secs_of_day / 3600, (secs_of_day % 3600) / 60, secs_of_day % 60);
    (
        format!("{y:04}{m:02}{d:02}T{hh:02}{mm:02}{ss:02}Z"),
        format!("{y:04}{m:02}{d:02}"),
    )
}

/// Days since the Unix epoch → (year, month, day). Howard Hinnant's algorithm.
fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = (z - era * 146_097) as u64; // [0, 146096]
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146_096) / 365; // [0, 399]
    let y = yoe as i64 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100); // [0, 365]
    let mp = (5 * doy + 2) / 153; // [0, 11]
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32; // [1, 31]
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32; // [1, 12]
    (if m <= 2 { y + 1 } else { y }, m, d)
}

fn sha256_hex(data: &[u8]) -> String {
    let mut h = Sha256::new();
    h.update(data);
    hex(&h.finalize())
}

fn hmac(key: &[u8], data: &[u8]) -> Vec<u8> {
    let mut mac = HmacSha256::new_from_slice(key).expect("HMAC accepts any key length");
    mac.update(data);
    mac.finalize().into_bytes().to_vec()
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// The `SHA256` of an empty body — the payload hash for GET/HEAD requests.
pub const EMPTY_PAYLOAD_SHA256: &str =
    "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855";

/// Inputs for one signed request.
pub struct SignRequest<'a> {
    pub method: &'a str,
    /// Path, already absolute and un-encoded (e.g. `/uploads/invoices/inv 1.pdf`).
    pub path: &'a str,
    /// Query parameters, unsorted and un-encoded.
    pub query: &'a [(String, String)],
    /// `Host` header value (`127.0.0.1:9000`).
    pub host: &'a str,
    pub region: &'a str,
    pub access_key: &'a str,
    pub secret_key: &'a str,
    /// Epoch seconds; the caller passes `SystemTime::now()` in production and a
    /// fixed value in tests.
    pub now: u64,
}

/// The headers a signed request must carry: `Authorization`, `x-amz-date` and
/// `x-amz-content-sha256` (S3 requires the last one even for empty bodies).
pub fn sign(req: &SignRequest) -> Vec<(String, String)> {
    sign_payload(req, EMPTY_PAYLOAD_SHA256.to_string())
}

/// Sign a request that carries a body (PUT/POST). Exposed so a test — or a
/// future write-side check — can put an object through the same signer the
/// read-side uses; a signature that only works for GETs would be a trap.
#[allow(clippy::too_many_arguments)]
pub fn sign_with_payload(
    method: &str,
    path: &str,
    query: &[(String, String)],
    host: &str,
    region: &str,
    access_key: &str,
    secret_key: &str,
    payload: &[u8],
) -> Vec<(String, String)> {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    sign_payload(
        &SignRequest { method, path, query, host, region, access_key, secret_key, now },
        sha256_hex(payload),
    )
}

fn sign_payload(req: &SignRequest, payload_hash: String) -> Vec<(String, String)> {
    let (amz_datetime, date) = amz_date(req.now);

    // Canonical query: encoded, sorted by key then value.
    let mut q: Vec<(String, String)> = req
        .query
        .iter()
        .map(|(k, v)| (uri_encode(k, false), uri_encode(v, false)))
        .collect();
    q.sort();
    let canonical_query = q
        .iter()
        .map(|(k, v)| format!("{k}={v}"))
        .collect::<Vec<_>>()
        .join("&");

    // Minimal signed header set; each value trimmed, names lowercase and sorted.
    let canonical_headers = format!(
        "host:{}\nx-amz-content-sha256:{}\nx-amz-date:{}\n",
        req.host, payload_hash, amz_datetime
    );
    let signed_headers = "host;x-amz-content-sha256;x-amz-date";

    let canonical_request = format!(
        "{}\n{}\n{}\n{}\n{}\n{}",
        req.method.to_uppercase(),
        uri_encode(req.path, true),
        canonical_query,
        canonical_headers,
        signed_headers,
        payload_hash
    );

    let scope = format!("{date}/{}/s3/aws4_request", req.region);
    let string_to_sign = format!(
        "AWS4-HMAC-SHA256\n{amz_datetime}\n{scope}\n{}",
        sha256_hex(canonical_request.as_bytes())
    );

    let k_date = hmac(format!("AWS4{}", req.secret_key).as_bytes(), date.as_bytes());
    let k_region = hmac(&k_date, req.region.as_bytes());
    let k_service = hmac(&k_region, b"s3");
    let k_signing = hmac(&k_service, b"aws4_request");
    let signature = hex(&hmac(&k_signing, string_to_sign.as_bytes()));

    vec![
        (
            "Authorization".to_string(),
            format!(
                "AWS4-HMAC-SHA256 Credential={}/{scope}, SignedHeaders={signed_headers}, Signature={signature}",
                req.access_key
            ),
        ),
        ("x-amz-date".to_string(), amz_datetime),
        ("x-amz-content-sha256".to_string(), payload_hash),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixed_request() -> SignRequest<'static> {
        SignRequest {
            method: "GET",
            path: "/uploads/invoices/inv-001.pdf",
            query: &[],
            host: "127.0.0.1:9000",
            region: "us-east-1",
            access_key: "noworries",
            secret_key: "noworries",
            now: 1_755_432_672, // 2025-08-17T12:11:12Z
        }
    }

    fn signature_of(headers: &[(String, String)]) -> String {
        let auth = &headers.iter().find(|(k, _)| k == "Authorization").unwrap().1;
        auth.rsplit("Signature=").next().unwrap().to_string()
    }

    /// Pinned against botocore's `SigV4Auth` for the identical request. A
    /// signature that is subtly wrong surfaces only as an opaque 403 from the
    /// server, so it is checked against a reference implementation rather than
    /// against itself.
    #[test]
    fn object_request_signature_matches_botocore() {
        let headers = sign(&fixed_request());
        assert_eq!(
            signature_of(&headers),
            "ae4bded04c59f2e7ee6e1443fd13b198c428325bb0380004e31b613e0c79d14c"
        );
        let auth = &headers.iter().find(|(k, _)| k == "Authorization").unwrap().1;
        assert!(
            auth.contains("Credential=noworries/20250817/us-east-1/s3/aws4_request"),
            "{auth}"
        );
        assert!(auth.contains("SignedHeaders=host;x-amz-content-sha256;x-amz-date"), "{auth}");
    }

    /// Same, for a list request — query encoding and ordering are part of the
    /// signature, so they need their own vector.
    #[test]
    fn list_request_signature_matches_botocore() {
        let q = vec![
            ("prefix".to_string(), "invoices/2026 q1".to_string()),
            ("list-type".to_string(), "2".to_string()),
        ];
        let headers = sign(&SignRequest {
            path: "/uploads",
            query: &q,
            ..fixed_request()
        });
        assert_eq!(
            signature_of(&headers),
            "325694f4249dcd8639c5cffaf547b4a2150194383bdae4c813a5cb8c744f2c91"
        );
    }

    #[test]
    fn uri_encoding_follows_sigv4_rules() {
        assert_eq!(uri_encode("invoices/2026 q1", false), "invoices%2F2026%20q1");
        assert_eq!(uri_encode("invoices/2026 q1", true), "invoices/2026%20q1");
        assert_eq!(uri_encode("a~b_c-d.e", true), "a~b_c-d.e");
        assert_eq!(uri_encode("ä", true), "%C3%A4");
    }

    #[test]
    fn amz_date_splits_timestamp_and_date() {
        let (dt, d) = amz_date(1_755_432_672);
        assert_eq!(dt, "20250817T121112Z");
        assert_eq!(d, "20250817");
        // Leap day, and the epoch itself.
        assert_eq!(amz_date(1_709_208_000).0, "20240229T120000Z");
        assert_eq!(amz_date(0).0, "19700101T000000Z");
    }
}
