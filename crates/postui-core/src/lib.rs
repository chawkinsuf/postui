/// Working name; final app name TBD (spec header).
pub const APP_NAME: &str = "postui";

/// The app's config directory, shared by every path helper (config.toml,
/// keys.toml, themes/, the default project). postui is a terminal tool, so
/// it follows the XDG convention on every Unix — including macOS, where
/// `directories::ProjectDirs` would otherwise point at
/// `~/Library/Application Support`: there it resolves
/// `$XDG_CONFIG_HOME/postui` (when set to an absolute path) or
/// `~/.config/postui`. Other platforms keep `ProjectDirs`' native answer.
pub fn config_dir() -> Option<std::path::PathBuf> {
    #[cfg(target_os = "macos")]
    {
        use std::path::PathBuf;
        let base = std::env::var_os("XDG_CONFIG_HOME")
            .map(PathBuf::from)
            .filter(|p| p.is_absolute())
            .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))?;
        Some(base.join(APP_NAME))
    }
    #[cfg(not(target_os = "macos"))]
    {
        directories::ProjectDirs::from("", "", APP_NAME).map(|d| d.config_dir().to_path_buf())
    }
}

pub mod jq;
pub mod json;
pub mod migrate;
pub mod model;
pub mod order;
pub mod prepare;
pub mod project;
pub mod storage;
pub mod trash;
pub mod varedit;
pub mod varmodel;
pub mod vars;

#[cfg(test)]
mod tests {
    #[test]
    fn app_name_is_nonempty() {
        assert!(!super::APP_NAME.is_empty());
    }

    /// Every platform's config dir ends in the app's own folder — on Unix
    /// (macOS included) an XDG-style `…/.config/postui` (or
    /// `$XDG_CONFIG_HOME/postui`), never a bare base directory.
    #[test]
    fn config_dir_ends_with_the_app_folder() {
        let dir = super::config_dir().expect("a config dir resolves");
        assert_eq!(
            dir.file_name().and_then(|n| n.to_str()),
            Some(super::APP_NAME)
        );
    }
}
