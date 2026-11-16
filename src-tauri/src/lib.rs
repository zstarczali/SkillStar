mod ai;
mod claude_cli;
mod commands;
mod db;
mod docx;
mod ebook;
mod extract;
mod ocr;
mod pdf;
mod process;
mod rag;
mod settings;
mod skill;
mod update;
mod usage;
mod vector;
mod web;

use commands::AppState;
use tauri::Manager;

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let builder = tauri::Builder::default()
        // a second launch brings the running SkillStar to the front instead of a second window
        .plugin(tauri_plugin_single_instance::init(|app, _argv, _cwd| {
            if let Some(w) = app.get_webview_window("main") {
                let _ = w.unminimize();
                let _ = w.show();
                let _ = w.set_focus();
            }
        }));
    // macOS: only the application menu and Edit (needed for Cmd+C/V/X/A/Z).
    #[cfg(target_os = "macos")]
    let builder = builder.enable_macos_default_menu(false).menu(|app| {
        use tauri::menu::{Menu, PredefinedMenuItem as P, Submenu};
        let app_menu = Submenu::with_items(app, "SkillStar", true, &[
            &P::hide(app, None)?,
            &P::hide_others(app, None)?,
            &P::show_all(app, None)?,
            &P::separator(app)?,
            &P::quit(app, None)?,
        ])?;
        let edit = Submenu::with_items(app, "Edit", true, &[
            &P::undo(app, None)?,
            &P::redo(app, None)?,
            &P::separator(app)?,
            &P::cut(app, None)?,
            &P::copy(app, None)?,
            &P::paste(app, None)?,
            &P::select_all(app, None)?,
        ])?;
        Menu::with_items(app, &[&app_menu, &edit])
    });
    builder
        .plugin(tauri_plugin_dialog::init())
        .plugin(
            tauri_plugin_window_state::Builder::default()
                .with_state_flags(commands::window_state_flags())
                .with_denylist(&["splashscreen"])
                .build(),
        )
        .setup(|app| {
            let dir = app.path().app_config_dir().unwrap_or_else(|_| std::env::temp_dir().join("SkillStar"));
            usage::init(app.handle().clone(), dir.clone());
            let data = app.path().app_data_dir().unwrap_or_else(|_| dir.clone());
            let docs = app
                .path()
                .document_dir()
                .or_else(|_| app.path().home_dir())
                .unwrap_or_else(|_| std::env::temp_dir());
            let db_path = data.join("skillstar.db");
            let conn = db::open(&db_path).map_err(|e| format!("SkillStar database ({}): {e}", db_path.display()))?;
            // documents left half-processed by a crash start again from "new"
            let _ = conn.execute("UPDATE documents SET status='new' WHERE status IN ('queued','processing')", []);
            let state = AppState {
                settings: settings::SettingsStore::load(dir),
                db_path,
                db: std::sync::Mutex::new(conn),
                data_dir: data,
                default_out: docs.join("SkillStar"),
            };
            app.manage(state);
            let started = std::time::Instant::now();
            let handle = app.handle().clone();
            std::thread::spawn(move || {
                std::thread::sleep(std::time::Duration::from_millis(300));
                if let Err(e) = commands::create_main_window(&handle) {
                    eprintln!("SkillStar: could not create the main window: {e}");
                }
                std::thread::sleep(commands::SPLASH_TIME.saturating_sub(started.elapsed()));
                commands::finish_splash(&handle);
            });
            Ok(())
        })
        .on_window_event(|window, event| {
            if window.label() == "splashscreen" && matches!(event, tauri::WindowEvent::Destroyed) {
                commands::finish_splash(window.app_handle());
            }
            if window.label() == "main" {
                match event {
                    tauri::WindowEvent::CloseRequested { .. } => commands::save_window_state(window.app_handle()),
                    tauri::WindowEvent::Moved(_) | tauri::WindowEvent::Resized(_) => {
                        commands::save_window_state_later(window.app_handle())
                    }
                    _ => {}
                }
            }
        })
        .invoke_handler(tauri::generate_handler![
            usage::ai_usage,
            usage::ai_usage_reset,
            commands::get_settings,
            commands::save_settings,
            commands::set_ui_language,
            commands::default_out_dir,
            commands::list_skills,
            commands::get_skill,
            commands::create_skill,
            commands::update_skill,
            commands::delete_skill,
            commands::list_documents,
            commands::add_documents,
            commands::web_import,
            commands::web_cancel,
            commands::web_info,
            commands::web_resume,
            commands::remove_documents,
            commands::update_document,
            commands::set_document_rag,
            commands::reorder_documents,
            commands::document_text,
            commands::save_page,
            commands::process_documents,
            commands::cancel_processing,
            commands::processing_queue,
            commands::search,
            commands::build_skill,
            commands::preview_skill,
            commands::skill_out_dir,
            commands::engine_status,
            commands::pdfium_download,
            commands::pdfium_test,
            commands::reveal_in_explorer,
            commands::page_image,
            commands::page_words,
            commands::vector_status,
            commands::vector_build,
            commands::vector_stop,
            commands::chat_list,
            commands::chat_turns,
            commands::chat_rename,
            commands::chat_delete,
            commands::open_file,
            commands::open_url,
            commands::exe_path,
            commands::ai_describe,
            commands::ai_skill_description,
            commands::ai_ask,
            commands::ai_test,
            commands::app_info,
            commands::set_zoom,
            commands::clipboard_text,
            update::check_for_update,
            update::download_and_install_update,
            update::update_server_check,
        ])
        .run(tauri::generate_context!())
        .expect("error while running SkillStar");
}
