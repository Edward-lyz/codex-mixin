import Cocoa

enum CodexIntegration: String, Decodable, Equatable {
    case unmanaged
    case managed
}

enum ManagedCodexGatewayMode: String, Decodable, Equatable {
    case codexOAuthProxy = "codex_oauth_proxy"
    case customOnly = "custom_only"
}

struct CodexIntegrationStatus: Decodable, Equatable {
    let integration: CodexIntegration
    let mode: ManagedCodexGatewayMode?
    let gatewayRequired: Bool
    let restoreMode: ManagedCodexGatewayMode?

    enum CodingKeys: String, CodingKey {
        case integration
        case mode
        case gatewayRequired = "gateway_required"
        case restoreMode = "restore_mode"
    }

    var isOfficialMode: Bool {
        integration == .unmanaged && restoreMode != nil
    }
}

func decodeCodexIntegrationStatus(_ json: String) throws -> CodexIntegrationStatus {
    do {
        return try JSONDecoder().decode(CodexIntegrationStatus.self, from: Data(json.utf8))
    } catch {
        throw GatewayError.command("Codex 接入状态 JSON 无法解析：\(error)")
    }
}

func stoppedGatewayStatusTitle(codexStatus: CodexIntegrationStatus?) -> String {
    codexStatus?.isOfficialMode == true ? "官方 Codex 模式" : "本地服务已停止"
}

func providerDisplayNameForGatewayState(
    _ displayName: String,
    isGatewayRunning: Bool,
    codexStatus: CodexIntegrationStatus?
) -> String {
    guard !isGatewayRunning, codexStatus?.isOfficialMode == true else { return displayName }
    return "\(displayName)（Mixin 配置已停用）"
}

extension AppDelegate {
    func readCodexIntegrationStatus() async throws -> CodexIntegrationStatus {
        try decodeCodexIntegrationStatus(
            try await runGateway(["codex-status", "--json"])
        )
    }

    @MainActor
    func refreshCodexIntegrationStatus() async throws -> CodexIntegrationStatus {
        let status = try await readCodexIntegrationStatus()
        codexIntegrationStatus = status
        return status
    }

    @MainActor
    func disableGatewayFromSwitch() {
        serviceStatus = "正在切回官方 Codex..."
        serviceEndpoint = nil
        serviceBusy = true
        Task { @MainActor in
            defer { serviceBusy = false }
            do {
                try await runOperationProgress(
                    title: "正在停用 Codex Mixin",
                    phases: [
                        "保存当前 Codex 接入模式",
                        "恢复官方 Codex 配置与认证",
                        "校验官方配置",
                        "停止本地网关",
                        "重启 Codex App",
                        "完成",
                    ],
                    detail: "Codex App 会重启，当前会话将中断",
                    successTitle: "✓ 已切回官方 Codex",
                    failureTitle: "✗ 切换未完成",
                    showFailureAlert: true,
                    failureAlertTitle: "无法安全停用 Codex Mixin"
                ) { progress in
                    progress.advance(to: 0)
                    _ = try await runGatewayStreaming(["codex-switch", "official"]) { line in
                        progress.advanceStreamedPhase(line)
                    }

                    let restored = try await refreshCodexIntegrationStatus()
                    guard restored.integration == .unmanaged,
                          restored.restoreMode != nil,
                          !restored.gatewayRequired
                    else {
                        throw GatewayError.command(
                            "CLI 未确认 Codex 已恢复为官方直连模式。"
                        )
                    }

                    progress.advance(to: 4)
                    _ = try await restartRunningCodexDesktopApp()

                    progress.advance(to: 5)
                    isRunning = false
                    providerStatusDetail = nil
                    serviceStatus = "已切回官方 Codex；本地网关已停止"
                    updateStatusTitle()
                }
            } catch {
                _ = try? await refreshCodexIntegrationStatus()
                await refreshStatusNow()
            }
        }
    }

    @MainActor
    func enableGatewayFromSwitch() {
        serviceStatus = "正在重新接入 Codex Mixin..."
        serviceEndpoint = nil
        serviceBusy = true
        Task { @MainActor in
            defer { serviceBusy = false }
            do {
                try await runOperationProgress(
                    title: "正在启用 Codex Mixin",
                    phases: [
                        "启动本地网关",
                        "恢复 Codex Mixin 接入",
                        "校验 Codex 配置",
                        "重启 Codex App",
                        "完成",
                    ],
                    detail: "Codex App 会重启，当前会话将中断",
                    successTitle: "✓ Codex Mixin 已启用",
                    failureTitle: "✗ Codex Mixin 接入失败",
                    showFailureAlert: true,
                    failureAlertTitle: "无法重新启用 Codex Mixin"
                ) { progress in
                    progress.advance(to: 0)
                    _ = try await runGatewayStreaming(["codex-switch", "mixin"]) { line in
                        progress.advanceStreamedPhase(line)
                    }

                    let restored = try await refreshCodexIntegrationStatus()
                    guard restored.integration == .managed,
                          restored.mode != nil,
                          restored.gatewayRequired,
                          restored.restoreMode == nil
                    else {
                        throw GatewayError.command(
                            "CLI 未确认 Codex 已恢复为 Mixin 接入模式。"
                        )
                    }

                    progress.advance(to: 3)
                    _ = try await restartRunningCodexDesktopApp()

                    progress.advance(to: 4)
                    await refreshStatusNow()
                }
            } catch {
                _ = try? await refreshCodexIntegrationStatus()
                await refreshStatusNow()
            }
        }
    }

    @MainActor
    private func restartRunningCodexDesktopApp() async throws -> String? {
        guard let app = NSWorkspace.shared.runningApplications.first(where: {
            $0.bundleIdentifier == "com.openai.codex"
        }) else {
            return nil
        }
        guard let appURL = app.bundleURL else {
            throw GatewayError.command("无法定位正在运行的 Codex App。")
        }
        guard app.terminate() else {
            throw GatewayError.command("无法退出 Codex App；请关闭阻挡退出的窗口后重试。")
        }

        for _ in 0..<60 {
            if app.isTerminated { break }
            try await Task.sleep(nanoseconds: 250_000_000)
        }
        guard app.isTerminated else {
            throw GatewayError.command("Codex App 在 15 秒内没有退出；请手动重启。")
        }

        try await openDesktopApplication(at: appURL)
        return app.localizedName ?? "Codex"
    }

    @MainActor
    private func openDesktopApplication(at url: URL) async throws {
        try await withCheckedThrowingContinuation { (continuation: CheckedContinuation<Void, Error>) in
            let configuration = NSWorkspace.OpenConfiguration()
            configuration.activates = true
            NSWorkspace.shared.openApplication(
                at: url,
                configuration: configuration
            ) { _, error in
                if let error {
                    continuation.resume(throwing: error)
                } else {
                    continuation.resume(returning: ())
                }
            }
        }
    }
}
