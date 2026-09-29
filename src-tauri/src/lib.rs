//! Tauri backend for the Git tool. Commands live in per-domain modules;
//! this file only wires them into the invoke handler.

mod branch;
mod diff;
mod log;
mod merge;
mod remote;
mod repo;
mod working;

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .invoke_handler(tauri::generate_handler![
            working::get_conflicts,
            working::commit,
            remote::push,
            merge::merge,
            merge::abort_merge,
            working::get_status,
            working::stage,
            working::unstage,
            working::resolve_conflict,
            working::get_file_contents,
            diff::get_diff,
            log::get_log,
            log::get_commit_files,
            diff::get_commit_file_diff,
            branch::list_branches,
            branch::checkout,
            branch::create_branch,
            branch::delete_branch,
            remote::fetch,
            remote::pull,
            working::discard,
            working::write_file
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
