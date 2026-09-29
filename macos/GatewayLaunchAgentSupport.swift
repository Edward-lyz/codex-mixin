import Cocoa

extension AppDelegate {
    func installMenuLaunchAgent() throws {
        try FileManager.default.createDirectory(
            at: menuLaunchAgentPath().deletingLastPathComponent(),
            withIntermediateDirectories: true
        )
        guard let executableURL = Bundle.main.executableURL else {
            throw GatewayError.command("Codex Mixin app executable not found")
        }
        let plist = menuLaunchAgentPlist(
            label: menuLaunchLabel,
            executablePath: executableURL.path,
            logPath: stateDir().appendingPathComponent("macos-app.log").path
        )
        try plist.write(to: menuLaunchAgentPath(), atomically: true, encoding: .utf8)
    }
}
