/// Whether apps use the dark theme (Settings > Personalization > Colors), which Explorer
/// follows. Without the value Windows is light.
pub fn dark() -> bool {
    use winreg::enums::HKEY_CURRENT_USER;
    use winreg::RegKey;
    RegKey::predef(HKEY_CURRENT_USER)
        .open_subkey(r"Software\Microsoft\Windows\CurrentVersion\Themes\Personalize")
        .and_then(|k| k.get_value::<u32, _>("AppsUseLightTheme"))
        .is_ok_and(|light| light == 0)
}

/// The tree view's visual style class for the theme.
pub fn tree(dark: bool) -> &'static str {
    if dark {
        "DarkMode_Explorer::TreeView"
    } else {
        "Explorer::TreeView"
    }
}
