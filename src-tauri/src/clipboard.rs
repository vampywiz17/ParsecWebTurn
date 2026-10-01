use webview2_com::{
    Microsoft::Web::WebView2::Win32::{
        ICoreWebView2PermissionRequestedEventArgs3, COREWEBVIEW2_PERMISSION_KIND,
        COREWEBVIEW2_PERMISSION_KIND_CLIPBOARD_READ, COREWEBVIEW2_PERMISSION_STATE_ALLOW,
        COREWEBVIEW2_PERMISSION_STATE_DENY,
    },
    PermissionRequestedEventHandler,
};
use windows::core::{Interface, PWSTR};

fn parsec_origin(value: &str) -> bool {
    url::Url::parse(value)
        .is_ok_and(|url| url.origin().ascii_serialization() == "https://web.parsec.app")
}

fn permitted(request: &str, top_level: &str) -> bool {
    parsec_origin(request) && parsec_origin(top_level)
}

pub async fn attach(window: &tauri::WebviewWindow) -> Result<(), String> {
    let (send, receive) = tokio::sync::oneshot::channel();
    window
        .with_webview(move |view| {
            let result = (|| unsafe {
                let core = view.controller().CoreWebView2()?;
                let mut token = 0;
                core.add_PermissionRequested(
                    &PermissionRequestedEventHandler::create(Box::new(|sender, args| {
                        let Some(args) = args else { return Ok(()) };
                        let mut kind = COREWEBVIEW2_PERMISSION_KIND::default();
                        args.PermissionKind(&mut kind)?;
                        if kind != COREWEBVIEW2_PERMISSION_KIND_CLIPBOARD_READ {
                            return Ok(()); // Preserve WebView2 defaults for every other permission.
                        }
                        // Never persist a grant: validate both the requesting origin and
                        // the top-level page on every request, including embedded frames.
                        args.cast::<ICoreWebView2PermissionRequestedEventArgs3>()?
                            .SetSavesInProfile(false)?;
                        let mut uri = PWSTR::null();
                        args.Uri(&mut uri)?;
                        let request = webview2_com::take_pwstr(uri);
                        let top_level = sender.and_then(|core| {
                            let mut uri = PWSTR::null();
                            core.Source(&mut uri).ok()?;
                            Some(webview2_com::take_pwstr(uri))
                        });
                        let allowed = top_level
                            .as_deref()
                            .is_some_and(|top| permitted(&request, top));
                        args.SetState(if allowed {
                            COREWEBVIEW2_PERMISSION_STATE_ALLOW
                        } else {
                            COREWEBVIEW2_PERMISSION_STATE_DENY
                        })
                    })),
                    &mut token,
                )
            })()
            .map_err(|_| "Cannot initialize Parsec clipboard permission".to_string());
            let _ = send.send(result);
        })
        .map_err(|_| "Cannot access the Parsec webview")?;
    tokio::time::timeout(std::time::Duration::from_secs(5), receive)
        .await
        .map_err(|_| "Clipboard permission initialization timed out")?
        .map_err(|_| "Clipboard permission initialization was interrupted")?
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clipboard_read_requires_both_exact_parsec_origins() {
        let parsec = "https://web.parsec.app/";
        assert!(permitted("https://web.parsec.app/client", parsec));
        for other in [
            "https://example.com/",
            "https://web.parsec.app.example.com/",
            "http://web.parsec.app/",
            "https://web.parsec.app:444/",
            "data:text/html,test",
            "not a URL",
        ] {
            assert!(!permitted(other, parsec));
            assert!(!permitted(parsec, other));
        }
    }
}
