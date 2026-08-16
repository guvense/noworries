//! Ruby on Rails framework adapter.
//!
//! Detects a Rails app by `bin/rails` (or `config/application.rb`) — a plain
//! `Gemfile` is too broad (Sinatra, plain Ruby). Rails has no default health
//! path in older versions (Rails 7.1+ ships `/up`), so readiness is TCP-based
//! (`health: "none"`) unless the spec sets one. Rails reads the HTTP port from
//! `PORT` and natively understands `DATABASE_URL` / `REDIS_URL`, so
//! [`conventional_env`] wires it directly.

use std::collections::BTreeMap;
use std::path::Path;

use super::{conventional_env, Framework};
use crate::lifecycle::ServiceEndpoint;

pub struct Rails;

impl Framework for Rails {
    fn name(&self) -> &'static str {
        "rails"
    }

    fn detect(&self, dir: &Path) -> bool {
        // `bin/rails` is the canonical Rails launcher; `config/application.rb`
        // covers apps whose bin stubs were stripped. Either is a strong signal
        // that plain Ruby / Sinagle-file Sinatra projects won't trip.
        dir.join("bin").join("rails").exists() || dir.join("config").join("application.rb").exists()
    }

    fn default_start_command(&self, dir: &Path) -> Option<String> {
        // Prefer the project's bin stub (uses the app's bundled Rails); fall back
        // to `bundle exec rails`. Bind all interfaces so the host can reach it.
        if dir.join("bin").join("rails").exists() {
            Some("bin/rails server -b 0.0.0.0 -p ${PORT:-3000}".to_string())
        } else {
            Some("bundle exec rails server -b 0.0.0.0 -p ${PORT:-3000}".to_string())
        }
    }

    fn default_health_path(&self) -> &'static str {
        "none"
    }

    fn default_port_env(&self) -> &'static str {
        "PORT"
    }

    fn env_wiring(&self, endpoints: &[ServiceEndpoint]) -> BTreeMap<String, String> {
        conventional_env(endpoints)
    }
}
