import Cocoa

extension AppDelegate {
    @MainActor
    @objc func installCodexConfig() {
        guard let installMode = runInstallCodexPanel() else { return }
        Task { @MainActor in
            serviceBusy = true
            serviceStatus = "正在准备 Codex 配置..."
            defer { serviceBusy = false }
            do {
                try await runOperationProgress(
                    title: "正在安装到 Codex",
                    phases: [
                        "检查本地配置与网关状态",
                        "获取 Codex 配置模板",
                        "获取可用模型列表",
                        "加载模型元数据",
                        "准备或安装 Codex CLI",
                        "写入 Codex 配置和模型目录",
                        "同步历史会话与 SQLite 状态",
                        "校验安装结果",
                    ],
                    detail: "安装期间请勿打开 Codex App",
                    successTitle: "✓ 安装完成",
                    failureTitle: "✗ 安装失败",
                    showFailureAlert: true,
                    failureAlertTitle: "安装到 Codex 失败"
                ) { progress in
                    let status = try await ensureGatewayReady()
                    applyGatewayStatus(status)
                    progress.advance(to: 0)
                    _ = try await runGatewayStreaming(installMode.commandArguments) { line in
                        progress.advanceStreamedPhase(line)
                    }
                    showAlert(
                        title: "Codex 配置已更新",
                        message: installMode.completionMessage
                    )
                    await refreshStatusNow()
                }
            } catch {
                serviceStatus = "安装 Codex 配置失败"
            }
        }
    }

    @objc func uninstallCodexConfig() {
        guard confirm(
            title: "从 Codex 恢复安装前配置",
            message: "会恢复安装前备份的 ~/.codex/config.toml；如果使用过“仅自定义模型模式”，也会删除本地登录占位并恢复原 ~/.codex/auth.json。历史会话将迁回原 provider，托管模型目录会被删除。完成后需要重启 Codex App；CLI 需要开新会话。"
        ) else { return }
        Task { @MainActor in
            serviceBusy = true
            defer { serviceBusy = false }
            do {
                try await runOperationProgress(
                    title: "正在从 Codex 恢复",
                    phases: [
                        "读取并锁定 Codex 配置",
                        "恢复安装前配置与登录状态",
                        "恢复历史会话与 SQLite 状态",
                    ],
                    successTitle: "✓ 恢复完成",
                    failureTitle: "✗ 恢复失败",
                    showFailureAlert: true,
                    failureAlertTitle: "从 Codex 恢复失败"
                ) { progress in
                    let output = try await runGatewayStreaming(["uninstall-codex"]) { line in
                        progress.advanceStreamedPhase(line)
                    }
                    let message = output.isEmpty
                        ? "已恢复安装前配置。请重启 Codex App；CLI 需要开新会话。"
                        : "\(output)\n\n请重启 Codex App；CLI 需要开新会话。"
                    showAlert(title: "Codex 配置已恢复", message: message)
                    refreshStatus()
                }
            } catch {
                // Failure already shown by the progress window + alert.
            }
        }
    }

    @objc func installClaudeCode() {
        Task { @MainActor in
            serviceBusy = true
            serviceStatus = "正在准备 Claude Code 配置..."
            defer { serviceBusy = false }
            do {
                try await runOperationProgress(
                    title: "正在安装到 Claude Code",
                    phases: [
                        "准备网关",
                        "写入 Claude 配置",
                        "完成",
                    ],
                    successTitle: "✓ 安装完成",
                    failureTitle: "✗ 安装失败",
                    showFailureAlert: true,
                    failureAlertTitle: "安装到 Claude Code 失败"
                ) { progress in
                    progress.advance(to: 0)
                    let status = try await ensureGatewayReady()
                    applyGatewayStatus(status)
                    progress.advance(to: 1)
                    _ = try await runGateway(["install-claude"])
                    progress.advance(to: 2)
                    showAlert(
                        title: "Claude Code 配置已更新",
                        message: "已写入可用模型列表，配置本地网关认证，隐藏官方登录入口，并禁用非必要官方流量。请重启 Claude Code 或开新会话。"
                    )
                    await refreshStatusNow()
                }
            } catch {
                serviceStatus = "安装 Claude Code 配置失败"
                showAlert(
                    title: "安装到 Claude Code 失败",
                    message: String(describing: error)
                )
            }
        }
    }

    @objc func uninstallClaudeCode() {
        guard confirm(
            title: "从 Claude Code 恢复配置",
            message: "会恢复 Codex Mixin 管理的模型列表、网关认证和流量设置，并保留其他 Claude Code 配置。完成后需要重启 Claude Code。"
        ) else { return }
        Task { @MainActor in
            serviceBusy = true
            defer { serviceBusy = false }
            do {
                try await runOperationProgress(
                    title: "正在从 Claude Code 恢复",
                    phases: [
                        "准备恢复",
                        "恢复 Claude 配置",
                        "完成",
                    ],
                    successTitle: "✓ 恢复完成",
                    failureTitle: "✗ 恢复失败",
                    showFailureAlert: true,
                    failureAlertTitle: "从 Claude Code 恢复失败"
                ) { progress in
                    progress.advance(to: 0)
                    progress.advance(to: 1)
                    let output = try await runGateway(["uninstall-claude"])
                    progress.advance(to: 2)
                    let message = output.isEmpty
                        ? "已恢复 Claude Code 配置。请重启 Claude Code。"
                        : "\(output)\n\n请重启 Claude Code。"
                    showAlert(title: "Claude Code 配置已恢复", message: message)
                    refreshStatus()
                }
            } catch {
                // Failure already shown by the progress window + alert.
            }
        }
    }

    @objc func installDsh() {
        Task { @MainActor in
            serviceBusy = true
            serviceStatus = "正在准备 DSH 配置..."
            defer { serviceBusy = false }
            do {
                try await runOperationProgress(
                    title: "正在安装到 DSH",
                    phases: [
                        "准备网关",
                        "写入 DSH settings",
                        "写入 DSH credentials",
                        "完成",
                    ],
                    successTitle: "✓ 安装完成",
                    failureTitle: "✗ 安装失败",
                    showFailureAlert: true,
                    failureAlertTitle: "安装到 DSH 失败"
                ) { progress in
                    progress.advance(to: 0)
                    let status = try await ensureGatewayReady()
                    applyGatewayStatus(status)
                    progress.advance(to: 1)
                    _ = try await runGateway(["connect", "dsh"])
                    progress.advance(to: 2)
                    progress.advance(to: 3)
                    showAlert(
                        title: "DSH 配置已更新",
                        message: "已把本地网关注册为 DSH 的 codex-mixin provider。请重启 DSH 或开新会话，然后在模型选择器中选择 Codex Mixin 模型。"
                    )
                    await refreshStatusNow()
                }
            } catch {
                serviceStatus = "安装 DSH 配置失败"
            }
        }
    }

    @objc func uninstallDsh() {
        guard confirm(
            title: "从 DSH 卸载",
            message: "会从 DSH settings.yaml 删除 llm-pi-ai.providers.codex-mixin，并从 .credentials.yaml 删除 CODEX_MIXIN_GATEWAY_API_KEY。其他 DSH 配置会保留。完成后需要重启 DSH。"
        ) else { return }
        Task { @MainActor in
            serviceBusy = true
            defer { serviceBusy = false }
            do {
                try await runOperationProgress(
                    title: "正在从 DSH 卸载",
                    phases: [
                        "读取 DSH 配置",
                        "移除 codex-mixin provider",
                        "清理 DSH credentials",
                        "完成",
                    ],
                    successTitle: "✓ 卸载完成",
                    failureTitle: "✗ 卸载失败",
                    showFailureAlert: true,
                    failureAlertTitle: "从 DSH 卸载失败"
                ) { progress in
                    progress.advance(to: 0)
                    progress.advance(to: 1)
                    let output = try await runGateway(["connect", "remove", "dsh"])
                    progress.advance(to: 2)
                    progress.advance(to: 3)
                    let message = output.isEmpty
                        ? "已从 DSH 移除 codex-mixin provider。请重启 DSH。"
                        : "\(output)\n\n请重启 DSH。"
                    showAlert(title: "DSH 配置已恢复", message: message)
                    refreshStatus()
                }
            } catch {
                // Failure already shown by the progress window + alert.
            }
        }
    }

    @objc func installOpenCode() {
        Task { @MainActor in
            serviceBusy = true
            serviceStatus = "正在准备 OpenCode 配置..."
            defer { serviceBusy = false }
            do {
                try await runOperationProgress(
                    title: "正在安装到 OpenCode",
                    phases: [
                        "准备网关",
                        "写入 OpenCode provider",
                        "写入模型与思考强度",
                        "完成",
                    ],
                    successTitle: "✓ 安装完成",
                    failureTitle: "✗ 安装失败",
                    showFailureAlert: true,
                    failureAlertTitle: "安装到 OpenCode 失败"
                ) { progress in
                    progress.advance(to: 0)
                    let status = try await ensureGatewayReady()
                    applyGatewayStatus(status)
                    progress.advance(to: 1)
                    _ = try await runGateway(["connect", "opencode"])
                    progress.advance(to: 2)
                    progress.advance(to: 3)
                    showAlert(
                        title: "OpenCode 配置已更新",
                        message: "已把本地网关注册为 OpenCode 的 codex-mixin provider，并加入当前已选模型与 none 到 max 思考强度。请重启 OpenCode 或开新会话，然后用 /models 选择模型、用 variant_cycle 切换思考强度。"
                    )
                    await refreshStatusNow()
                }
            } catch {
                serviceStatus = "安装 OpenCode 配置失败"
            }
        }
    }

    @objc func uninstallOpenCode() {
        guard confirm(
            title: "从 OpenCode 卸载",
            message: "会从 OpenCode 全局配置删除 Codex Mixin 管理的 codex-mixin provider 和本地网关凭据文件，其他 OpenCode 配置会保留。完成后需要重启 OpenCode。"
        ) else { return }
        Task { @MainActor in
            serviceBusy = true
            defer { serviceBusy = false }
            do {
                try await runOperationProgress(
                    title: "正在从 OpenCode 卸载",
                    phases: [
                        "读取 OpenCode 配置",
                        "移除 codex-mixin provider",
                        "清理本地网关凭据",
                        "完成",
                    ],
                    successTitle: "✓ 卸载完成",
                    failureTitle: "✗ 卸载失败",
                    showFailureAlert: true,
                    failureAlertTitle: "从 OpenCode 卸载失败"
                ) { progress in
                    progress.advance(to: 0)
                    progress.advance(to: 1)
                    let output = try await runGateway(["connect", "remove", "opencode"])
                    progress.advance(to: 2)
                    progress.advance(to: 3)
                    let message = output.isEmpty
                        ? "已从 OpenCode 移除 codex-mixin provider。请重启 OpenCode。"
                        : "\(output)\n\n请重启 OpenCode。"
                    showAlert(title: "OpenCode 配置已恢复", message: message)
                    refreshStatus()
                }
            } catch {
                // Failure already shown by the progress window + alert.
            }
        }
    }

    @objc func installPi() {
        Task { @MainActor in
            serviceBusy = true
            serviceStatus = "正在准备 Pi 配置..."
            defer { serviceBusy = false }
            do {
                try await runOperationProgress(
                    title: "正在安装到 Pi",
                    phases: [
                        "准备网关",
                        "写入 Pi provider 和模型",
                        "安装 Pi 用量上报 Hooks",
                        "完成",
                    ],
                    successTitle: "✓ 安装完成",
                    failureTitle: "✗ 安装失败",
                    showFailureAlert: true,
                    failureAlertTitle: "安装到 Pi 失败"
                ) { progress in
                    progress.advance(to: 0)
                    let status = try await ensureGatewayReady()
                    applyGatewayStatus(status)
                    progress.advance(to: 1)
                    _ = try await runGateway(["connect", "pi"])
                    progress.advance(to: 2)
                    progress.advance(to: 3)
                    showAlert(
                        title: "Pi 配置已更新",
                        message: "已把本地网关注册为 Pi 的 codex-mixin provider，并安装 query、代码修改和 transcript 用量上报 Hooks。请在 Pi 中运行 /reload 或开启新会话，然后选择 codex-mixin 模型。"
                    )
                    await refreshStatusNow()
                }
            } catch {
                serviceStatus = "安装 Pi 配置失败"
            }
        }
    }

    @objc func uninstallPi() {
        guard confirm(
            title: "从 Pi 卸载",
            message: "会从 Pi models.json 删除 Codex Mixin 管理的 provider，并删除本地网关凭据和用量上报 Hooks。其他 Pi 配置会保留。完成后请运行 /reload 或开启新会话。"
        ) else { return }
        Task { @MainActor in
            serviceBusy = true
            defer { serviceBusy = false }
            do {
                try await runOperationProgress(
                    title: "正在从 Pi 卸载",
                    phases: [
                        "读取 Pi 配置",
                        "移除 codex-mixin provider",
                        "清理凭据和上报 Hooks",
                        "完成",
                    ],
                    successTitle: "✓ 卸载完成",
                    failureTitle: "✗ 卸载失败",
                    showFailureAlert: true,
                    failureAlertTitle: "从 Pi 卸载失败"
                ) { progress in
                    progress.advance(to: 0)
                    progress.advance(to: 1)
                    let output = try await runGateway(["connect", "remove", "pi"])
                    progress.advance(to: 2)
                    progress.advance(to: 3)
                    let message = output.isEmpty
                        ? "已从 Pi 移除 codex-mixin provider 和用量上报 Hooks。请运行 /reload 或开启新会话。"
                        : "\(output)\n\n请运行 /reload 或开启新会话。"
                    showAlert(title: "Pi 配置已恢复", message: message)
                    refreshStatus()
                }
            } catch {
                // Failure already shown by the progress window + alert.
            }
        }
    }

    @objc func copyLocalEndpoint() {
        Task { @MainActor in
            do {
                let output = try await runGateway(["config", "--json", "--scope", "effective"])
                let data = Data(output.utf8)
                guard
                    let object = try JSONSerialization.jsonObject(with: data) as? [String: Any],
                    let bind = object["bind"] as? String
                else {
                    throw GatewayError.command("无法从有效配置中读取本地网关端口")
                }
                NSPasteboard.general.clearContents()
                NSPasteboard.general.setString("http://\(bind)/v1", forType: .string)
            } catch {
                showAlert(title: "复制本地接口失败", message: String(describing: error))
            }
        }
    }
}
