/// Working name; final app name TBD (spec header).
pub const APP_NAME: &str = "postui";

#[cfg(test)]
mod tests {
    #[test]
    fn app_name_is_nonempty() {
        assert!(!super::APP_NAME.is_empty());
    }
}
