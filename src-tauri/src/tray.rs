use tauri::{
    menu::{Menu, MenuItem},
    tray::TrayIconBuilder,
    AppHandle, Manager,
};

/// Amin lives in the menu bar, not the Dock — a standing presence, not an
/// app you "open" and "quit" each time. Pairs with the window's
/// CloseRequested handler in lib.rs, which hides the window instead of
/// exiting; only "Quit Amin" here actually terminates the process.
pub fn setup(app: &AppHandle) -> tauri::Result<()> {
    let show = MenuItem::with_id(app, "show", "Show Amin", true, None::<&str>)?;
    let quit = MenuItem::with_id(app, "quit", "Quit Amin", true, None::<&str>)?;
    let menu = Menu::with_items(app, &[&show, &quit])?;

    TrayIconBuilder::new()
        .icon(
            app.default_window_icon()
                .expect("bundled tray icon (see tauri.conf.json bundle.icon)")
                .clone(),
        )
        .menu(&menu)
        .show_menu_on_left_click(true)
        .on_menu_event(|app, event| match event.id.as_ref() {
            "show" => {
                if let Some(window) = app.get_webview_window("main") {
                    let _ = window.show();
                    let _ = window.set_focus();
                }
            }
            "quit" => app.exit(0),
            _ => {}
        })
        .build(app)?;

    Ok(())
}
