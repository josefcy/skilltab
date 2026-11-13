mod commands;

use skilltab_core::{Env, Skilltab};
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

            let home = app.path().home_dir()?;
            let jstack = home.join("jstack");
            let env = Env { home, projects: Vec::new(), jstack_root: jstack.is_dir().then_some(jstack) };
            let data_dir = app.path().local_data_dir()?.join("skilltab");
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
