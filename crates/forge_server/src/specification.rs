use serde::Serialize;

#[derive(Serialize)]
pub struct Environment {
    operating_system: String,
    current_working_dir: String,
}

impl Default for Environment {
    fn default() -> Self {
        Self {
            operating_system: std::env::consts::OS.to_string(),
            current_working_dir: std::env::current_dir()
                .expect("Failed to get current working directory")
                .to_string_lossy()
                .to_string(),
        }
    }
}
