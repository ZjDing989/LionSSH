#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod codex;
mod crypto;
mod locale;
mod model;
mod monitor;
mod native_payload;
mod service;
mod ssh;
mod store;
mod terminal;
mod updater;
mod verification;

use serde_json::Value;
use service::Service;
use std::{
    sync::{atomic::Ordering, Arc},
    time::Duration,
};
use tauri::{
    menu::{Menu, MenuItem, PredefinedMenuItem},
    tray::{MouseButton, TrayIconBuilder, TrayIconEvent},
    Manager, WebviewWindowBuilder,
};
use tauri_plugin_opener::OpenerExt;

fn show_window(app: &tauri::AppHandle) {
    if let Some(window) = app.get_webview_window("main") {
        let _ = window.set_skip_taskbar(false);
        let _ = window.unminimize();
        let _ = window.show();
        let _ = window.set_focus();
    }
}

#[tauri::command]
async fn lion_request(
    window: tauri::WebviewWindow,
    app: tauri::AppHandle,
    state: tauri::State<'_, Arc<Service>>,
    method: String,
    args: Vec<Value>,
) -> Result<Value, String> {
    if window.label() != "main" {
        return Err("不允许的调用来源".into());
    }
    if state.quitting.load(Ordering::SeqCst) {
        return Err("应用正在关闭".into());
    }
    if args.len() > 4
        || serde_json::to_vec(&args).map_err(|_| "请求格式无效")?.len() > 2 * 1024 * 1024
    {
        return Err("请求内容过长".into());
    }
    if [
        "get-update-state",
        "check-update",
        "install-update",
        "open-update-download",
    ]
    .contains(&method.as_str())
    {
        if !args.is_empty() {
            return Err("更新操作不接受参数".into());
        }
        let updates = app.state::<Arc<updater::UpdateManager>>().inner().clone();
        return match method.as_str() {
            "open-update-download" => app
                .opener()
                .open_url(
                    "https://github.com/ZjDing989/LionSSH/releases/latest",
                    None::<&str>,
                )
                .map(|_| Value::Null)
                .map_err(|error| error.to_string()),
            "get-update-state" => Ok(updates.state()),
            "check-update" => updates.check().await,
            _ => {
                let prepared = updates.prepare().await?;
                if state.quitting.load(Ordering::SeqCst) {
                    return Err("应用正在关闭".into());
                }
                state.shutdown().await;
                state.finished.store(true, Ordering::SeqCst);
                let installed = tauri::async_runtime::spawn_blocking(move || prepared.install())
                    .await
                    .map_err(|error| error.to_string())
                    .and_then(|result| result);
                match installed {
                    Ok(updater::InstallDisposition::Exit) => {
                        updates.stop();
                        app.exit(0);
                        Ok(Value::Null)
                    }
                    Ok(updater::InstallDisposition::Restart) => {
                        updates.stop();
                        app.restart();
                    }
                    Err(error) => {
                        state.resume_after_failed_update();
                        Err(error)
                    }
                }
            }
        };
    }
    if method == "pick-key" {
        if !args.is_empty() {
            return Err("选择密钥的请求格式无效".into());
        }
        let selected = rfd::AsyncFileDialog::new()
            .set_title(locale::text(
                state.resolved_language(),
                "选择 SSH 私钥",
                "Select an SSH private key",
            ))
            .pick_file()
            .await;
        return Ok(selected
            .map(|file| Value::String(file.path().to_string_lossy().into_owned()))
            .unwrap_or(Value::Null));
    }
    if !service::PUBLIC_METHODS.contains(&method.as_str()) {
        return Err("不支持的应用操作".into());
    }
    if method == "set-theme" && args.first().and_then(Value::as_str) == Some("system") {
        state
            .request(
                "native-theme",
                vec![Value::String(service::system_theme("light"))],
            )
            .await?;
    }
    let mut result = state.request(&method, args).await?;
    if method == "initial" {
        result["update"] = app.state::<Arc<updater::UpdateManager>>().state();
        service::apply_theme(&app, &result["theme"]);
        apply_language(&app, &result["language"]);
    } else if method == "set-theme" {
        service::apply_theme(&app, &result);
    }
    Ok(result)
}

fn tray_menu(app: &tauri::AppHandle, language: &str) -> tauri::Result<Menu<tauri::Wry>> {
    let open = MenuItem::with_id(
        app,
        "open",
        locale::text(language, "打开软件", "Open Lion SSH"),
        true,
        None::<&str>,
    )?;
    let separator = PredefinedMenuItem::separator(app)?;
    let quit = MenuItem::with_id(
        app,
        "quit",
        locale::text(language, "关闭软件", "Quit Lion SSH"),
        true,
        None::<&str>,
    )?;
    Menu::with_items(app, &[&open, &separator, &quit])
}

pub fn apply_language(app: &tauri::AppHandle, language: &Value) {
    if let Some(tray) = app.tray_by_id("lion-tray") {
        if let Ok(menu) = tray_menu(app, language["resolved"].as_str().unwrap_or("en")) {
            let _ = tray.set_menu(Some(menu));
        }
    }
}

fn install_tray(app: &tauri::AppHandle) -> tauri::Result<()> {
    let language = app
        .try_state::<Arc<Service>>()
        .map(|service| service.resolved_language())
        .unwrap_or_else(locale::system_language);
    let menu = tray_menu(app, language)?;
    let mut builder = TrayIconBuilder::with_id("lion-tray")
        .tooltip("Lion SSH")
        .menu(&menu)
        .show_menu_on_left_click(false)
        .on_menu_event(|app, event| match event.id.as_ref() {
            "open" => show_window(app),
            "quit" => {
                if let Some(state) = app.try_state::<Arc<Service>>() {
                    state.quit(app);
                }
            }
            _ => {}
        })
        .on_tray_icon_event(|tray, event| {
            if let TrayIconEvent::DoubleClick {
                button: MouseButton::Left,
                ..
            } = event
            {
                show_window(tray.app_handle());
            }
        });
    if let Some(icon) = app.default_window_icon() {
        builder = builder.icon(icon.clone());
    }
    builder.build(app)?;
    Ok(())
}

fn main() {
    if let Some(result) = updater::run_portable_helper_from_args() {
        if let Err(error) = &result {
            eprintln!("{error}");
        }
        std::process::exit(i32::from(result.is_err()));
    }
    if std::env::args().any(|arg| arg == "--backend-test") {
        let runtime = tokio::runtime::Runtime::new().expect("无法初始化异步运行时");
        let result = runtime.block_on(verification::run());
        runtime.shutdown_timeout(Duration::from_secs(1));
        if let Err(error) = result {
            eprintln!("{error}");
            std::process::exit(1);
        }
        return;
    }
    let context = match verification::desktop_context(tauri::generate_context!()) {
        Ok(context) => context,
        Err(error) => {
            eprintln!("{error}");
            std::process::exit(1);
        }
    };
    let application = tauri::Builder::default()
        .plugin(tauri_plugin_updater::Builder::new().build())
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_single_instance::init(|app, _, _| {
            show_window(app)
        }))
        .invoke_handler(tauri::generate_handler![lion_request])
        .setup(|app| {
            let handle = app.handle().clone();
            let data_dir = service::data_directory().map_err(std::io::Error::other)?;
            let config = app.config().app.windows.first().ok_or("主窗口配置缺失")?;
            let window = WebviewWindowBuilder::from_config(app, config)?
                .data_directory(data_dir.join("webview"))
                .disable_drag_drop_handler()
                .initialization_script(
                    "document.addEventListener('contextmenu', (event) => event.preventDefault(), { capture: true });",
                )
                .on_new_window(|_, _| tauri::webview::NewWindowResponse::Deny)
                .on_navigation(|url| {
                    url.scheme() == "tauri"
                        || url.host_str() == Some("tauri.localhost")
                        || (cfg!(debug_assertions)
                            && url.host_str() == Some("127.0.0.1")
                            && url.port() == Some(1420))
                })
                .build()?;
            #[cfg(windows)]
            window.with_webview(|webview| unsafe {
                // Also suppress WebView2's native menus, including before the app mounts.
                let result = webview
                    .controller()
                    .CoreWebView2()
                    .and_then(|core| core.Settings())
                    .and_then(|settings| settings.SetAreDefaultContextMenusEnabled(false));
                if let Err(error) = result {
                    eprintln!("无法关闭 WebView2 默认右键菜单: {error}");
                }
            })?;
            let resolved = match window.theme() {
                Ok(tauri::Theme::Dark) => "dark",
                _ => "light",
            };
            let service = Service::spawn(&handle, &data_dir, &service::system_theme(resolved))
                .map_err(std::io::Error::other)?;
            app.manage(Arc::clone(&service));
            let updates = updater::UpdateManager::new(handle.clone());
            app.manage(Arc::clone(&updates));
            // Verification launches stay offline except for their loopback SSH fixtures.
            if !std::env::args().any(|arg| arg == "--smoke" || arg == "--verification-instance") {
                updates.start();
            }
            service
                .tray_available
                .store(install_tray(&handle).is_ok(), Ordering::SeqCst);
            // Smoke launches use an explicit temporary data directory and exit themselves.
            // This exercises the packaged service without disrupting an existing instance.
            if std::env::args().any(|arg| arg == "--smoke") {
                let _ = window.hide();
                let smoke_app = handle.clone();
                let smoke_service = Arc::clone(&service);
                tauri::async_runtime::spawn(async move {
                    let result = smoke_service.request("initial", vec![]).await;
                    let exit_code = i32::from(result.is_err());
                    if let Some(path) = std::env::var_os("LION_SMOKE_RESULT") {
                        let report = match result {
                            Ok(initial) => serde_json::json!({"ok": true, "initial": initial}),
                            Err(error) => serde_json::json!({"ok": false, "error": error}),
                        };
                        let _ = std::fs::write(path, serde_json::to_vec_pretty(&report).unwrap());
                    }
                    tokio::time::sleep(Duration::from_millis(500)).await;
                    smoke_service.quit_with_code(&smoke_app, exit_code);
                });
            }
            Ok(())
        })
        .on_window_event(|window, event| {
            let app = window.app_handle();
            let Some(service) = app.try_state::<Arc<Service>>() else {
                return;
            };
            match event {
                tauri::WindowEvent::CloseRequested { api, .. } => {
                    if service.finished.load(Ordering::SeqCst) {
                        return;
                    }
                    api.prevent_close();
                    if service.quitting.load(Ordering::SeqCst) {
                        return;
                    }
                    if service.tray_available.load(Ordering::SeqCst) {
                        let _ = window.hide();
                        let _ = window.set_skip_taskbar(true);
                    } else {
                        service.quit(app);
                    }
                }
                tauri::WindowEvent::ThemeChanged(theme) => {
                    let resolved = if *theme == tauri::Theme::Dark {
                        "dark"
                    } else {
                        "light"
                    };
                    let service = Arc::clone(&service);
                    tauri::async_runtime::spawn(async move {
                        let _ = service
                            .request(
                                "native-theme",
                                vec![Value::String(service::system_theme(resolved))],
                            )
                            .await;
                    });
                }
                _ => {}
            }
        })
        .build(context);
    let application = match application {
        Ok(application) => application,
        Err(error) => {
            rfd::MessageDialog::new()
                .set_title(locale::text(
                    locale::system_language(),
                    "Lion SSH 启动失败",
                    "Lion SSH could not start",
                ))
                .set_description(error.to_string())
                .set_level(rfd::MessageLevel::Error)
                .show();
            return;
        }
    };
    application.run(|app, event| {
        if let Some(service) = app.try_state::<Arc<Service>>() {
            match event {
                tauri::RunEvent::ExitRequested { api, .. }
                    if !service.finished.load(Ordering::SeqCst) =>
                {
                    api.prevent_exit();
                    service.quit(app);
                }
                tauri::RunEvent::Exit => {
                    if let Some(updates) = app.try_state::<Arc<updater::UpdateManager>>() {
                        updates.stop();
                    }
                    service.kill();
                }
                _ => {}
            }
        }
    });
}
