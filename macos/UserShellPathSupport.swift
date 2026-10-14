import Foundation

/// How long the login shell may take to print its PATH. A slow profile must
/// not hold an operation up; the known folders still apply without it.
private let loginShellPathTimeout: TimeInterval = 3

/// Folders where command-line tools are commonly installed, relative to the
/// home folder, checked even when the login shell does not answer.
private let userToolFolders = [
    ".local/bin", ".npm-global/bin", ".bun/bin", ".volta/bin",
    ".asdf/shims", ".local/share/mise/shims", ".cargo/bin", ".opencode/bin",
]
private let systemToolFolders = ["/opt/homebrew/bin", "/usr/local/bin"]

/// The PATH a Terminal window has. An app opened from the Finder, the Dock or
/// a login item gets launchd's `/usr/bin:/bin:/usr/sbin:/sbin`, so agent CLIs
/// installed by Homebrew, npm or a standalone installer were not detected.
/// Computed once: the login shell runs one time per app launch.
let userShellPATH: String = resolveUserShellPATH(
    current: ProcessInfo.processInfo.environment["PATH"] ?? "",
    loginShellPATH: readLoginShellPATH(),
    home: FileManager.default.homeDirectoryForCurrentUser.path,
    folderExists: { path in
        var isDirectory: ObjCBool = false
        return FileManager.default.fileExists(atPath: path, isDirectory: &isDirectory)
            && isDirectory.boolValue
    }
)

/// Login shell folders first, then the app's own, then known tool folders
/// that exist; each folder once, in first-seen order.
func resolveUserShellPATH(
    current: String,
    loginShellPATH: String?,
    home: String,
    folderExists: (String) -> Bool
) -> String {
    let known = (userToolFolders.map { "\(home)/\($0)" } + systemToolFolders)
        .filter(folderExists)
    var seen = Set<String>()
    var folders: [String] = []
    for folder in (loginShellPATH ?? "").split(separator: ":").map(String.init)
        + current.split(separator: ":").map(String.init)
        + known
    where !folder.isEmpty && seen.insert(folder).inserted {
        folders.append(folder)
    }
    return folders.joined(separator: ":")
}

/// The PATH the user's login shell sets, or nil when the shell fails or does
/// not answer in time. A non-interactive login shell reads the profile files
/// without starting prompts or terminal setup.
private func readLoginShellPATH() -> String? {
    let shell = ProcessInfo.processInfo.environment["SHELL"].flatMap { $0.isEmpty ? nil : $0 }
        ?? "/bin/zsh"
    let marker = "__CODEX_MIXIN_PATH__"
    let process = Process()
    let output = Pipe()
    process.executableURL = URL(fileURLWithPath: shell)
    process.arguments = ["-l", "-c", "printf '\(marker)%s' \"$PATH\""]
    process.standardInput = FileHandle.nullDevice
    process.standardOutput = output
    process.standardError = FileHandle.nullDevice
    let finished = DispatchSemaphore(value: 0)
    process.terminationHandler = { _ in finished.signal() }
    do {
        try process.run()
    } catch {
        appendAppDiagnosticLog(
            "USER_PATH login shell failed shell=\(shell) error=\(error)",
            directory: appDataDirectory()
        )
        return nil
    }
    if finished.wait(timeout: .now() + loginShellPathTimeout) == .timedOut {
        process.terminate()
        appendAppDiagnosticLog(
            "USER_PATH login shell timed out shell=\(shell)",
            directory: appDataDirectory()
        )
        return nil
    }
    let text = String(decoding: output.fileHandleForReading.readDataToEndOfFile(), as: UTF8.self)
    // Profiles may print banners; keep only what follows the marker.
    guard process.terminationStatus == 0, let range = text.range(of: marker) else {
        appendAppDiagnosticLog(
            "USER_PATH login shell exit=\(process.terminationStatus) shell=\(shell)",
            directory: appDataDirectory()
        )
        return nil
    }
    return String(text[range.upperBound...])
}
