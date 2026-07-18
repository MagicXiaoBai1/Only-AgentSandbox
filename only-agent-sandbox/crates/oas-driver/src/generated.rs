//! ttrpc 生成代码（build.rs 由 `proto/shim.proto` 生成）。
//!
//! `shim` 模块含消息类型，`shim_ttrpc` 含 `Shim` service trait + `ShimClient` +
//! `create_shim` 服务注册函数。`shim_ttrpc` 内部以 `super::shim::...` 引用消息，
//! 故两者须为同一父模块下的兄弟。

pub mod shim {
    include!(concat!(env!("OUT_DIR"), "/shim.rs"));
}

pub mod shim_ttrpc {
    include!(concat!(env!("OUT_DIR"), "/shim_ttrpc.rs"));
}
