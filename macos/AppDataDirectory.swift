import Foundation

/// Directory for data the menu app owns itself: its diagnostic log, update
/// watchdog log, quota display cache, and provider icon cache. The CLI owns
/// its own state directory and reports it through `interface --json`; the app
/// never writes there.
func appDataDirectory(fileManager: FileManager = .default) -> URL {
    let base = fileManager.urls(for: .applicationSupportDirectory, in: .userDomainMask).first
        ?? fileManager.homeDirectoryForCurrentUser
            .appendingPathComponent("Library/Application Support", isDirectory: true)
    return base.appendingPathComponent("Codex Mixin", isDirectory: true)
}
