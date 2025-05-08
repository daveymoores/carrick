use crate::app_context::AppContext;
use crate::utils::join_path_segments;

#[derive(Debug, Clone)]
pub struct RouterContext {
    pub name: String,                              // Variable name for this router
    pub prefix: String,                            // Path prefix for this router
    pub parent_app: Option<Box<AppContext>>,       // App this router is mounted on
    pub parent_router: Option<Box<RouterContext>>, // Parent router (if mounted on another router)
}

impl RouterContext {
    pub fn resolve_full_path(&self, path: &str) -> String {
        println!("router resolve_full_path {:?}", self);
        let mut path_segments = Vec::new();

        // First, add the specific endpoint path
        if !path.is_empty() {
            path_segments.push(path.to_string());
        }

        // Then add this router's prefix
        if !self.prefix.is_empty() {
            path_segments.insert(0, self.prefix.clone());
        }

        // Add parent router prefixes by inserting at the beginning
        let mut current_router = self.parent_router.as_deref();
        while let Some(router) = current_router {
            if !router.prefix.is_empty() {
                path_segments.insert(0, router.prefix.clone());
            }
            current_router = router.parent_router.as_deref();
        }

        // Add parent app paths if this router is mounted on an app
        if let Some(app) = self.parent_app.as_deref() {
            let app_path = app.resolve_full_path("");
            if !app_path.is_empty() {
                path_segments.insert(0, app_path);
            }
        }

        join_path_segments(path_segments)
    }
}
