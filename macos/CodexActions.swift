import Cocoa

extension AppDelegate {
    @objc func installCodexConfig() {
        guard let installMode = runInstallCodexPanel() else { return }
        Task { @MainActor in
            serviceBusy = true
            serviceStatus = "正在准备 Codex 配置..."
            defer { serviceBusy = false }
            do {
                let status = try await ensureGatewayReady()
                applyGatewayStatus(status)
                _ = try await runGateway(installMode.commandArguments)
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
        guard confirm(title: "从 Codex 恢复官方配置", message: "会恢复安装前备份的 ~/.codex/config.toml，将历史会话迁回原 provider，并删除 Codex Mixin 托管的模型目录。完成后需要重启 Codex App；CLI 需要开新会话。") else { return }
        Task { @MainActor in
            do {
                let output = try await runGateway(["uninstall-codex"])
                let message = output.isEmpty ? "已恢复安装前配置。请重启 Codex App；CLI 需要开新会话。" : "\(output)\n\n请重启 Codex App；CLI 需要开新会话。"
                showAlert(title: "Codex 配置已恢复", message: message)
                refreshStatus()
            } catch {
                showAlert(title: "从 Codex 恢复失败", message: String(describing: error))
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
