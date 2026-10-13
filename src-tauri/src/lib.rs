//! Tauri backend for the Git tool. Commands live in per-domain modules;
//! this file only wires them into the invoke handler.

#[cfg(test)]
mod alloc_count;

/// Test-only counting allocator (see `alloc_count`). It forwards every request
/// to the system allocator, so the only thing it changes is that a test can see
/// how many heap operations its own thread performed.
#[cfg(test)]
#[global_allocator]
static COUNTING: alloc_count::Counting = alloc_count::Counting;

mod branch;
mod diff;
mod log;
mod merge;
mod rebase;
mod remote;
mod repo;
mod stash;
mod tag;
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
            working::write_file,
            stash::list_stashes,
            stash::stash_save,
            stash::stash_apply,
            stash::stash_pop,
            stash::stash_drop,
            tag::list_tags,
            tag::create_tag,
            tag::delete_tag,
            rebase::rebase,
            rebase::rebase_in_progress,
            rebase::rebase_continue,
            rebase::rebase_abort
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
