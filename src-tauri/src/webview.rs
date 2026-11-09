//! WebView2 browser arguments for WebDriver runs on Windows (#133, D-039).
//!
//! Microsoft Edge WebDriver attaches to a WebView2 app by setting
//! `WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS` (with `--remote-debugging-port=0`) before launching it.
//! Microsoft documents that variable as appended to the app's own arguments, but the WebView2
//! runtime on CI (156.0.4314.58) drops it whenever the app passes arguments of its own, and wry
//! always does. So in debug builds Queue appends the variable itself. Release builds never read
//! it: `main_window_args` is only called under `cfg!(all(windows, debug_assertions))`.

/// The arguments wry 0.57 passes when the app gives none (`webview2::create_environment`, with
/// autoplay on, as Queue's window has it). Passing any arguments replaces these, so they're kept.
pub const WRY_DEFAULT_ARGS: &str = "--disable-features=msWebOOUI,msPdfOOUI,msSmartScreenProtection --autoplay-policy=no-user-gesture-required";

/// The main window's WebView2 arguments given `WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS`: wry's
/// defaults followed by the variable's switches, or `None` (wry's own defaults) when it's unset
/// or blank.
pub fn main_window_args(extra: Option<&str>) -> Option<String> {
    let extra = extra?.trim();
    if extra.is_empty() {
        return None;
    }
    Some(format!("{WRY_DEFAULT_ARGS} {extra}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unset_or_blank_keeps_wrys_defaults() {
        assert_eq!(main_window_args(None), None);
        assert_eq!(main_window_args(Some("")), None);
        assert_eq!(main_window_args(Some("  \t ")), None);
    }

    #[test]
    fn the_variable_follows_wrys_defaults() {
        let args =
            main_window_args(Some(" --remote-debugging-port=0 --enable-automation ")).unwrap();
        assert_eq!(
            args,
            format!("{WRY_DEFAULT_ARGS} --remote-debugging-port=0 --enable-automation")
        );
        // WebView2 merges repeated --disable-features, so msedgedriver's own list doesn't
        // undo wry's (Microsoft: "except for disabled and enabled features").
        assert!(
            args.starts_with("--disable-features=msWebOOUI,msPdfOOUI,msSmartScreenProtection ")
        );
    }
}
