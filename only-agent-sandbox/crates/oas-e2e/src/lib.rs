//! oas-e2e：端到端测试专用 crate（test-only）。
//!
//! 把真实 `OasManager`（+ 真实 `MemoryStore`）与手写 mock 的 driver/net/storage
//! 装配起来，经 `oas-cri` 的 UDS gRPC 通道跑 CRI 生命周期端到端用例。本身不产出
//! 任何产物，仅承载 `tests/` 下的集成测试与共享 mock。
