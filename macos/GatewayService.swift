import Cocoa
import UniformTypeIdentifiers

extension AppDelegate {
    @objc func startService() {
        serviceStatus = "本地网关启动中..."
        serviceEndpoint = nil
        serviceBusy = true
        Task { @MainActor in
            defer { serviceBusy = false }
            do {
                try await runOperationProgress(
                    title: "正在启动本地网关",
                    phases: [
                        "准备配置",
                        "启动网关",
                        "等待就绪",
                    ],
                    successTitle: "✓ 网关已启动",
                    failureTitle: "✗ 启动失败",
                    showFailureAlert: true,
                    failureAlertTitle: "启动服务失败"
                ) { progress in
                    progress.advance(to: 0)
                    progress.advance(to: 1)
                    let status = try await runGateway(["service", "start", "--managed", "--json"])
                    progress.advance(to: 2)
                    applyGatewayStatus(status)
                    await refreshStatusNow()
                }
            } catch {
                isRunning = false
                serviceStatus = "本地网关启动失败"
                serviceEndpoint = nil
                updateStatusTitle()
            }
        }
    }

    @objc func restartService() {
        serviceStatus = "本地网关重启中..."
        serviceEndpoint = nil
        serviceBusy = true
        Task { @MainActor in
            defer { serviceBusy = false }
            do {
                try await runOperationProgress(
                    title: "正在重启本地网关",
                    phases: [
                        "停止旧进程",
                        "启动网关",
                        "等待就绪",
                    ],
                    successTitle: "✓ 网关已重启",
                    failureTitle: "✗ 重启失败",
                    showFailureAlert: true,
                    failureAlertTitle: "重启服务失败"
                ) { progress in
                    progress.advance(to: 0)
                    let status = try await runGateway(["service", "restart", "--managed", "--json"])
                    progress.advance(to: 1)
                    progress.advance(to: 2)
                    applyGatewayStatus(status)
                    await refreshStatusNow()
                }
            } catch {
                isRunning = false
                serviceStatus = "本地网关重启失败"
                serviceEndpoint = nil
                updateStatusTitle()
            }
        }
    }

    @objc func stopService() {
        stopGateway(allowCodexDisconnect: false)
    }

    /// `allowCodexDisconnect` is the explicit "stop only" choice: Codex keeps
    /// its Mixin config and cannot reach GPT until the gateway starts again.
    func stopGateway(allowCodexDisconnect: Bool) {
        serviceStatus = "本地网关停止中..."
        serviceEndpoint = nil
        serviceBusy = true
        Task { @MainActor in
            defer { serviceBusy = false }
            do {
                try await runOperationProgress(
                    title: "正在停止本地网关",
                    phases: [
                        "停止网关进程",
                        "等待退出",
                        "完成",
                    ],
                    successTitle: "✓ 网关已停止",
                    failureTitle: "✗ 停止失败",
                    showFailureAlert: true,
                    failureAlertTitle: "停止服务失败"
                ) { progress in
                    progress.advance(to: 0)
                    let arguments = allowCodexDisconnect
                        ? ["service", "stop", "--managed", "--allow-codex-disconnect", "--json"]
                        : ["service", "stop", "--managed", "--json"]
                    _ = try await runGateway(arguments)
                    progress.advance(to: 1)
                    progress.advance(to: 2)
                    isRunning = false
                    providerStatusDetail = nil
                    serviceStatus = allowCodexDisconnect
                        ? stoppedGatewayStatusTitle(codexStatus: codexIntegrationStatus)
                        : "本地网关已停止"
                    updateStatusTitle()
                }
            } catch {
                await refreshStatusNow()
            }
        }
    }

    @objc func toggleLaunchAtLogin() {
        serviceBusy = true
        Task { @MainActor in
            defer { serviceBusy = false }
            do {
                try await runOperationProgress(
                    title: "正在更新登录自启",
                    phases: ["更新登录启动", "应用网关状态", "完成"],
                    successTitle: "✓ 登录自启已更新",
                    failureTitle: "✗ 更新失败",
                    showFailureAlert: true,
                    failureAlertTitle: "更新登录自启失败"
                ) { progress in
                    progress.advance(to: 0)
                    let enabled = !FileManager.default.fileExists(atPath: menuLaunchAgentPath().path)
                    _ = try await runGateway([
                        "service", "autostart", enabled ? "enable" : "disable", "--json",
                    ])
                    progress.advance(to: 1)
                    if enabled {
                        try installMenuLaunchAgent()
                    } else {
                        try await bootoutIfLoaded(menuLaunchDomainAndLabel())
                        if FileManager.default.fileExists(atPath: menuLaunchAgentPath().path) {
                            try FileManager.default.removeItem(at: menuLaunchAgentPath())
                        }
                    }
                    progress.advance(to: 2)
                    await refreshStatusNow()
                }
            } catch {
                // Failure already shown by the progress window + alert.
            }
        }
    }

    @objc func refreshStatus() {
        Task { @MainActor in
            await refreshStatusNow()
        }
    }

    @objc func openLogs() {
        Task { @MainActor in
            do {
                let paths = try await interfacePaths()
                guard let path = paths["gateway_log"], !path.isEmpty else {
                    throw GatewayError.command("CLI did not return the gateway log path")
                }
                let logURL = URL(fileURLWithPath: path)
                guard FileManager.default.fileExists(atPath: logURL.path) else {
                    showAlert(title: "日志还不存在", message: "本地网关启动后会写入 \(logURL.path)。")
                    return
                }
                NSWorkspace.shared.open(logURL)
            } catch {
                showAlert(title: "打开日志失败", message: String(describing: error))
            }
        }
    }

    @objc func openConfigFolder() {
        Task { @MainActor in
            do {
                let paths = try await interfacePaths()
                guard let path = paths["state"], !path.isEmpty else {
                    throw GatewayError.command("CLI did not return the state directory")
                }
                NSWorkspace.shared.open(URL(fileURLWithPath: path))
            } catch {
                showAlert(title: "打开配置目录失败", message: String(describing: error))
            }
        }
    }

    @objc func exportConfigBackup() {
        let panel = NSSavePanel()
        panel.title = "导出配置备份"
        panel.message = "备份使用 Base64 编码但未加密，包含 API Key、AWS 凭据和本地访问密钥，请妥善保管。"
        panel.nameFieldStringValue = "codex-mixin-config.b64"
        panel.allowedContentTypes = [UTType(filenameExtension: "b64") ?? .data]
        guard panel.runModal() == .OK, let destination = panel.url else { return }
        Task { @MainActor in
            do {
                _ = try await runGateway(["config", "export", destination.path])
                showAlert(
                    title: "配置备份已导出",
                    message: "Base64 备份已保存到 \(destination.path)，文件权限为仅当前用户可读写。Base64 不是加密，请勿公开分享。"
                )
            } catch {
                showAlert(title: "导出配置备份失败", message: String(describing: error))
            }
        }
    }

    @objc func importConfigBackup() {
        let panel = NSOpenPanel()
        panel.title = "导入配置备份"
        panel.message = "选择 Codex Mixin 导出的 Base64 配置备份。"
        panel.allowedContentTypes = [UTType(filenameExtension: "b64") ?? .data]
        panel.allowsMultipleSelection = false
        panel.canChooseDirectories = false
        guard panel.runModal() == .OK, let source = panel.url else { return }
        guard confirm(
            title: "导入配置备份？",
            message: "这会替换当前 Provider、模型选择、Fusion 和凭据配置，然后重启本地网关。"
        ) else { return }

        serviceBusy = true
        Task { @MainActor in
            var imported = false
            defer { serviceBusy = false }
            do {
                _ = try await runGateway(["config", "import", source.path])
                imported = true
                let status = try await runGateway(["service", "restart", "--managed", "--json"])
                applyGatewayStatus(status)
                await refreshStatusNow()
                showAlert(
                    title: "配置备份已导入",
                    message: "配置已安全写入并应用，本地网关已重启。"
                )
            } catch {
                showAlert(
                    title: imported ? "配置已导入，但应用失败" : "导入配置备份失败",
                    message: String(describing: error)
                )
                await refreshStatusNow()
            }
        }
    }

    @objc func quit() {
        quitApplication(restoreClients: false)
    }

    @objc func quitAndRestore() {
        guard confirm(
            title: "退出并恢复原配置？",
            message: "会先把 Codex、Claude Code、DSH、OpenCode、Pi 中由 Codex Mixin 接管的配置恢复为安装前的状态，再停止本地网关并退出。正在运行的 Codex App 会自动重启；其他客户端需要重启或开新会话。"
        ) else { return }
        quitApplication(restoreClients: true)
    }

    /// Quit is a real quit: the gateway stops even when a client still routes
    /// through it. `restoreClients` first restores every managed client config.
    private func quitApplication(restoreClients: Bool) {
        serviceBusy = true
        serviceStatus = restoreClients ? "正在恢复原配置并退出..." : "正在停止本地网关并退出..."
        serviceEndpoint = nil
        Task { @MainActor in
            do {
                var codexWasManaged = false
                if restoreClients {
                    codexWasManaged = try await readCodexIntegrationStatus().integration == .managed
                }
                let arguments = restoreClients
                    ? ["service", "stop", "--managed", "--restore-clients", "--json"]
                    : ["service", "stop", "--managed", "--allow-codex-disconnect", "--json"]
                _ = try await runGateway(arguments)
                if codexWasManaged {
                    do {
                        _ = try await restartRunningCodexDesktopApp()
                    } catch {
                        showAlert(
                            title: "Codex App 未能自动重启",
                            message: "原配置已恢复，本地网关已停止。请手动重启 Codex App：\(error)"
                        )
                    }
                }
                NSApp.terminate(nil)
            } catch {
                serviceBusy = false
                await refreshStatusNow()
                showAlert(title: "退出 Codex Mixin 失败", message: "本地网关未能停止：\(error)")
            }
        }
    }

}
