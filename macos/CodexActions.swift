import Cocoa

extension AppDelegate {
    @objc func installCodexConfig() {
        guard let installMode = runInstallCodexPanel() else { return }
        Task { @MainActor in
            serviceBusy = true
            serviceStatus = "正在准备 Codex 配置..."
            let progressWindow = InstallProgressWindowController()
            progressWindow.present()
            defer { progressWindow.finish() }
            defer { serviceBusy = false }
            do {
                let status = try await ensureGatewayReady()
                applyGatewayStatus(status)
                _ = try await runGatewayStreaming(installMode.commandArguments) { line in
                    progressWindow.update(phase: line)
                }
                showAlert(
                    title: "Codex 配置已更新",
                    message: installMode.completionMessage
                )
                await refreshStatusNow()
            } catch {
                serviceStatus = "安装 Codex 配置失败"
                showAlert(title: "安装到 Codex 失败", message: String(describing: error))
            }
        }
    }

    @objc func uninstallCodexConfig() {
        guard confirm(title: "从 Codex 恢复安装前配置", message: "会恢复安装前备份的 ~/.codex/config.toml；如果使用过“仅自定义模型模式”，也会删除本地登录占位并恢复原 ~/.codex/auth.json。历史会话将迁回原 provider，托管模型目录会被删除。完成后需要重启 Codex App；CLI 需要开新会话。") else { return }
        Task { @MainActor in
            let progressWindow = InstallProgressWindowController(title: "正在从 Codex 恢复")
            progressWindow.present()
            defer { progressWindow.finish() }
            do {
                let output = try await runGatewayStreaming(["uninstall-codex"]) { line in
                    progressWindow.update(phase: line)
                }
                let message = output.isEmpty ? "已恢复安装前配置。请重启 Codex App；CLI 需要开新会话。" : "\(output)\n\n请重启 Codex App；CLI 需要开新会话。"
                showAlert(title: "Codex 配置已恢复", message: message)
                refreshStatus()
            } catch {
                showAlert(title: "从 Codex 恢复失败", message: String(describing: error))
            }
        }
    }

    @objc func installClaudeCode() {
        Task { @MainActor in
            serviceBusy = true
            serviceStatus = "正在准备 Claude Code 配置..."
            defer { serviceBusy = false }
            do {
                let status = try await ensureGatewayReady()
                applyGatewayStatus(status)
                _ = try await runGateway(["install-claude"])
                showAlert(
                    title: "Claude Code 配置已更新",
                    message: "已把 ANTHROPIC_BASE_URL 指向本地网关。请重启 Claude Code；在模型设置中选择 codex-mixin 中已配置的模型，例如 Claude Sonnet 5。"
                )
                await refreshStatusNow()
            } catch {
                serviceStatus = "安装 Claude Code 配置失败"
                showAlert(title: "安装到 Claude Code 失败", message: String(describing: error))
            }
        }
    }

    @objc func uninstallClaudeCode() {
        guard confirm(title: "从 Claude Code 恢复配置", message: "会删除 ~/.claude/settings.json 中的 ANTHROPIC_BASE_URL 和 codex_mixin_managed 标记。其他 Claude Code 设置会保留。完成后需要重启 Claude Code。") else { return }
        Task { @MainActor in
            do {
                let output = try await runGateway(["uninstall-claude"])
                let message = output.isEmpty ? "已恢复 Claude Code 配置。请重启 Claude Code。" : "\(output)\n\n请重启 Claude Code。"
                showAlert(title: "Claude Code 配置已恢复", message: message)
                refreshStatus()
            } catch {
                showAlert(title: "从 Claude Code 恢复失败", message: String(describing: error))
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
