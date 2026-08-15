//! External / upstream dependency wiring.
//!
//! An *external* is a service the app under test calls out to but that noworries
//! does not stand up — a partner sandbox API, a separate auth server, a
//! third-party gateway. noworries can't containerize it, but it can inject its
//! URL and credentials into the app's environment so the app reaches it during
//! a run. Each external contributes env vars under **two** naming schemes:
//!
//! - the app's own names, when given (`url_env`, per-auth `*_env`, and the
//!   literal `env` map), so no app config change is needed; and
//! - conventional `NOWORRIES_EXTERNAL_<NAME>_*` names, always set, so a
//!   convention-driven app (or a quick check) can read them with zero mapping.
//!
//! Every string value interpolates `${VAR}` from the resolved var env
//! (`app.env` < `.noworries.env` < process env), so secrets stay in the
//! gitignored `.noworries.env` and are prompted for when missing.

use std::collections::BTreeMap;

use crate::runner::{base64_encode, interpolate};
use crate::spec::ExternalSpec;

/// Normalize a name into an uppercase, env-var-safe token
/// (`payments-v2` → `PAYMENTS_V2`).
fn env_token(name: &str) -> String {
    name.chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() {
                c.to_ascii_uppercase()
            } else {
                '_'
            }
        })
        .collect()
}

/// Build the env vars to inject into the app for every external dependency.
/// `vars` is the `${VAR}` lookup env (app.env < .noworries.env < process env).
/// `mock_urls` maps an external's name to a running mock's URL; when present it
/// overrides the declared `url` so the app calls the mock instead of the sandbox.
pub fn resolve_external_env(
    externals: &[ExternalSpec],
    vars: &BTreeMap<String, String>,
    mock_urls: &BTreeMap<String, String>,
) -> BTreeMap<String, String> {
    let mut out = BTreeMap::new();
    let ip = |s: &str| interpolate(s, vars);

    for ext in externals {
        let prefix = format!("NOWORRIES_EXTERNAL_{}", env_token(&ext.name));

        // A mock's URL wins over a declared sandbox URL.
        let resolved_url = mock_urls
            .get(&ext.name)
            .cloned()
            .or_else(|| ext.url.as_ref().map(|u| ip(u)));
        if let Some(u) = resolved_url {
            out.insert(format!("{prefix}_URL"), u.clone());
            if let Some(name) = &ext.url_env {
                out.insert(name.clone(), u);
            }
        }

        // Literal extra env vars (interpolated).
        for (k, v) in &ext.env {
            out.insert(k.clone(), ip(v));
        }

        if let Some(auth) = &ext.auth {
            if let Some(b) = &auth.basic {
                let user = ip(&b.username);
                let pass = ip(&b.password);
                let header =
                    format!("Basic {}", base64_encode(format!("{user}:{pass}").as_bytes()));
                out.insert(format!("{prefix}_USER"), user.clone());
                out.insert(format!("{prefix}_PASSWORD"), pass.clone());
                out.insert(format!("{prefix}_AUTHORIZATION"), header.clone());
                if let Some(n) = &b.username_env {
                    out.insert(n.clone(), user);
                }
                if let Some(n) = &b.password_env {
                    out.insert(n.clone(), pass);
                }
                if let Some(n) = &b.header_env {
                    out.insert(n.clone(), header);
                }
            }
            if let Some(be) = &auth.bearer {
                let token = ip(&be.token);
                let scheme = be.scheme.clone().unwrap_or_else(|| "Bearer".to_string());
                let header = if scheme.is_empty() {
                    token.clone()
                } else {
                    format!("{scheme} {token}")
                };
                out.insert(format!("{prefix}_TOKEN"), token.clone());
                out.insert(format!("{prefix}_AUTHORIZATION"), header.clone());
                if let Some(n) = &be.token_env {
                    out.insert(n.clone(), token);
                }
                if let Some(n) = &be.header_env {
                    out.insert(n.clone(), header);
                }
            }
            if let Some(k) = &auth.api_key {
                let value = ip(&k.value);
                let header_name = k.header.clone().unwrap_or_else(|| "X-API-Key".to_string());
                out.insert(format!("{prefix}_API_KEY"), value.clone());
                out.insert(format!("{prefix}_API_KEY_HEADER"), header_name);
                if let Some(n) = &k.value_env {
                    out.insert(n.clone(), value);
                }
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::spec::NoworriesSpec;

    fn externals_of(yaml: &str) -> Vec<ExternalSpec> {
        NoworriesSpec::parse(yaml).unwrap().externals
    }

    #[test]
    fn basic_auth_sets_raw_and_ready_header_plus_conventional_and_explicit() {
        let yaml = r#"
version: 1
services: [postgres]
externals:
  - name: payments-v2
    url: "https://sandbox.pay.example.com"
    url_env: PAYMENTS_BASE_URL
    auth:
      basic:
        username: "alice"
        password: "s3cret"
        username_env: PAY_USER
        header_env: PAY_AUTH
"#;
        let vars = BTreeMap::new();
        let env = resolve_external_env(&externals_of(yaml), &vars, &BTreeMap::new());
        // conventional (name normalized)
        assert_eq!(env["NOWORRIES_EXTERNAL_PAYMENTS_V2_URL"], "https://sandbox.pay.example.com");
        assert_eq!(env["NOWORRIES_EXTERNAL_PAYMENTS_V2_USER"], "alice");
        assert_eq!(env["NOWORRIES_EXTERNAL_PAYMENTS_V2_PASSWORD"], "s3cret");
        assert_eq!(env["NOWORRIES_EXTERNAL_PAYMENTS_V2_AUTHORIZATION"], "Basic YWxpY2U6czNjcmV0");
        // explicit app-specific names
        assert_eq!(env["PAYMENTS_BASE_URL"], "https://sandbox.pay.example.com");
        assert_eq!(env["PAY_USER"], "alice");
        assert_eq!(env["PAY_AUTH"], "Basic YWxpY2U6czNjcmV0");
    }

    #[test]
    fn secrets_interpolate_from_vars() {
        let yaml = r#"
version: 1
services: [postgres]
externals:
  - name: partner
    url: "${PARTNER_URL}"
    auth:
      bearer: { token: "${PARTNER_TOKEN}" }
"#;
        let mut vars = BTreeMap::new();
        vars.insert("PARTNER_URL".into(), "https://x.example".into());
        vars.insert("PARTNER_TOKEN".into(), "tok123".into());
        let env = resolve_external_env(&externals_of(yaml), &vars, &BTreeMap::new());
        assert_eq!(env["NOWORRIES_EXTERNAL_PARTNER_URL"], "https://x.example");
        assert_eq!(env["NOWORRIES_EXTERNAL_PARTNER_TOKEN"], "tok123");
        assert_eq!(env["NOWORRIES_EXTERNAL_PARTNER_AUTHORIZATION"], "Bearer tok123");
    }

    #[test]
    fn api_key_sets_value_and_header_name() {
        let yaml = r#"
version: 1
services: [postgres]
externals:
  - name: geo
    auth:
      api_key: { value: "K1", header: "X-Geo-Key", value_env: GEO_KEY }
    env:
      GEO_REGION: "eu"
"#;
        let vars = BTreeMap::new();
        let env = resolve_external_env(&externals_of(yaml), &vars, &BTreeMap::new());
        assert_eq!(env["NOWORRIES_EXTERNAL_GEO_API_KEY"], "K1");
        assert_eq!(env["NOWORRIES_EXTERNAL_GEO_API_KEY_HEADER"], "X-Geo-Key");
        assert_eq!(env["GEO_KEY"], "K1");
        assert_eq!(env["GEO_REGION"], "eu");
    }
}
