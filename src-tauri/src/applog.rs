//! Append-only JSON-lines action log on its own writer thread, so file I/O
//! never delays a key press. Rotates at 5 MB, keeping 3 old files.

use std::io::Write;
use std::path::PathBuf;
use std::sync::mpsc::{Sender, channel};

const MAX_BYTES: u64 = 5 * 1024 * 1024;
const KEEP: u32 = 3;
const REDACT: &[&str] = &["passcode", "password", "token"];

#[derive(Clone)]
pub struct Log(Sender<serde_json::Value>);

fn redact(v: &mut serde_json::Value) {
    match v {
        serde_json::Value::Object(map) => {
            for (k, val) in map.iter_mut() {
                if REDACT.iter().any(|r| k.eq_ignore_ascii_case(r)) {
                    *val = serde_json::Value::String("[redacted]".into());
                } else {
                    redact(val);
                }
            }
        }
        serde_json::Value::Array(items) => items.iter_mut().for_each(redact),
        _ => {}
    }
}

fn rotate(path: &PathBuf) {
    for i in (1..KEEP).rev() {
        let _ = std::fs::rename(path.with_extension(format!("log.{i}")), path.with_extension(format!("log.{}", i + 1)));
    }
    let _ = std::fs::rename(path, path.with_extension("log.1"));
}

impl Log {
    pub fn start(path: PathBuf) -> Self {
        let (tx, rx) = channel::<serde_json::Value>();
        std::thread::spawn(move || {
            if let Some(dir) = path.parent() {
                let _ = std::fs::create_dir_all(dir);
            }
            for mut line in rx {
                redact(&mut line);
                if std::fs::metadata(&path).map(|m| m.len() > MAX_BYTES).unwrap_or(false) {
                    rotate(&path);
                }
                let ts = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis()).unwrap_or(0);
                if let serde_json::Value::Object(map) = &mut line {
                    map.insert("tsMs".into(), serde_json::json!(ts));
                }
                if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(&path) {
                    // serde_json escapes quotes and newlines in titles and labels.
                    let _ = writeln!(f, "{line}");
                }
            }
        });
        Log(tx)
    }

    pub fn write(&self, line: serde_json::Value) {
        let _ = self.0.send(line);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn redacts_secret_fields_at_any_depth() {
        let mut v = serde_json::json!({ "a": { "Password": "x", "list": [{ "token": "y" }] }, "label": "ok" });
        redact(&mut v);
        assert_eq!(v, serde_json::json!({ "a": { "Password": "[redacted]", "list": [{ "token": "[redacted]" }] }, "label": "ok" }));
    }
}
