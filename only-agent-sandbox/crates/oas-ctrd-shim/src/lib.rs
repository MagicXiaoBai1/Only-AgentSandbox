//! oas-ctrld-shim: OAS 作为 containerd 下层 runtime 的 v2 shim.

//! 双协议单二进制（见 `@docs/adr/0001`）：同一个 `containerd-oas-v2` 同时注册
//! Sandbox (2.x) 与 Task (1.6.33) 两条 ttrpc service，被哪版 containerd 驱动就只走对应一条。

//! 模块三分区，删改任一协议锁在自己文件夹内：
//!   - `common`: 协议无关（VM 抽象、ttrpc server 装配、start 握手）。
//!   - `sandbox`: containerd 2.x Sandbox service。
//!   - `task`: containerd 1.6.33 Task service。

//! 以 lib 形式导出，供 `main.rs` (bin) 与 `tests/`（进程内 e2e）共用。

pub mod common;
pub mod sandbox;
pub mod task;