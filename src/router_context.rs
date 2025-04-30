#[derive(Debug, Clone)]
pub struct RouterContext {
    pub prefix: String,
    pub parent: Option<Box<RouterContext>>,
}

impl RouterContext {
    pub fn resolve_full_path(&self, path: &str) -> String {
        let mut full_path = String::new();
        let mut current = Some(self);

        // Build path from root to current router
        while let Some(ctx) = current {
            full_path = format!("{}{}", ctx.prefix, full_path);
            current = ctx.parent.as_deref();
        }

        // Add the specific endpoint path
        format!("{}{}", full_path, path)
    }
}
