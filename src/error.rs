use axum::{http::StatusCode, response::{IntoResponse, Response}, Json};
use serde_json::json;

#[derive(Debug)]
pub struct AppError { pub status: StatusCode, pub message: String }
impl AppError {
    pub fn bad(message: impl Into<String>) -> Self { Self { status: StatusCode::BAD_REQUEST, message: message.into() } }
    pub fn upstream(message: impl Into<String>) -> Self { Self { status: StatusCode::BAD_GATEWAY, message: message.into() } }
    pub fn unauthorized() -> Self { Self { status: StatusCode::UNAUTHORIZED, message: "请先登录".into() } }
}
impl std::fmt::Display for AppError { fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result { write!(f, "{}", self.message) } }
impl std::error::Error for AppError {}
impl IntoResponse for AppError { fn into_response(self) -> Response { (self.status, Json(json!({"error": self.message}))).into_response() } }
impl From<anyhow::Error> for AppError {
    fn from(e: anyhow::Error) -> Self { tracing::error!(error = %e, "local operation failed"); Self { status: StatusCode::INTERNAL_SERVER_ERROR, message: "本地操作失败，请查看服务日志".into() } }
}
pub type Result<T> = std::result::Result<T, AppError>;

