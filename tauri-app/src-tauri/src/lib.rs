//! RockCraft Tauri frontend — integration layer over `rockcraft-core`.
//!
//! This is the M2 scaffold: it opens an empty window and registers no
//! commands or events yet. Subsequent issues build the note-highway and
//! record screens on top of this structure, wiring the webview to `core`
//! via Tauri commands and events.

/// Build and run the Tauri application.
#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
