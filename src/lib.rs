//! Sylux: dependable private storage (S1 PoC foundation).

pub mod config;
pub mod domain;
pub mod error;
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
