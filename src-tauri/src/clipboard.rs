use webview2_com::{
    Microsoft::Web::WebView2::Win32::{
        ICoreWebView2Profile4, ICoreWebView2_13, COREWEBVIEW2_PERMISSION_KIND_CLIPBOARD_READ,
        COREWEBVIEW2_PERMISSION_STATE_ALLOW,
    },
    SetPermissionStateCompletedHandler,
};
use windows::core::{w, Interface};

pub async fn attach(window: &tauri::WebviewWindow) -> Result<(), String> {
    let (send, receive) = tokio::sync::oneshot::channel();
    let (grant_send, grant_receive) = tokio::sync::oneshot::channel();
    window
        .with_webview(move |view| {
            let result = (|| unsafe {
                let core = view.controller().CoreWebView2()?;
                let profile = core.cast::<ICoreWebView2_13>()?.Profile()?;
                // Persist only this exact origin in the app's dedicated WebView2
                // profile. A request-only grant with SavesInProfile(false) can
                // allow the call yet return empty text in current runtimes.
                // Parsec retains the native Clipboard API, focus checks and
                // browser Permissions Policy; no clipboard bridge is exposed.
                profile.cast::<ICoreWebView2Profile4>()?.SetPermissionState(
                    COREWEBVIEW2_PERMISSION_KIND_CLIPBOARD_READ,
                    w!("https://web.parsec.app"),
                    COREWEBVIEW2_PERMISSION_STATE_ALLOW,
                    &SetPermissionStateCompletedHandler::create(Box::new(move |error| {
                        let result = error
                            .ok()
                            .map_err(|_| "Cannot grant Parsec clipboard permission".to_string());
                        let _ = grant_send.send(result);
                        Ok(())
                    })),
                )
            })()
            .map_err(|_| "Cannot initialize Parsec clipboard permission".to_string());
            let _ = send.send(result);
        })
        .map_err(|_| "Cannot access the Parsec webview")?;
    tokio::time::timeout(std::time::Duration::from_secs(5), async {
        receive
            .await
            .map_err(|_| "Clipboard permission initialization was interrupted")??;
        grant_receive
            .await
            .map_err(|_| "Clipboard permission grant was interrupted")?
    })
    .await
    .map_err(|_| "Clipboard permission initialization timed out")?
}
