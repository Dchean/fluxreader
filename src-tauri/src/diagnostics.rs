use crate::error::AppError;
use serde::Serialize;
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

const MAX_BYTES: u64 = 256 * 1024;
const FILE_COUNT: usize = 3;
static LOG: OnceLock<DiagnosticLog> = OnceLock::new();

pub struct DiagnosticLog {
    directory: PathBuf,
    max_bytes: u64,
    file_count: usize,
    writer: Mutex<()>,
}

#[derive(Serialize)]
struct Entry<'a> {
    time: String,
    activity_id: u64,
    source: &'a str,
    outcome: &'a str,
    error_class: &'a str,
}

impl DiagnosticLog {
    pub fn new(directory: &Path, max_bytes: u64, file_count: usize) -> std::io::Result<Self> {
        fs::create_dir_all(directory)?;
        Ok(Self {
            directory: directory.to_path_buf(),
            max_bytes: max_bytes.max(512),
            file_count: file_count.clamp(1, 5),
            writer: Mutex::new(()),
        })
    }

    pub fn record(
        &self,
        activity_id: u64,
        source: &str,
        outcome: &str,
        error: Option<&AppError>,
    ) -> std::io::Result<()> {
        let entry = Entry {
            time: chrono::Utc::now().to_rfc3339(),
            activity_id,
            source: safe_source(source),
            outcome: safe_outcome(outcome),
            error_class: error.map(error_class).unwrap_or("none"),
        };
        let mut bytes = serde_json::to_vec(&entry)?;
        bytes.push(b'\n');
        let _guard = self
            .writer
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        let current = self.path(0);
        if fs::metadata(&current).map(|meta| meta.len()).unwrap_or(0) + bytes.len() as u64
            > self.max_bytes
        {
            let oldest = self.path(self.file_count - 1);
            if oldest.exists() {
                fs::remove_file(oldest)?;
            }
            for index in (1..self.file_count).rev() {
                let previous = self.path(index - 1);
                if previous.exists() {
                    fs::rename(previous, self.path(index))?;
                }
            }
        }
        OpenOptions::new()
            .create(true)
            .append(true)
            .open(current)?
            .write_all(&bytes)
    }

    fn path(&self, index: usize) -> PathBuf {
        self.directory.join(format!("sync-{index}.log"))
    }
}

fn safe_source(value: &str) -> &str {
    match value {
        "manual" | "auto" | "focus" | "feeds" | "states" | "discovery" | "push"
        | "subscriptions" | "shutdown" | "refresh" => value,
        _ => "unknown",
    }
}

fn safe_outcome(value: &str) -> &str {
    match value {
        "started" | "success" | "partial" | "error" | "cancelled" | "not_configured"
        | "event_failed" | "stopped" | "timeout" => value,
        _ => "unknown",
    }
}

fn error_class(error: &AppError) -> &str {
    if error.is_auth_failure() {
        return "auth";
    }
    match error.code.as_str() {
        "auth" | "network" | "protocol" | "notConnected" | "staleSession" | "internal" | "db"
        | "migration" | "json" | "parse" | "partial" | "credentialCorrupt"
        | "credentialDecrypt" | "syncCancelled" => &error.code,
        _ => "other",
    }
}

pub fn install(directory: &Path) -> std::io::Result<()> {
    let logger = DiagnosticLog::new(directory, MAX_BYTES, FILE_COUNT)?;
    let _ = LOG.set(logger);
    Ok(())
}

// Note: 发布诊断只写受控分类，拒绝把错误正文、URL、凭据写到磁盘 — 见 .agents/notes/implemented/architecture/2026-11-05-同步运行生命周期与诊断.md
pub fn record(activity_id: u64, source: &str, outcome: &str, error: Option<&AppError>) {
    if let Some(logger) = LOG.get() {
        if logger.record(activity_id, source, outcome, error).is_err() {
            log::warn!("同步诊断日志写入失败");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn diagnostic_records_are_bounded_and_never_copy_untrusted_strings() {
        let directory = std::env::temp_dir().join(format!(
            "fluxreader-diagnostics-{}-{}",
            std::process::id(),
            chrono::Utc::now().timestamp_nanos_opt().unwrap()
        ));
        let logger = DiagnosticLog::new(&directory, 512, 3).unwrap();
        let secret = "Authorization: Bearer secret; https://user:password@example.test/?token=private <p>body</p>";
        let error = AppError::new(secret, secret);
        for activity_id in 0..40 {
            logger
                .record(activity_id, secret, secret, Some(&error))
                .unwrap();
        }
        let files: Vec<_> = fs::read_dir(&directory)
            .unwrap()
            .map(Result::unwrap)
            .collect();
        assert_eq!(files.len(), 3);
        for file in files {
            assert!(file.metadata().unwrap().len() <= 512);
            let text = fs::read_to_string(file.path()).unwrap();
            for forbidden in [
                "Authorization",
                "secret",
                "https:",
                "password",
                "token",
                "<p>",
            ] {
                assert!(!text.contains(forbidden), "{forbidden}");
            }
            for line in text.lines() {
                let parsed: serde_json::Value = serde_json::from_str(line).unwrap();
                assert_eq!(parsed["source"], "unknown");
                assert_eq!(parsed["error_class"], "other");
            }
        }
        fs::remove_dir_all(directory).unwrap();
    }
}
