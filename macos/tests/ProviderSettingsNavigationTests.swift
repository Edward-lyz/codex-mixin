import Cocoa

func menuItemImage(_ systemSymbolName: String) -> NSImage? {
    nil
}

@main
struct ProviderSettingsNavigationTests {
    static func main() throws {
        _ = NSApplication.shared
        let setupScript = ducxTerminalSetupScript(
            terminalTitle: "Codex Mixin DUCX test-session",
            releaseVersion: "1.2.3",
            archiveURL: URL(string: "http://example.invalid/ducx.tar.bz2")!,
            archive: URL(fileURLWithPath: "/tmp/ducx.tar.bz2"),
            downloadStatus: URL(fileURLWithPath: "/tmp/download.status"),
            installStatus: URL(fileURLWithPath: "/tmp/install.status"),
            loginStatus: URL(fileURLWithPath: "/tmp/login.status"),
            executable: URL(fileURLWithPath: "/managed/ducx/current/bin/ducx"),
            loginRequired: true
        )
        precondition(
            setupScript.contains("curl --fail --location --progress-bar --show-error"),
            "The setup terminal must display curl download progress"
        )
        precondition(
            setupScript.contains("'/managed/ducx/current/bin/ducx' login"),
            "The same setup terminal must continue into DUCX login"
        )
        precondition(
            setupScript.contains("close candidateWindow"),
            "The dedicated setup terminal must close itself after success"
        )
        precondition(
            setupScript.contains("trap 'exit 130' HUP INT TERM"),
            "Closing the setup terminal early must fail the App workflow promptly"
        )
        let duccSetupScript = duccTerminalSetupScript(
            terminalTitle: "Codex Mixin DUCC test-session",
            releaseVersion: "2.1.218.3",
            zstdArchiveURL: URL(string: "http://example.invalid/ducc.tar.zst")!,
            bzip2ArchiveURL: URL(string: "http://example.invalid/ducc.tar.bz2")!,
            archive: URL(fileURLWithPath: "/tmp/ducc.archive"),
            archiveFormatStatus: URL(fileURLWithPath: "/tmp/ducc-format.status"),
            downloadStatus: URL(fileURLWithPath: "/tmp/ducc-download.status"),
            installStatus: URL(fileURLWithPath: "/tmp/ducc-install.status"),
            installErrorStatus: URL(fileURLWithPath: "/tmp/ducc-install.error"),
            loginStatus: URL(fileURLWithPath: "/tmp/ducc-login.status"),
            executable: URL(
                fileURLWithPath: "/managed/ducc/home/.baidu-cc/baidu-cc/bin/ducc"
            ),
            isolatedHome: URL(fileURLWithPath: "/managed/ducc/home"),
            loginRequired: true
        )
        precondition(
            duccSetupScript.contains("HOME='/managed/ducc/home'"),
            "DUCC login must run with the managed isolated HOME"
        )
        let officialArchiveURLs = duccArchiveURLs(
            version: "2.1.218.3",
            architecture: "arm64"
        )
        precondition(
            officialArchiveURLs.zstd.hasSuffix(
                "/baidu-cc-darwin-arm64-2.1.218.3.tar.zst"
            )
                && officialArchiveURLs.bzip2.hasSuffix(
                    "/baidu-cc-darwin-arm64-2.1.218.3.tar.bz2"
                ),
            "DUCC archive URLs must follow the official macOS package rules"
        )
        guard let zstdPosition = duccSetupScript.range(of: "ducc.tar.zst")?.lowerBound,
              let bzip2Position = duccSetupScript.range(of: "ducc.tar.bz2")?.lowerBound
        else {
            preconditionFailure("The DUCC setup must include both archive formats")
        }
        precondition(
            zstdPosition < bzip2Position,
            "DUCC setup must try zstd before falling back to bzip2"
        )
        precondition(
            duccSetupScript.contains("/usr/bin/tar -tf '/tmp/ducc.archive'")
                && duccSetupScript.contains("/usr/bin/tar -tjf '/tmp/ducc.archive'"),
            "DUCC setup must validate each archive with format-specific tar arguments"
        )
        precondition(
            duccSetupScript.contains(
                "'/managed/ducc/home/.baidu-cc/baidu-cc/bin/ducc' login"
            ),
            "The managed DUCC executable must perform QR-code login"
        )
        precondition(
            !duccSetupScript.contains("install.sh")
                && !duccSetupScript.contains("update_claude_json")
                && !duccSetupScript.contains(".zshrc"),
            "Managed DUCC setup must not run the official mutating installer"
        )
        precondition(
            duccSetupScript.contains("close candidateWindow"),
            "The dedicated DUCC setup terminal must close itself after success"
        )
        precondition(
            duccSetupScript.contains("具体错误：")
                && duccSetupScript.contains("'/tmp/ducc-install.error'"),
            "DUCC setup failures must expose the original installer error in Terminal"
        )
        let managedRoot = FileManager.default.temporaryDirectory
            .appendingPathComponent("codex-mixin-ducx-layout-\(UUID().uuidString)")
        try FileManager.default.createDirectory(
            at: managedRoot,
            withIntermediateDirectories: true
        )
        defer { try? FileManager.default.removeItem(at: managedRoot) }
        try replaceManagedDucxLink(
            named: "baidu-cx",
            destination: "10.145.0.3",
            root: managedRoot,
            fileManager: .default
        )
        try replaceManagedDucxLink(
            named: "current",
            destination: "10.145.0.3",
            root: managedRoot,
            fileManager: .default
        )
        let officialDestination = try FileManager.default.destinationOfSymbolicLink(
            atPath: managedRoot.appendingPathComponent("baidu-cx").path
        )
        let currentDestination = try FileManager.default.destinationOfSymbolicLink(
            atPath: managedRoot.appendingPathComponent("current").path
        )
        precondition(
            officialDestination == "10.145.0.3",
            "The managed install must expose DUCX's official baidu-cx runtime entry"
        )
        precondition(
            currentDestination == "10.145.0.3",
            "The managed install must retain Codex Mixin's stable current entry"
        )
        let syntaxCheck = Process()
        syntaxCheck.executableURL = URL(fileURLWithPath: "/bin/zsh")
        syntaxCheck.arguments = ["-n", "-c", setupScript]
        try syntaxCheck.run()
        syntaxCheck.waitUntilExit()
        precondition(
            syntaxCheck.terminationStatus == 0,
            "The generated DUCX setup terminal script must be valid zsh"
        )
        let duccSyntaxCheck = Process()
        duccSyntaxCheck.executableURL = URL(fileURLWithPath: "/bin/zsh")
        duccSyntaxCheck.arguments = ["-n", "-c", duccSetupScript]
        try duccSyntaxCheck.run()
        duccSyntaxCheck.waitUntilExit()
        precondition(
            duccSyntaxCheck.terminationStatus == 0,
            "The generated DUCC setup terminal script must be valid zsh"
        )

        let response = try JSONDecoder().decode(
            ProviderListResponse.self,
            from: Data(
                """
                {
                  "config_version": 2,
                  "gateway_bind": "127.0.0.1:8787",
                  "gateway_auth_configured": true,
                  "codex_install_mode": "custom_only",
                  "providers": [{
                    "id": "baidu-oneapi",
                    "display_name": "Baidu OneAPI",
                    "enabled": true,
                    "auxiliary_model_upstream": false,
                    "preset_id": "baidu-oneapi",
                    "protocol": "openai_responses",
                    "base_url": "http://example.invalid",
                    "api_path": "/v1/responses",
                    "model_source": {"kind": "static"},
                    "api_key_configured": true,
                    "quota_username": "tester",
                    "quota_parser": "baidu_oneapi",
                    "baidu_auth_bridge": null,
                    "ducx_app_server": null,
                    "selected_models": ["GLM-5.2"],
                    "new_models": [],
                    "unavailable_selected_models": [],
                    "cached_models": [{
                      "id": "GLM-5.2",
                      "display_name": "GLM-5.2"
                    }],
                    "models_refreshed_at_ms": null,
                    "readiness": "ready",
                    "readiness_issues": [],
                    "routable_model_count": 1
                  }]
                }
                """.utf8
            )
        )
        var receivedArguments: [[String]] = []
        var applyCount = 0
        var completionTitle: String?
        var requestedBridgeModes: [BaiduAuthBridgeMode] = []
        let controller = ProviderSettingsWindowController(
            loadHandler: { response },
            runHandler: { arguments in
                receivedArguments.append(arguments)
                return ""
            },
            applyHandler: {
                applyCount += 1
            },
            baiduBridgeSetupHandler: { mode in
                requestedBridgeModes.append(mode)
                return URL(
                    fileURLWithPath: mode == .duccLoopback
                        ? "/managed/ducc/home/.baidu-cc/baidu-cc/bin/ducc"
                        : "/managed/ducx/current/bin/ducx"
                )
            },
            completionHandler: { title, _ in
                completionTitle = title
            }
        )

        controller.present()
        waitUntil {
            controller.window?.attachedSheet != nil
        }
        guard let window = controller.window, let sheet = window.attachedSheet else {
            preconditionFailure("The auth bridge reminder must be presented")
        }
        guard let configureDucc = descendantViews(of: NSButton.self, in: sheet.contentView)
            .first(where: { $0.title == "配置 DUCC" }) else {
            preconditionFailure("The reminder must offer DUCC configuration")
        }

        configureDucc.performClick(nil)
        waitUntil {
            !window.isVisible
        }
        precondition(
            requestedBridgeModes == [.duccLoopback],
            "Opening DUCC configuration must set up only the DUCC core"
        )
        precondition(
            receivedArguments.contains(where: {
                $0.starts(with: ["providers", "update", "baidu-oneapi"])
                    && $0.containsSubsequence([
                        "--baidu-auth-bridge",
                        "ducc_loopback",
                    ])
                    && $0.containsSubsequence(["--ducx-app-server", "false"])
                    && $0.containsSubsequence([
                        "--ducc-executable",
                        "/managed/ducc/home/.baidu-cc/baidu-cc/bin/ducc",
                    ])
            }),
            "Opening DUCC configuration must save the DUCC bridge and legacy-safe fallback"
        )
        precondition(
            applyCount == 1,
            "Opening DUCC configuration must apply and restart the gateway"
        )
        precondition(
            !window.isVisible,
            "The provider settings window must close after DUCC setup succeeds"
        )
        precondition(
            completionTitle == "DUCC 已配置",
            "Successful DUCC setup must visibly confirm completion"
        )

        controller.close()
        print("Provider auth bridge reminder navigation: passed")
    }

    private static func waitUntil(
        timeout: TimeInterval = 2,
        condition: () -> Bool
    ) {
        let deadline = Date(timeIntervalSinceNow: timeout)
        while !condition(), RunLoop.main.run(mode: .default, before: deadline) {
            if Date() >= deadline {
                break
            }
        }
    }

    private static func descendantViews<T: NSView>(
        of type: T.Type,
        in root: NSView?
    ) -> [T] {
        guard let root else { return [] }
        let current = (root as? T).map { [$0] } ?? []
        return current + root.subviews.flatMap { descendantViews(of: type, in: $0) }
    }
}

private extension Array where Element == String {
    func containsSubsequence(_ expected: [String]) -> Bool {
        guard expected.count <= count else { return false }
        return indices.contains { start in
            let end = index(start, offsetBy: expected.count, limitedBy: endIndex)
            return end.map { Array(self[start..<$0]) == expected } ?? false
        }
    }
}
