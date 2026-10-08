//! Herdr's socket API: one JSON request per connection, one JSON line back.

use serde_json::{json, Value};
use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;
use std::time::Duration;

#[derive(Debug)]
pub struct Error(pub String);

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

pub type Result<T> = std::result::Result<T, Error>;

pub fn call(method: &str, params: Value) -> Result<Value> {
    let path = std::env::var("HERDR_SOCKET_PATH")
        .map_err(|_| Error("HERDR_SOCKET_PATH is not set; run this from inside Herdr".into()))?;
    let stream = UnixStream::connect(&path).map_err(|e| Error(format!("{path}: {e}")))?;
    stream.set_read_timeout(Some(Duration::from_secs(10))).ok();
    let request = json!({ "id": "herdr-pacer", "method": method, "params": params });
    (&stream)
        .write_all(format!("{request}\n").as_bytes())
        .map_err(|e| Error(format!("{method}: {e}")))?;
    let mut line = String::new();
    BufReader::new(&stream)
        .read_line(&mut line)
        .map_err(|e| Error(format!("{method}: {e}")))?;
    let mut reply: Value =
        serde_json::from_str(&line).map_err(|e| Error(format!("{method}: {e}")))?;
    if let Some(err) = reply.get("error") {
        let message = err["message"].as_str().unwrap_or("error");
        return Err(Error(format!("{method}: {message}")));
    }
    Ok(reply["result"].take())
}

/// The pane's tokens, set or cleared (`None`), under our metadata source.
pub fn report_tokens(pane_id: &str, tokens: Value, ttl_ms: Option<u64>) -> Result<()> {
    report_metadata(pane_id, json!({ "tokens": tokens }), ttl_ms)
}

/// The pane's metadata under our source: tokens, and the agent's display name.
pub fn report_metadata(pane_id: &str, mut params: Value, ttl_ms: Option<u64>) -> Result<()> {
    params["pane_id"] = json!(pane_id);
    params["source"] = json!("herdr-pacer");
    if let Some(ttl) = ttl_ms {
        params["ttl_ms"] = json!(ttl);
    }
    call("pane.report_metadata", params).map(drop)
}

fn bin() -> std::path::PathBuf {
    std::env::var_os("HERDR_BIN_PATH").map(Into::into).unwrap_or_else(|| {
        let path = std::env::var_os("PATH").unwrap_or_default();
        std::env::split_paths(&path).map(|d| d.join("herdr")).find(|p| p.is_file()).unwrap_or_else(|| "herdr".into())
    })
}

/// Whether this Herdr parses the `glue` token option (herdr-glue.patch). Stock
/// Herdr rejects the whole config.toml over a field it does not know, so the
/// rows follow what the binary can parse. Cached against the binary's size
/// and mtime: a Herdr update — its self-update replaces a patched build with
/// a stock one — shows up as a new answer on the next hook.
pub fn glue() -> bool {
    let bin = bin();
    let id = std::fs::metadata(&bin).map(|m| format!("{} {:?}", m.len(), m.modified().ok())).unwrap_or_default();
    let cache = crate::usage::state_dir().join("herdr-glue");
    if let Some(hit) = std::fs::read_to_string(&cache).ok().and_then(|t| t.strip_prefix(&format!("{id}\n")).map(|v| v == "1")) {
        return hit;
    }
    // ask the binary's own parser: a config holding one glued token
    let probe = crate::usage::state_dir().join(format!("glue-probe-{}.toml", std::process::id()));
    let wrote = std::fs::write(&probe, "[ui.sidebar.agents]\nrows = [[{ token = \"$a\" }, { token = \"$b\", glue = true }]]\n");
    // a missing file checks out fine, so no file, no glue
    let glue = wrote.is_ok() && std::process::Command::new(&bin)
        .args(["config", "check"])
        .env("HERDR_CONFIG_PATH", &probe)
        .output()
        .is_ok_and(|o| o.status.success());
    let _ = std::fs::remove_file(probe);
    if !id.is_empty() {
        let _ = std::fs::write(cache, format!("{id}\n{}", glue as u8));
    }
    glue
}
