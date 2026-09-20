//! Operating-system adapters used by the cross-platform core.
//!
//! Callers express intent through this module instead of branching on an OS.
//! Platform-specific process flags, paths, and permission tools stay here.

mod paths;
mod permissions;
mod process;

pub use paths::{home_dir, home_dir_required};
pub use permissions::{restrict_owner_only_dir, restrict_owner_only_file};
pub use process::{
    force_kill_process_tree, force_kill_process_tree_async, pid_is_running,
    prepare_background_command, prepare_background_tokio_command, prepare_daemon_command,
    send_process_signal,
};
