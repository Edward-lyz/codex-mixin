import Cocoa

extension AppDelegate {
    @objc func showAgentManagement() {
        if agentManagementWindowController == nil {
            agentManagementWindowController = AgentManagementWindowController(
                model: makeAgentManagementModel()
            )
        }
        agentManagementWindowController?.present()
    }

    private func makeAgentManagementModel() -> AgentManagementModel {
        AgentManagementModel(
            listHandler: { [weak self] in
                guard let self else {
                    throw GatewayError.command("Codex Mixin 已退出")
                }
                return try decodeAgentList(try await self.runGateway(agentListArguments()))
            },
            versionsHandler: { [weak self] in
                guard let self else {
                    throw GatewayError.command("Codex Mixin 已退出")
                }
                return try decodeAgentVersions(try await self.runGateway(agentVersionsArguments()))
            },
            upgradeHandler: { [weak self] entry in
                guard let self else {
                    throw GatewayError.command("Codex Mixin 已退出")
                }
                return try decodeAgentUpgrade(
                    try await self.runGateway(agentUpgradeArguments(id: entry.id))
                )
            },
            connectHandler: { [weak self] entry, mode in
                guard let self else {
                    throw GatewayError.command("Codex Mixin 已退出")
                }
                try await self.connectAgent(entry: entry, mode: mode)
            },
            disconnectHandler: { [weak self] entry in
                guard let self else {
                    throw GatewayError.command("Codex Mixin 已退出")
                }
                try await self.disconnectAgent(entry: entry)
            },
            connectModePicker: { [weak self] entry in
                self?.pickCodexConnectMode(entry)
            },
            confirmHandler: { [weak self] title, message in
                self?.confirmAgentOperation(title: title, message: message) ?? false
            }
        )
    }

    /// Codex connects in one of two modes; the panel is the existing
    /// official-account / custom-only choice.
    @MainActor
    private func pickCodexConnectMode(_ entry: AgentEntry) -> AgentConnectMode? {
        guard agentRequiresConnectMode(id: entry.id) else { return nil }
        return runCodexConnectPanel()
    }

    @MainActor
    private func confirmAgentOperation(title: String, message: String) -> Bool {
        confirm(title: title, message: message)
    }

    @MainActor
    func connectAgent(entry: AgentEntry, mode: AgentConnectMode?) async throws {
        serviceBusy = true
        defer { serviceBusy = false }
        try await runOperationProgress(
            title: AppLocalization.string("agentManagement.connectProgressTitle", entry.name),
            phases: [
                AppLocalization.string("agentManagement.progress.prepareGateway"),
                AppLocalization.string("agentManagement.progress.applyConnection"),
                AppLocalization.string("agentManagement.progress.done"),
            ],
            successTitle: AppLocalization.string("agentManagement.progress.successConnect"),
            failureTitle: AppLocalization.string("agentManagement.progress.failureConnect"),
            showFailureAlert: false,
            failureAlertTitle: AppLocalization.string(
                "agentManagement.connectFailed",
                entry.name,
                ""
            )
        ) { progress in
            progress.advance(to: 0)
            let status = try await ensureGatewayReady()
            applyGatewayStatus(status)
            progress.advance(to: 1)
            _ = try await runGatewayStreaming(
                agentConnectArguments(id: entry.id, mode: mode)
            ) { line in
                progress.advanceStreamedPhase(line)
            }
            progress.advance(to: 2)
            await refreshStatusNow()
        }
    }

    /// Disconnecting one agent restores only that agent's configuration. The
    /// shared local gateway keeps running for every other agent.
    @MainActor
    func disconnectAgent(entry: AgentEntry) async throws {
        serviceBusy = true
        defer { serviceBusy = false }
        try await runOperationProgress(
            title: AppLocalization.string("agentManagement.disconnectProgressTitle", entry.name),
            phases: [
                AppLocalization.string("agentManagement.progress.restoreConfiguration"),
                AppLocalization.string("agentManagement.progress.done"),
            ],
            successTitle: AppLocalization.string("agentManagement.progress.successDisconnect"),
            failureTitle: AppLocalization.string("agentManagement.progress.failureDisconnect"),
            showFailureAlert: false,
            failureAlertTitle: AppLocalization.string(
                "agentManagement.disconnectFailed",
                entry.name,
                ""
            )
        ) { progress in
            progress.advance(to: 0)
            _ = try await runGatewayStreaming(
                agentDisconnectArguments(id: entry.id)
            ) { line in
                progress.advanceStreamedPhase(line)
            }
            progress.advance(to: 1)
            await refreshStatusNow()
        }
    }
}
