use crate::utils::join_path_segments;

#[derive(Debug, Clone)]
pub struct AppContext {
    pub name: String,
    pub mount_path: String,
    pub parent_app: Option<Box<AppContext>>,
}

impl AppContext {
    pub fn resolve_full_path(&self, path: &str) -> String {
        println!("app resolve_full_path {:?}", self);
        let mut path_segments = Vec::new();

        // Add the specific endpoint path
        if !path.is_empty() {
            path_segments.push(path.to_string());
        }

        // Add this app's mount path
        if !self.mount_path.is_empty() {
            path_segments.insert(0, self.mount_path.clone());
        }

        // Add parent app paths by inserting at the beginning
        let mut current_app = self.parent_app.as_deref();
        while let Some(app) = current_app {
            if !app.mount_path.is_empty() {
                path_segments.insert(0, app.mount_path.clone());
            }
            current_app = app.parent_app.as_deref();
        }
        println!("AppContext --> {:?}", path_segments);
        join_path_segments(path_segments)
    }
}
