//! 协议无关的公共设施: VM 抽象、ttrpc server 装配骨架、start 握手原语。
//!
//! `sandbox` (2.x) 与 `task` (1.6.33) 两条协议路径共用本模块。

pub mod bundle;
pub mod server;
pub mod start;
pub mod vm;