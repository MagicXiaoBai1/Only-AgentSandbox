//! CRI 调用调试日志宏。
//!
//! 在 debug 级别下，为每个 CRI RPC 打印两行：调用时打印 API 名 + 入参，处理完打印返回值
//! （Ok 打响应体、Err 打 `Status`）。由 `oas-runtime` 的订阅器 `EnvFilter` 按级别过滤，
//! 非 debug 级别时这些 `tracing::debug!` 不产生输出，热路径零开销可忽略。
//!
//! 用法：
//! ```ignore
//! async fn version(&self, req: Request<pb::VersionRequest>) -> Result<Response<pb::VersionResponse>, Status> {
//!     let req = req.into_inner();
//!     cri_call!("Version", &req, async move {
//!         let v = self.mgr.version().await.map_err(to_status)?;
//!         Ok(Response::new(pb::VersionResponse { /* ... */ }))
//!     })
//! }
//! ```
//!
//! 说明：先以 `&req` 借用打印入参（借用随语句结束释放），再把 `async move { ... }` 作为
//! future 交由宏 `.await`。`async move` 接管 `req` 与 `self` 的所有权/引用，体内可用 `?`
//! 提前返回 `Err(Status)`——宏统一在 Ok/Err 两条路径上打返回日志。

/// 记录一次 CRI RPC：`CRI call`（API 名 + 入参）→ 执行体 → `CRI return`（响应体或 Status）。
#[macro_export]
macro_rules! cri_call {
    ($api:literal, $req:expr, $body:expr) => {{
        tracing::debug!(target: "oas-cri", api = $api, req = ?$req, "CRI call");
        let __result = $body.await;
        match &__result {
            Ok(__resp) => tracing::debug!(target: "oas-cri", api = $api, resp = ?__resp, "CRI return"),
            Err(__err) => tracing::debug!(target: "oas-cri", api = $api, err = %__err, "CRI return"),
        }
        __result
    }};
}
