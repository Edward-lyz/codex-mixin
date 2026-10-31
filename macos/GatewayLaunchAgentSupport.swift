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
        let logPath = stateDir().appendingPathComponent("macos-app.log").path
        let plist: [String: Any] = [
            "Label": menuLaunchLabel,
            "ProgramArguments": [executableURL.path, "--background"],
            "RunAtLoad": true,
            "KeepAlive": ["SuccessfulExit": false],
            "ThrottleInterval": 10,
            "ProcessType": "Interactive",
            "StandardOutPath": logPath,
            "StandardErrorPath": logPath,
        ]
        let data = try PropertyListSerialization.data(fromPropertyList: plist, format: .xml, options: 0)
        try data.write(to: menuLaunchAgentPath(), options: .atomic)
    }

    func menuLaunchAtLoginMarkerPath() -> URL {
        stateDir().appendingPathComponent("launch-at-login.applied")
    }

    /// Applies the launch-at-login default once, then leaves the user's
    /// choice alone. A platform without the startup service, an unstable
    /// bundle path, a system-level disable, and a login item the user removed
    /// all skip the default instead of overriding it.
    func applyDefaultLaunchAtLogin() {
        Task { @MainActor [weak self] in
            guard let self else { return }
            do {
                let capabilities = try await self.interfaceCapabilities()
                let state = LaunchAtLoginState(
                    platformSupportsStartupService: capabilities["gateway_autostart"] ?? false,
                    bundlePathIsStable: launchAtLoginBundlePathIsStable(
                        Bundle.main.bundleURL.path
                    ),
                    launchAgentInstalled: FileManager.default.fileExists(
                        atPath: self.menuLaunchAgentPath().path
                    ),
                    systemDisabled: try await self.menuLaunchAgentIsDisabled(),
                    defaultAppliedMarkerExists: FileManager.default.fileExists(
                        atPath: self.menuLaunchAtLoginMarkerPath().path
                    )
                )
                let action = launchAtLoginAction(for: state)
                guard case .installDefault = action else {
                    self.appendDiagnosticLog(
                        "launch at login default skipped: \(launchAtLoginSkipDescription(action))"
                    )
                    return
                }
                _ = try await self.runGateway(["service", "autostart", "enable", "--json"])
                try self.installMenuLaunchAgent()
                let marker = self.menuLaunchAtLoginMarkerPath()
                try FileManager.default.createDirectory(
                    at: marker.deletingLastPathComponent(),
                    withIntermediateDirectories: true
                )
                try Data().write(to: marker)
                self.appendDiagnosticLog("launch at login default applied")
            } catch {
                self.appendDiagnosticLog(
                    "launch at login default failed: \(localizedErrorDescription(error))"
                )
            }
        }
    }

    func menuLaunchAgentIsDisabled() async throws -> Bool {
        let output = try await runProcess(
            "/bin/launchctl",
            ["print-disabled", launchDomain()]
        )
        return launchAgentDisabledOverride(in: output, label: menuLaunchLabel)
    }
}
