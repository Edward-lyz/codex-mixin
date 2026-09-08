import Cocoa

extension AppDelegate {
    func installLaunchAgent() throws {
        try FileManager.default.createDirectory(at: stateDir(), withIntermediateDirectories: true)
        try FileManager.default.createDirectory(at: launchAgentPath().deletingLastPathComponent(), withIntermediateDirectories: true)

        let executable = try gatewayExecutableURL()
        let logFile = stateDir().appendingPathComponent("gateway.log").path
        let plist = """
        <?xml version="1.0" encoding="UTF-8"?>
        <!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
        <plist version="1.0">
        <dict>
          <key>Label</key>
          <string>\(serviceLabel)</string>
          <key>ProgramArguments</key>
          <array>
            <string>\(xmlEscape(executable.path))</string>
            <string>start</string>
            <string>--log-file</string>
            <string>\(xmlEscape(logFile))</string>
          </array>
          <key>RunAtLoad</key>
          <true/>
          <key>KeepAlive</key>
          <dict>
            <key>SuccessfulExit</key>
            <false/>
          </dict>
          <key>ThrottleInterval</key>
          <integer>10</integer>
          <key>ProcessType</key>
          <string>Background</string>
          <key>StandardOutPath</key>
          <string>/dev/null</string>
          <key>StandardErrorPath</key>
          <string>/dev/null</string>
          <key>WorkingDirectory</key>
          <string>\(xmlEscape(FileManager.default.homeDirectoryForCurrentUser.path))</string>
        </dict>
        </plist>
        """
        try plist.write(to: launchAgentPath(), atomically: true, encoding: .utf8)
        try installMenuLaunchAgent()
    }

    func installMenuLaunchAgent() throws {
        try FileManager.default.createDirectory(at: menuLaunchAgentPath().deletingLastPathComponent(), withIntermediateDirectories: true)
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

    func launchAgentNeedsUpdate() throws -> Bool {
        let data = try Data(contentsOf: launchAgentPath())
        guard
            let plist = try PropertyListSerialization.propertyList(from: data, format: nil) as? [String: Any],
            let arguments = plist["ProgramArguments"] as? [String],
            let keepAlive = plist["KeepAlive"] as? [String: Any]
        else {
            return true
        }
        let expectedArguments = [
            try gatewayExecutableURL().path,
            "start",
            "--log-file",
            stateDir().appendingPathComponent("gateway.log").path,
        ]
        return arguments != expectedArguments
            || plist["RunAtLoad"] as? Bool != true
            || keepAlive["SuccessfulExit"] as? Bool != false
            || plist["ThrottleInterval"] as? Int != 10
            || plist["ProcessType"] as? String != "Background"
    }
}
