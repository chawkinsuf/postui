/// Working name; final app name TBD (spec header).
pub const APP_NAME: &str = "postui";

pub mod json;
pub mod model;
pub mod prepare;
pub mod project;
pub mod storage;
pub mod varmodel;
pub mod vars;

#[cfg(test)]
mod tests {
    #[test]
    fn app_name_is_nonempty() {
        assert!(!super::APP_NAME.is_empty());
    }
}
