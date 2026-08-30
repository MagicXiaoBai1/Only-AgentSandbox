//! 领域错误 → tonic::Status 映射 + 幂等删除助手（§3.1）。
//!
//! 注意：`OasError`（oas-manager）与 `tonic::Status` 都非本 crate 类型，孤儿规则
//! 禁止 `impl From<OasError> for tonic::Status`，故用自由函数 `to_status`。
//! 调用点用 `.map_err(error::to_status)?`。

use oas_manager::OasError;

/// 把 `OasError` 映射成 `tonic::Status`（§3.1 错误映射示例）。
pub fn to_status(e: OasError) -> tonic::Status {
    use tonic::Status;
    let msg = e.to_string();
    match e {
        OasError::NotFound(_) | OasError::ImageNotInList(_) => Status::not_found(msg),
        OasError::TypeMismatch(_) | OasError::InvalidArgument(_) => Status::invalid_argument(msg),
        OasError::Conflict(_) => Status::already_exists(msg),
        OasError::Unavailable(_) => Status::unavailable(msg),
        OasError::Internal(_) => Status::internal(msg),
    }
}

/// 幂等删除：`NotFound` 吞成 `Ok`，其余错误照常映射（§3.1 红线：
/// Stop*/Remove* 删不存在 = Ok，不让任何 handler 漏）。
///
/// 仅供返回 `Result<(), OasError>` 的删除型 Manager 方法使用。
pub fn swallow_not_found(r: Result<(), OasError>) -> Result<(), tonic::Status> {
    match r {
        Ok(()) => Ok(()),
        Err(OasError::NotFound(_)) => Ok(()),
        Err(e) => Err(to_status(e)),
    }
}
