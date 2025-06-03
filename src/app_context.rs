#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct AppContext {
    #[allow(dead_code)]
    pub name: String,
}
