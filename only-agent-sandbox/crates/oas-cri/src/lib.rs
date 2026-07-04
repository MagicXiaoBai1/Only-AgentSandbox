//! ① API / CRI 层 (oas-cri) —— 纯翻译：proto ⇄ 内部模型、幂等短路、领域错误→Status。
//!
//! 不持有业务状态，全部委托给 `oas_manager::Manager`（§3.1）。

pub mod runtime {
    pub mod v1 {
        tonic::include_proto!("runtime.v1");
    }
}

mod convert;
mod error;
mod unsupported;

#[macro_use]
mod log;

mod image_svc;
mod runtime_svc;

pub use image_svc::ImageSvc;
pub use runtime_svc::RuntimeSvc;
