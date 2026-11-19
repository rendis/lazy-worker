pub const SUPPORTED: bool = false;

pub fn accessibility_status() -> &'static str {
    "unsupported"
}

pub fn request_accessibility() -> Result<(), &'static str> {
    Err("accessibilityRequestFailed")
}

pub struct Worker;

impl Worker {
    pub fn start() -> Result<Self, &'static str> {
        Err("unsupported")
    }

    pub fn is_running(&mut self) -> bool {
        false
    }
}
