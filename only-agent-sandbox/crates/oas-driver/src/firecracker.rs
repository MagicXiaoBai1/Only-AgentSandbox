//! Firecracker API 薄 client（HTTP/1.1 over UDS）。
//!
//! 手写极简 HTTP/1.1——firecracker API 就几个 PUT/GET，不值得引 hyper。所有调用走
//! `$jail_root/run/firecracker.socket`。shim 以 root 跑，可访问 jailer uid 拥有的 socket。

use std::io::{Read, Write};
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::time::Duration;

use crate::DriverError;

/// firecracker API 客户端（连一个 VM 的 API socket）。
pub struct FirecrackerClient {
    sock: PathBuf,
}

impl FirecrackerClient {
    pub fn new(sock: impl Into<PathBuf>) -> Self {
        Self { sock: sock.into() }
    }

    fn request(&self, method: &str, path: &str, body: &str) -> Result<(u16, String), DriverError> {
        let mut s = UnixStream::connect(&self.sock).map_err(|e| {
            DriverError::FirecrackerApi(format!("connect {}: {e}", self.sock.display()))
        })?;
        let _ = s.set_read_timeout(Some(Duration::from_secs(10)));
        let req = format!(
            "{method} {path} HTTP/1.1\r\nHost: localhost\r\nContent-Type: application/json\r\nContent-Length: {len}\r\nConnection: close\r\n\r\n{body}",
            len = body.len()
        );
        s.write_all(req.as_bytes())?;
        // firecracker 用 HTTP/1.1 keep-alive，响应后不关连接——read_to_end 会一直阻塞到
        // read_timeout。故先读到响应头结束（\r\n\r\n），再按 Content-Length 精确读齐 body
        // （错误响应才有 body；204 成功无 body），不依赖连接关闭。
        let mut buf = Vec::with_capacity(512);
        let mut chunk = [0u8; 512];
        loop {
            let n = s.read(&mut chunk)?;
            if n == 0 {
                return Err(DriverError::FirecrackerApi(
                    "firecracker closed connection before sending response headers".into(),
                ));
            }
            buf.extend_from_slice(&chunk[..n]);
            if buf.windows(4).any(|w| w == b"\r\n\r\n") {
                break;
            }
        }
        let header_end = buf
            .windows(4)
            .position(|w| w == b"\r\n\r\n")
            .unwrap_or(buf.len());
        let headers = String::from_utf8_lossy(&buf[..header_end]).to_string();
        let mut body_buf = buf[header_end + 4..].to_vec();

        let content_length = headers
            .lines()
            .find_map(|l| {
                l.to_lowercase()
                    .strip_prefix("content-length:")
                    .and_then(|v| v.trim().parse::<usize>().ok())
            })
            .unwrap_or(0);
        while body_buf.len() < content_length {
            let n = s.read(&mut chunk)?;
            if n == 0 {
                break;
            }
            body_buf.extend_from_slice(&chunk[..n]);
        }
        let body_len = body_buf.len().min(content_length);
        let resp_body = String::from_utf8_lossy(&body_buf[..body_len]).to_string();

        // 状态行：`HTTP/1.1 204 No Content`
        let status_line = headers.lines().next().unwrap_or("");
        let code = status_line
            .split_whitespace()
            .nth(1)
            .and_then(|c| c.parse::<u16>().ok())
            .unwrap_or(0);
        Ok((code, resp_body))
    }

    /// `PUT /logger`。失败仅记日志（非致命）。
    pub fn put_logger(&self, log_path: &Path) {
        let body = format!(
            "{{\"log_path\":\"{}\",\"level\":\"Debug\",\"show_level\":true,\"show_log_origin\":true}}",
            log_path.display()
        );
        if let Err(e) = self.request("PUT", "/logger", &body) {
            tracing::warn!(target: "oas-shim", "set firecracker logger failed (proceeding): {e}");
        }
    }

    /// `PUT /snapshot/load`（resume_vm=true）。成功 → Ok，失败 → Err 带 状态码 + body。
    pub fn put_snapshot_load(
        &self,
        vmstate_jail: &str,
        mem_jail: &str,
    ) -> Result<(), DriverError> {
        let body = format!(
            "{{\"snapshot_path\":\"{vmstate_jail}\",\"mem_backend\":{{\"backend_type\":\"File\",\"backend_path\":\"{mem_jail}\"}},\"track_dirty_pages\":false,\"resume_vm\":true}}"
        );
        let (code, resp_body) = self.request("PUT", "/snapshot/load", &body)?;
        if (200..300).contains(&code) {
            Ok(())
        } else {
            Err(DriverError::Snapshot(format!(
                "snapshot/load HTTP {code}: {resp_body}"
            )))
        }
    }

    /// `GET /`。用于身份校验时确认候选 pid 确实是 firecracker 实例。
    pub fn get_info(&self) -> Result<bool, DriverError> {
        match self.request("GET", "/", "") {
            Ok((code, _)) => Ok((200..300).contains(&code)),
            Err(_) => Ok(false),
        }
    }
}
