import Cocoa

func menuItemImage(_ systemSymbolName: String) -> NSImage? {
    nil
}

@main
struct ProviderSettingsNavigationTests {
    static func main() throws {
        _ = NSApplication.shared
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
                "PATH='/opt/homebrew/bin:/usr/local/bin:/usr/bin:/bin:/usr/sbin:/sbin'"
            ),
            "Terminal validation and App extraction must use the same zstd-capable PATH"
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
        precondition(requiresOpenCodeGoQuotaCredentials("opencode-go"))
        precondition(!requiresOpenCodeGoQuotaCredentials("baidu-oneapi"))
        precondition(
            duccSetupScript.contains("具体错误：")
                && duccSetupScript.contains("'/tmp/ducc-install.error'"),
            "DUCC setup failures must expose the original installer error in Terminal"
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
            applyHandler: { _ in
                applyCount += 1
            },
            baiduBridgeSetupHandler: { mode in
                requestedBridgeModes.append(mode)
                return URL(
                    fileURLWithPath: "/managed/ducc/home/.baidu-cc/baidu-cc/bin/ducc"
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
        guard controller.window?.attachedSheet != nil else {
            preconditionFailure("The auth bridge reminder must be presented")
        }
        guard let configureDucc = controller.baiduBridgeReminderAlert?.buttons.first else {
            preconditionFailure("The reminder must offer DUCC configuration")
        }

        configureDucc.performClick(nil)
        waitUntil {
            controller.window?.isVisible == false
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
                    && $0.containsSubsequence([
                        "--ducc-executable",
                        "/managed/ducc/home/.baidu-cc/baidu-cc/bin/ducc",
                    ])
            }),
            "Opening DUCC configuration must save only the DUCC bridge"
        )
        precondition(
            applyCount == 1,
            "Opening DUCC configuration must apply and restart the gateway"
        )
        precondition(
            controller.window?.isVisible == false,
            "The provider settings window must close after DUCC setup succeeds"
        )
        precondition(
            completionTitle?.contains("DUCC") == true
                && (completionTitle?.contains("Configured") == true || completionTitle?.contains("已") == true),
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
