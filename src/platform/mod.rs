//! Operating-system adapters used by the cross-platform core.
//!
//! Callers express intent through this module instead of branching on an OS.
//! Platform-specific process flags, paths, and permission tools stay here.

mod codex_cli;
mod desktop;
mod files;
mod package;
mod paths;
mod permissions;
mod process;
mod shell;
mod signing;
mod startup_service;
mod terminal;

pub use codex_cli::{
    CodexCliInstaller, codex_cli_candidates, codex_cli_installer, launcher_command,
    path_executable_candidates,
};
pub use desktop::{
    DesktopNotification, NotificationDelivery, RunningDesktopApp, codex_desktop_apps,
    desktop_app_restart_hint, desktop_environment_hint, restart_desktop_app, running_desktop_app,
    show_notification,
};
pub use files::{
    EXECUTABLE_SUFFIX, OwnerOnlyStatus, executable_file_name, is_owner_only, lock_exclusive,
    make_executable, make_private_executable, make_private_executable_on, owner_only_status,
    persist_temp_path, set_owner_only_dir_mode, set_owner_only_mode, set_owner_only_mode_on,
    symlink_file,
};
pub use package::{PackageTarget, package_target};
pub use paths::{
    claude_desktop_config_root, home_dir, home_dir_required, os_name, set_home_env,
    set_tokio_home_env,
};
pub use permissions::{restrict_owner_only_dir, restrict_owner_only_file};
pub use process::{
    ShutdownSignal, current_executable_image_name, force_kill_process_tree,
    force_kill_process_tree_async, isolate_process_group, isolate_tokio_process_group,
    kill_process_tree, kill_process_tree_async, pid_is_running, prepare_background_command,
    prepare_background_tokio_command, prepare_daemon_command, send_process_signal,
    terminate_isolated_child, terminate_isolated_tokio_child,
};
pub use shell::{
    PYTHON_LAUNCHER, is_report_hook_command, portable_path_text, report_hook_command, shell_quote,
};
pub use signing::prepare_modified_executable;
pub use startup_service::{
    StartupServiceSpec, StartupServiceStatus, install_startup_service, remove_startup_service,
    start_startup_service, startup_service_status, startup_service_supported, stop_startup_service,
};
pub use terminal::{TerminalCommand, open_terminal_window};

pub mod update;

pub mod installation;
