pub mod admin;
pub mod backup;
pub mod config;
pub mod domain;
pub mod error;
pub mod export;
pub mod http;
pub mod persistence;

pub fn application_name() -> &'static str {
    "Sylux"
}

#[cfg(test)]
mod tests {
    #[test]
    fn exposes_product_name() {
        assert_eq!(super::application_name(), "Sylux");
    }
}
