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
    let mut params = json!({ "pane_id": pane_id, "source": "herdr-pacer", "tokens": tokens });
    if let Some(ttl) = ttl_ms {
        params["ttl_ms"] = json!(ttl);
    }
    call("pane.report_metadata", params).map(drop)
}
