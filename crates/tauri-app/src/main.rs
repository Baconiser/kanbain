// Prevents additional console window on Windows in release, DO NOT REMOVE!!
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use std::sync::Arc;

use async_trait::async_trait;
use services::services::{
    config::load_config_from_file,
    notification::{NotificationService, PushNotifier, set_global_push_notifier},
};
#[cfg(target_os = "macos")]
use tauri::Manager;
use tauri_plugin_notification::NotificationExt;
use tauri_plugin_opener::OpenerExt;
use tokio_util::sync::CancellationToken;
use tracing_subscriber::{EnvFilter, prelude::*};
use utils::assets::config_path;
use uuid::Uuid;

#[cfg(target_os = "linux")]
mod linux_notifications;
#[cfg(target_os = "macos")]
mod macos_notifications;
#[cfg(target_os = "windows")]
mod windows_notifications;

/// Native push notifier for backend-initiated notifications.
/// Uses platform-native APIs with click handling where available,
/// falls back to `tauri-plugin-notification` otherwise.
struct TauriNotifier {
    app_handle: tauri::AppHandle,
}

/// Whether platform-native notifications with click handling are available.
fn use_native_notifications() -> bool {
    #[cfg(target_os = "macos")]
    return macos_notifications::is_available();
    #[cfg(target_os = "windows")]
    return windows_notifications::is_available();
    #[cfg(target_os = "linux")]
    return linux_notifications::is_available();
    #[cfg(not(any(target_os = "macos", target_os = "windows", target_os = "linux")))]
    false
}

/// Show a notification using the platform-native API (with click handling).
fn show_native_notification(title: &str, body: &str, deeplink_path: Option<&str>) {
    #[cfg(target_os = "macos")]
    macos_notifications::show_notification(title, body, deeplink_path);
    #[cfg(target_os = "windows")]
    windows_notifications::show_notification(title, body, deeplink_path);
    #[cfg(target_os = "linux")]
    linux_notifications::show_notification(title, body, deeplink_path);
}

#[tauri::command]
async fn show_system_notification(
    title: String,
    body: String,
    deeplink_path: Option<String>,
) -> Result<(), String> {
    if use_native_notifications() {
        show_native_notification(&title, &body, deeplink_path.as_deref());
        return Ok(());
    }

    // Fallback: generic NotificationService (e.g. macOS dev mode).
    let config = load_config_from_file(&config_path()).await;
    let notification_service = NotificationService::new(Arc::new(tokio::sync::RwLock::new(config)));
    notification_service.notify(&title, &body, None).await;
    Ok(())
}

#[tauri::command]
fn read_clipboard_text() -> Result<String, String> {
    let mut clipboard = arboard::Clipboard::new().map_err(|e| e.to_string())?;
    clipboard.get_text().map_err(|e| e.to_string())
}

#[async_trait]
impl PushNotifier for TauriNotifier {
    async fn send(&self, title: &str, message: &str, workspace_id: Option<Uuid>) {
        let deeplink_path = workspace_id.map(|id| format!("/workspaces/{id}"));

        if use_native_notifications() {
            show_native_notification(title, message, deeplink_path.as_deref());
            return;
        }

        // Fallback: tauri-plugin-notification (no click handling).
        if let Err(e) = self
            .app_handle
            .notification()
            .builder()
            .title(title)
            .body(message)
            .show()
        {
            tracing::warn!("Failed to send Tauri notification: {}", e);
        }
    }
}

fn main() {
    // Install rustls crypto provider before any TLS operations
    rustls::crypto::aws_lc_rs::default_provider()
        .install_default()
        .expect("Failed to install rustls crypto provider");

    let log_level = std::env::var("RUST_LOG").unwrap_or_else(|_| "info".to_string());
    let filter_string = format!(
        "warn,server={level},services={level},db={level},executors={level},deployment={level},local_deployment={level},utils={level},kanbain_tauri={level}",
        level = log_level
    );
    let env_filter = EnvFilter::try_new(filter_string).expect("Failed to create tracing filter");

    tracing_subscriber::registry()
        .with(tracing_subscriber::fmt::layer().with_filter(env_filter))
        .init();

    // Shared token so we can tell the server to shut down when the app quits.
    let shutdown_token = Arc::new(CancellationToken::new());
    let shutdown_token_for_event = shutdown_token.clone();

    // Reassigned on macOS only.
    #[cfg_attr(not(target_os = "macos"), allow(unused_mut))]
    let mut builder = tauri::Builder::default()
        .plugin(tauri_plugin_shell::init())
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_notification::init())
        .invoke_handler(tauri::generate_handler![
            show_system_notification,
            read_clipboard_text
        ]);

    // Unlock WKWebView's native refresh rate on macOS ProMotion / high-Hz displays.
    // On macOS 13–15 WKWebView caps rAF at 60fps; this plugin disables that cap
    // via WebKit's private _features API. No-op on macOS 26+ (cap removed by Apple).
    #[cfg(target_os = "macos")]
    {
        builder = builder.plugin(tauri_plugin_macos_fps::init());
    }

    builder
        .setup(move |app| {
            // Initialize platform-native notifications (request permission,
            // install click-handling delegates) before anything else.
            #[cfg(target_os = "macos")]
            macos_notifications::initialize(app.handle().clone());
            #[cfg(target_os = "windows")]
            windows_notifications::initialize(app.handle().clone());
            #[cfg(target_os = "linux")]
            linux_notifications::initialize(app.handle().clone());

            if cfg!(debug_assertions) {
                // Dev mode: frontend dev server (Vite) and backend are started
                // externally. Use WebviewUrl::External so that macOS WKWebView
                // renders with the same content scaling as the production build.
                let frontend_port =
                    std::env::var("FRONTEND_PORT").unwrap_or_else(|_| "3000".to_string());
                let dev_url = format!("http://localhost:{frontend_port}");
                tracing::info!("Running in dev mode — using external frontend/backend servers (devUrl={dev_url})");
                let window = create_window(
                    app,
                    tauri::WebviewUrl::External(dev_url.parse().unwrap()),
                )?;
                #[cfg(target_os = "macos")]
                {
                    disable_pinch_zoom(&window);
                    optimize_webview_performance(&window);
                }
                let _ = window;
            } else {
                // Production: start the Axum server first, then open the window
                // once it's ready so the user never sees a blank/error page.
                let app_handle = app.handle().clone();

                // Register native Tauri notifications before the server starts.
                set_global_push_notifier(Arc::new(TauriNotifier {
                    app_handle: app_handle.clone(),
                }));

                let token = shutdown_token.clone();
                tauri::async_runtime::spawn(async move {
                    match server::startup::start().await {
                        Ok(server_handle) => {
                            let url = server_handle.url();

                            // Create the window on the main thread — macOS
                            // silently drops windows created from async tasks.
                            let url_clone = url.clone();
                            let create_handle = app_handle.clone();
                            let _ = app_handle.run_on_main_thread(move || {
                                let webview_url =
                                    tauri::WebviewUrl::External(url_clone.parse().unwrap());
                                match create_window(&create_handle, webview_url) {
                                    Ok(window) => {
                                        #[cfg(target_os = "macos")]
                                        {
                                            disable_pinch_zoom(&window);
                                            optimize_webview_performance(&window);
                                        }
                                        let _ = window;
                                    }
                                    Err(e) => tracing::error!("Failed to create window: {e}"),
                                }
                            });
                            tracing::info!("Window opened at {url}");

                            // Wait for either the server to exit on its own or
                            // the external shutdown token to be cancelled.
                            let server_token = server_handle.shutdown_token();
                            tauri::async_runtime::spawn(async move {
                                token.cancelled().await;
                                server_token.cancel();
                            });

                            if let Err(e) = server_handle.serve().await {
                                tracing::error!("Server error: {e}");
                            }
                        }
                        Err(e) => {
                            tracing::error!("Server failed to start: {e}");
                        }
                    }
                });
            }

            Ok(())
        })
        .on_window_event(move |window, event| {
            match event {
                tauri::WindowEvent::CloseRequested { api, .. } => {
                    // Hide the window instead of closing it so the app keeps
                    // running in the background (agents/processes stay alive).
                    // The dock icon stays visible so users can click it to reopen.
                    api.prevent_close();
                    let _ = window.hide();
                }
                tauri::WindowEvent::Destroyed => {
                    // Only fires on actual app exit (e.g. Cmd+Q).
                    shutdown_token_for_event.cancel();
                }
                _ => {}
            }
        })
        .build(tauri::generate_context!())
        .expect("error while building tauri application")
        .run(move |_app, _event| {
            // macOS: clicking the dock icon when the window is hidden should reopen it.
            #[cfg(target_os = "macos")]
            if let tauri::RunEvent::Reopen { .. } = _event {
                show_window(_app);
            }
        });
}

/// Disable trackpad/touchpad pinch-to-zoom on macOS while keeping Cmd+/- zoom.
/// WKWebView handles magnification at the native level — JS `preventDefault()`
/// cannot block it.
#[cfg(target_os = "macos")]
fn disable_pinch_zoom(window: &tauri::WebviewWindow) {
    let _ = window.with_webview(|webview| unsafe {
        let wk: &objc2_web_kit::WKWebView = &*webview.inner().cast();
        wk.setAllowsMagnification(false);
    });
}

/// Enable GPU-accelerated compositing and drawing in WKWebView.
///
/// Embedded WKWebView may not have the same GPU acceleration defaults as Safari,
/// contributing to the observed performance gap (Chrome > Safari > Tauri).
/// Sets private WebKit preferences via NSKeyValueCoding (setValue:forKey:)
/// using the same raw msg_send pattern as tauri-plugin-macos-fps.
#[cfg(target_os = "macos")]
fn optimize_webview_performance(window: &tauri::WebviewWindow) {
    use objc2::{
        msg_send,
        runtime::{AnyClass, AnyObject, Bool},
    };

    let _ = window.with_webview(|webview| unsafe {
        let wk: &objc2_web_kit::WKWebView = &*webview.inner().cast();

        let config: *mut AnyObject = msg_send![wk, configuration];
        if config.is_null() {
            tracing::warn!("WebView optimization: WKWebViewConfiguration is null");
            return;
        }
        let prefs: *mut AnyObject = msg_send![config, preferences];
        if prefs.is_null() {
            tracing::warn!("WebView optimization: WKPreferences is null");
            return;
        }

        let ns_num_cls = match AnyClass::get(c"NSNumber") {
            Some(cls) => cls,
            None => return,
        };
        let yes: *mut AnyObject = msg_send![ns_num_cls, numberWithBool: Bool::new(true)];

        for key_str in ["acceleratedCompositingEnabled", "acceleratedDrawingEnabled"] {
            let key = objc2_foundation::NSString::from_str(key_str);
            let _: () = msg_send![prefs, setValue: yes, forKey: &*key];
        }

        tracing::info!(
            "WebView: GPU acceleration enabled (acceleratedCompositingEnabled + acceleratedDrawingEnabled)"
        );
    });
}

#[cfg(target_os = "macos")]
fn show_window(app: &tauri::AppHandle) {
    if let Some(window) = app.get_webview_window("main") {
        let _ = window.show();
        let _ = window.set_focus();
    }
}

fn create_window<R: tauri::Runtime, M: tauri::Manager<R>>(
    manager: &M,
    url: tauri::WebviewUrl,
) -> Result<tauri::WebviewWindow<R>, tauri::Error> {
    let handle = manager.app_handle().clone();
    let builder = tauri::WebviewWindowBuilder::new(manager, "main", url)
        .title("KanbAIn")
        .inner_size(1280.0, 800.0)
        .min_inner_size(800.0, 600.0)
        .resizable(true)
        .zoom_hotkeys_enabled(false)
        .disable_drag_drop_handler();

    // macOS: overlay title bar keeps traffic lights but removes title bar chrome,
    // letting web content extend to the top of the window.
    // Traffic lights are vertically centered within the navbar height (~28px).
    #[cfg(target_os = "macos")]
    let builder = builder
        .title_bar_style(tauri::TitleBarStyle::Overlay)
        .hidden_title(true)
        .traffic_light_position(tauri::LogicalPosition::new(8.0, 14.0));

    builder
        .on_new_window(move |url, _features| {
            tracing::info!("New window requested for URL: {}", url);
            let url_str = url.to_string();
            let _ = handle.opener().open_url(&url_str, None::<&str>);
            tauri::webview::NewWindowResponse::Deny
        })
        .build()
}
