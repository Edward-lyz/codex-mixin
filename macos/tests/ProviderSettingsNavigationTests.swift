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
        let syntaxCheck = Process()
        syntaxCheck.executableURL = URL(fileURLWithPath: "/bin/zsh")
        syntaxCheck.arguments = ["-n", "-c", setupScript]
        try syntaxCheck.run()
        syntaxCheck.waitUntilExit()
        precondition(
            syntaxCheck.terminationStatus == 0,
            "The generated DUCX setup terminal script must be valid zsh"
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
        let controller = ProviderSettingsWindowController(
            loadHandler: { response },
            runHandler: { arguments in
                receivedArguments.append(arguments)
                return ""
            },
            applyHandler: {
                applyCount += 1
            },
            ducxSetupHandler: {
                URL(fileURLWithPath: "/managed/ducx/current/bin/ducx")
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
            preconditionFailure("The DUCX reminder must be presented")
        }
        guard let openSettings = descendantViews(of: NSButton.self, in: sheet.contentView)
            .first(where: { $0.title == "下载并配置 DUCX" }) else {
            preconditionFailure("The reminder must offer configuration")
        }

        openSettings.performClick(nil)
        waitUntil {
            !window.isVisible
        }
        precondition(
            receivedArguments.contains(where: {
                $0.starts(with: ["providers", "update", "baidu-oneapi"])
                    && $0.containsSubsequence(["--ducx-app-server", "true"])
                    && $0.containsSubsequence([
                        "--ducx-executable",
                        "/managed/ducx/current/bin/ducx",
                    ])
            }),
            "Opening DUCX configuration must save the enabled DUCX option"
        )
        precondition(
            applyCount == 1,
            "Opening DUCX configuration must apply and restart the gateway"
        )
        precondition(
            !window.isVisible,
            "The provider settings window must close after DUCX setup succeeds"
        )
        precondition(
            completionTitle == "DUCX 已配置",
            "Successful setup must visibly confirm completion"
        )

        controller.close()
        print("Provider DUCX reminder navigation: passed")
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
