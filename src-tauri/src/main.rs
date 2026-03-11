#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod cache;
mod claude_auth;
mod codex_discovery;
mod codex_usage;
mod commands;
mod jsonl_parser;
mod session_discovery;
mod session_parser;
mod tray;
mod types;

use tauri::Manager;

fn main() {
    tauri::Builder::default()
        .plugin(tauri_plugin_shell::init())
        .invoke_handler(tauri::generate_handler![
            commands::get_active_sessions,
            commands::get_active_codex_sessions,
            commands::get_claude_user_info,
            commands::get_codex_usage,
            commands::load_session_silent,
            commands::close_window,
        ])
        .setup(|app| {
            // macOS: 隐藏 Dock 图标，仅保留状态栏图标
            #[cfg(target_os = "macos")]
            app.set_activation_policy(tauri::ActivationPolicy::Accessory);

            // 初始化 Tray 猫动画
            tray::setup_tray(app.handle());

            // 窗口定位到屏幕右下角、程序坞上方
            if let Some(window) = app.get_webview_window("main") {
                if let Ok(Some(monitor)) = window.current_monitor() {
                    let size = monitor.size();
                    let scale = monitor.scale_factor();
                    let x = (size.width as f64 / scale) - 180.0 - 16.0;
                    // 留出 Dock 高度（约 80px）+ 间距
                    let y = (size.height as f64 / scale) - 140.0 - 90.0;
                    let _ = window.set_position(tauri::Position::Logical(
                        tauri::LogicalPosition::new(x, y),
                    ));
                }
            }

            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
