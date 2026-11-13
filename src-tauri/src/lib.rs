mod commands;

use skilltab_core::{Env, Skilltab};
use std::path::PathBuf;
use tauri::{Manager, WindowEvent};

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let builder = commands::builder();

    #[cfg(debug_assertions)]
    builder
        .export(specta_typescript::Typescript::default(), concat!(env!("CARGO_MANIFEST_DIR"), "/../src/bindings.ts"))
        .expect("export bindings");

    tauri::Builder::default()
        .invoke_handler(builder.invoke_handler())
        .manage(commands::RefreshGuard::default())
        .setup(move |app| {
            builder.mount_events(app);

            // SKILLTAB_HOME points the whole app, its own data included, at another home, so it can be
            // exercised against a fixture without touching the real skill folders.
            let (home, data_dir) = match std::env::var_os("SKILLTAB_HOME").map(PathBuf::from) {
                Some(home) => (home.clone(), home.join(".skilltab-data")),
                None => (app.path().home_dir()?, app.path().local_data_dir()?.join("skilltab")),
            };
            // SKILLTAB_DATA moves only the data dir, so the real machine can be looked at with a copy of its usage index
            // and the scan's usage ingest never writes the real one.
            let data_dir = std::env::var_os("SKILLTAB_DATA").map(PathBuf::from).unwrap_or(data_dir);
            let jstack = home.join("jstack");
            let env = Env { home, projects: Vec::new(), jstack_root: jstack.is_dir().then_some(jstack) };
            app.manage(Skilltab::open(env, data_dir).map_err(|e| format!("{e:?}"))?);
            Ok(())
        })
        .on_window_event(|window, event| {
            if let WindowEvent::Focused(true) = event {
                let handle = window.app_handle().clone();
                tauri::async_runtime::spawn_blocking(move || commands::refresh_and_notify(&handle));
            }
        })
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
