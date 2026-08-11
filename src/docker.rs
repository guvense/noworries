//! Thin helpers around the `docker` CLI.

use std::process::{Command, Stdio};

pub struct Output {
    pub code: i32,
    pub stdout: String,
    pub stderr: String,
}

/// Run `docker <args>` capturing stdout/stderr. Never panics.
pub fn capture(args: &[String]) -> Output {
    match Command::new("docker")
        .args(args)
        .stdin(Stdio::null())
        .output()
    {
        Ok(o) => Output {
            code: o.status.code().unwrap_or(-1),
            stdout: String::from_utf8_lossy(&o.stdout).into_owned(),
            stderr: String::from_utf8_lossy(&o.stderr).into_owned(),
        },
        Err(e) => Output {
            code: -1,
            stdout: String::new(),
            stderr: e.to_string(),
        },
    }
}

/// Run `docker <args>` with inherited stdio (progress visible). Returns exit code.
pub fn stream(args: &[String]) -> i32 {
    Command::new("docker")
        .args(args)
        .stdin(Stdio::null())
        .status()
        .map(|s| s.code().unwrap_or(-1))
        .unwrap_or(-1)
}
