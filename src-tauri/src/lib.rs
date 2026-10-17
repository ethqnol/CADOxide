pub mod commands;

use commands::db::get_review_threads;
use commands::git::get_git_revisions;
use commands::kicad::{
    execute_drc, export_pcb_vrml_3d, get_kicad_version, render_pcb_layers, render_schematic_sheets,
};

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_log::Builder::default().build())
        .invoke_handler(tauri::generate_handler![
            get_kicad_version,
            render_pcb_layers,
            render_schematic_sheets,
            execute_drc,
            export_pcb_vrml_3d,
            get_git_revisions,
            get_review_threads,
        ])
        .run(tauri::generate_context!())
        .expect("error while building tauri application");
}
