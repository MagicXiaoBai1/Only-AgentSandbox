//! 不支持方法的统一 `unimplemented` 响应（§3.1 `unsupported.rs`）。
//!
//! MVP 非目标（§0）：Exec/Attach/PortForward/ExecSync、在线资源热调、CRI stats、
//! checkpoint、events、metrics 等全部返回 `Status::unimplemented`。

/// 返回 `Status::unimplemented`，提示某方法在 MVP 不支持。
pub fn unimpl(name: &str) -> tonic::Status {
    tonic::Status::unimplemented(format!("{name} is not supported (oas MVP non-target)"))
}
