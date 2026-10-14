import Cocoa
import Combine

private struct FakeError: Error, CustomStringConvertible {
    let description: String
}

/// Blocks a handler until the test releases it, so re-entry can be tested
/// without timing guesses.
@MainActor
private final class OperationGate {
    private var continuation: CheckedContinuation<Void, Never>?
    private(set) var entered = false

    func enter() async {
        entered = true
        await withCheckedContinuation { continuation in
            self.continuation = continuation
        }
    }

    func waitUntilEntered() async {
        while !entered {
            await Task.yield()
        }
    }

    func open() {
        continuation?.resume()
        continuation = nil
    }
}

@MainActor
private final class CommandRecorder {
    var commands: [[String]] = []
    var connectModes: [AgentConnectMode?] = []
    var pickerRequests: [String] = []
}

private func cliInfo(
    version: String,
    latest: String = "",
    via: String = "npm",
    path: String = "/usr/local/bin/agent",
    command: String = "npm install -g agent",
    appVersion: String? = nil,
    update: Bool = false,
    error: String? = nil
) -> AgentCLIInfo {
    AgentCLIInfo(
        version: version,
        latest: latest,
        via: via,
        path: path,
        command: command,
        appVersion: appVersion,
        update: update,
        error: error
    )
}

private func agentEntry(
    id: String,
    name: String,
    detected: Bool = true,
    state: AgentIntegrationState = .unmanaged,
    mode: String? = nil,
    detail: String = "",
    integrationError: String? = nil,
    cli: AgentCLIInfo
) -> AgentEntry {
    AgentEntry(
        id: id,
        name: name,
        detected: detected,
        integration: AgentIntegration(
            state: state,
            mode: mode,
            detail: detail,
            error: integrationError
        ),
        cli: cli
    )
}

private func upgradableEntry(
    id: String,
    name: String,
    version: String = "1.0.0",
    latest: String = "1.1.0"
) -> AgentEntry {
    agentEntry(
        id: id,
        name: name,
        cli: cliInfo(version: version, latest: latest, update: true)
    )
}

@main
struct AgentManagementWindowTests {
    @MainActor
    static func main() async {
        _ = NSApplication.shared

        testDecodesListContract()
        testRejectsMalformedJSON()
        testDecodesNullableCLIFields()
        testOrderingAndGrouping()
        testRowPresentationCarriesStatusAndVersions()
        testCommandContract()
        testDisconnectConfirmationStatesRestoreScope()
        await testWindowBuildsContentAndFoldStartsClosed()
        await testRefreshMergesVersions()
        await testSingleUpgradeUpdatesRowAndBlocksReentry()
        await testUpgradeFailureReportsError()
        await testUpgradeAllRunsSequentiallyInOrder()
        await testUpgradeAllContinuesAfterFailure()
        await testConnectRequiresModeForCodexOnly()
        await testCancelledCodexModePanelLeavesAgentUntouched()
        await testDisconnectRestoresOneAgentOnly()

        print("Agent management window: passed")
    }

    // MARK: - CLI contract

    @MainActor
    private static func testDecodesListContract() {
        let json = """
        {
          "agents": [
            {
              "id": "codex",
              "name": "Codex",
              "detected": true,
              "integration": {
                "state": "managed",
                "mode": "codex_oauth_proxy",
                "detail": "恢复 ~/.codex/config.toml 与 auth.json",
                "error": null
              },
              "cli": {
                "version": "0.9.0",
                "latest": "0.10.0",
                "via": "npm",
                "path": "/usr/local/bin/codex",
                "command": "npm install -g @openai/codex",
                "app_version": "1.2.3",
                "update": true,
                "error": null
              }
            },
            {
              "id": "pi",
              "name": "Pi",
              "detected": false,
              "integration": {
                "state": "unmanaged",
                "mode": null,
                "detail": "",
                "error": "本地网关未运行"
              },
              "cli": {
                "version": "",
                "latest": "",
                "via": "",
                "path": "",
                "command": "",
                "update": false,
                "error": "未找到命令"
              }
            }
          ]
        }
        """
        guard let response = try? decodeAgentList(json) else {
            preconditionFailure("agent list JSON must decode")
        }
        precondition(response.agents.count == 2)
        let codex = response.agents[0]
        precondition(codex.id == "codex")
        precondition(codex.detected)
        precondition(codex.integration.state == .managed)
        precondition(codex.integration.mode == "codex_oauth_proxy")
        precondition(codex.integration.error == nil)
        precondition(codex.cli.version == "0.9.0")
        precondition(codex.cli.latest == "0.10.0")
        precondition(codex.cli.appVersion == "1.2.3")
        precondition(codex.cli.update)
        precondition(!codex.cli.isMissing)
        let pi = response.agents[1]
        precondition(!pi.detected)
        precondition(pi.integration.state == .unmanaged)
        precondition(pi.integration.mode == nil)
        precondition(pi.integration.error == "本地网关未运行")
        precondition(pi.cli.appVersion == nil)
        precondition(pi.cli.error == "未找到命令")
        precondition(pi.cli.isMissing)

        let versions = """
        {"agents":[{"id":"codex","cli":{"version":"0.10.0","latest":"0.10.0","via":"npm","path":"/usr/local/bin/codex","command":"npm install -g @openai/codex","app_version":"1.2.3","update":false,"error":null}}]}
        """
        guard let versionResponse = try? decodeAgentVersions(versions) else {
            preconditionFailure("agent versions JSON must decode")
        }
        precondition(versionResponse.agents.first?.cli.version == "0.10.0")
        precondition(versionResponse.agents.first?.cli.update == false)

        let upgrade = """
        {"id":"codex","cli":{"version":"0.10.0","latest":"0.10.0","via":"npm","path":"/usr/local/bin/codex","command":"npm install -g @openai/codex","update":false,"error":null}}
        """
        guard let upgradeResponse = try? decodeAgentUpgrade(upgrade) else {
            preconditionFailure("agent upgrade JSON must decode")
        }
        precondition(upgradeResponse.id == "codex")
        precondition(upgradeResponse.cli.version == "0.10.0")
        precondition(upgradeResponse.cli.appVersion == nil)
    }

    @MainActor
    private static func testRejectsMalformedJSON() {
        var listFailed = false
        do {
            _ = try decodeAgentList(#"{"agents":[{"id":"codex"}]}"#)
        } catch {
            listFailed = true
        }
        precondition(listFailed, "an agent entry without the contract fields must fail fast")

        var stateFailed = false
        do {
            _ = try decodeAgentList(#"{"agents":[{"id":"codex","name":"Codex","detected":true,"integration":{"state":"broken","mode":null,"detail":"","error":null},"cli":{"version":"1","latest":"1","via":"npm","path":"/bin/codex","command":"npm i -g codex","update":false}}]}"#)
        } catch {
            stateFailed = true
        }
        precondition(stateFailed, "an unknown integration state must fail fast")

        var versionsFailed = false
        do {
            _ = try decodeAgentVersions("not json")
        } catch {
            versionsFailed = true
        }
        precondition(versionsFailed)
    }

    /// The CLI serializes the version fields as nullable strings when it
    /// cannot read them; null and a missing key must both land on "unknown"
    /// instead of failing the whole list.
    @MainActor
    private static func testDecodesNullableCLIFields() {
        let nullFields = """
        {
          "agents": [
            {
              "id": "codex",
              "name": "Codex",
              "detected": true,
              "integration": {
                "state": "unmanaged",
                "mode": null,
                "detail": "",
                "error": null
              },
              "cli": {
                "version": null,
                "latest": null,
                "via": null,
                "path": null,
                "command": null,
                "app_version": null,
                "update": false,
                "error": null
              }
            }
          ]
        }
        """
        guard let response = try? decodeAgentList(nullFields) else {
            preconditionFailure("nullable CLI fields must decode")
        }
        let entry = response.agents[0]
        precondition(entry.cli.version.isEmpty)
        precondition(entry.cli.latest.isEmpty)
        precondition(entry.cli.via.isEmpty)
        precondition(entry.cli.path.isEmpty)
        precondition(entry.cli.command.isEmpty)
        precondition(entry.cli.appVersion == nil)
        precondition(entry.cli.update == false)
        precondition(entry.cli.error == nil)
        precondition(entry.cli.isMissing)
        let row = agentRowPresentation(for: entry, isBusy: false)
        precondition(row.cliVersionText == nil)
        precondition(row.updateText == nil)

        let missingFields = """
        {"agents":[{"id":"pi","name":"Pi","detected":false,"integration":{"state":"unmanaged","mode":null,"detail":"","error":null},"cli":{"update":false}}]}
        """
        guard let missingResponse = try? decodeAgentList(missingFields) else {
            preconditionFailure("missing CLI fields must decode as unknown")
        }
        precondition(missingResponse.agents[0].cli.version.isEmpty)
        precondition(missingResponse.agents[0].cli.latest.isEmpty)
        precondition(missingResponse.agents[0].cli.command.isEmpty)
        precondition(missingResponse.agents[0].cli.isMissing)

        let nullVersions = """
        {"agents":[{"id":"codex","cli":{"version":null,"latest":null,"via":null,"path":null,"command":null,"app_version":null,"update":false,"error":null}}]}
        """
        guard let versions = try? decodeAgentVersions(nullVersions) else {
            preconditionFailure("nullable CLI fields in agent versions must decode")
        }
        precondition(versions.agents[0].cli.version.isEmpty)
        precondition(versions.agents[0].cli.update == false)

        let nullUpgrade = """
        {"id":"codex","cli":{"version":null,"latest":null,"via":null,"path":null,"command":null,"update":false}}
        """
        guard let upgrade = try? decodeAgentUpgrade(nullUpgrade) else {
            preconditionFailure("nullable CLI fields in agent upgrade must decode")
        }
        precondition(upgrade.cli.version.isEmpty)
        precondition(upgrade.cli.appVersion == nil)
        precondition(upgrade.cli.error == nil)

        var missingUpdateFailed = false
        do {
            _ = try decodeAgentList("""
            {"agents":[{"id":"codex","name":"Codex","detected":true,"integration":{"state":"unmanaged","mode":null,"detail":"","error":null},"cli":{"version":"1.0.0","latest":"1.0.0","via":"npm","path":"/bin/codex","command":"npm i -g codex"}}]}
            """)
        } catch {
            missingUpdateFailed = true
        }
        precondition(missingUpdateFailed, "update is required: a missing flag must fail fast")
    }

    // MARK: - Ordering and presentation

    @MainActor
    private static func testOrderingAndGrouping() {
        let agents = [
            agentEntry(id: "pi", name: "Pi", cli: cliInfo(version: "1.0.0")),
            agentEntry(
                id: "dsh",
                name: "DSH",
                detected: false,
                cli: cliInfo(version: "", path: "", update: false)
            ),
            upgradableEntry(id: "opencode", name: "OpenCode"),
            agentEntry(id: "zebra", name: "Zebra CLI", cli: cliInfo(version: "2.0.0")),
            agentEntry(id: "codex", name: "Codex", cli: cliInfo(version: "0.9.0")),
            upgradableEntry(id: "claude", name: "Claude Code"),
            agentEntry(id: "claude-desktop", name: "Claude Desktop", cli: cliInfo(version: "1.0.0")),
        ]
        let ordered = orderedAgentEntries(agents).map(\.id)
        precondition(ordered == [
            "codex", "claude", "claude-desktop", "dsh", "opencode", "pi", "zebra",
        ])
        precondition(detectedAgentEntries(agents).map(\.id) == [
            "codex", "claude", "claude-desktop", "opencode", "pi", "zebra",
        ])
        precondition(undetectedAgentEntries(agents).map(\.id) == ["dsh"])
        precondition(upgradableAgentEntries(agents).map(\.id) == ["claude", "opencode"])
        precondition(agentUndetectedFoldTitle(count: 1) == "Agents not detected (1)")
    }

    @MainActor
    private static func testRowPresentationCarriesStatusAndVersions() {
        let managed = agentEntry(
            id: "codex",
            name: "Codex",
            state: .managed,
            mode: "codex_oauth_proxy",
            detail: "GPT 与自定义模型都可用",
            cli: cliInfo(
                version: "0.9.0",
                latest: "0.10.0",
                appVersion: "1.2.3",
                update: true
            )
        )
        let managedRow = agentRowPresentation(for: managed, isBusy: false)
        precondition(managedRow.cliVersionText == "CLI 0.9.0")
        precondition(managedRow.appVersionText == "App 1.2.3")
        precondition(managedRow.updateText == "Update to 0.10.0")
        precondition(managedRow.updateHelp == "Updates with npm install -g agent")
        precondition(managedRow.statusText == "Connected")
        precondition(managedRow.isConnected)
        precondition(!managedRow.statusIsError)
        precondition(managedRow.detailText.contains("Official account mode"))
        precondition(!managedRow.detailText.contains("GPT 与自定义模型都可用"))
        precondition(!managedRow.detailText.contains("CLI 0.9.0"))

        let failing = agentEntry(
            id: "claude",
            name: "Claude Code",
            state: .error,
            integrationError: "网关认证失败",
            cli: cliInfo(version: "1.0.0", error: "无法读取版本")
        )
        let failingRow = agentRowPresentation(for: failing, isBusy: false)
        precondition(failingRow.statusText == "Connection error")
        precondition(failingRow.statusIsError)
        precondition(failingRow.cliVersionText == "1.0.0")
        precondition(!failingRow.isConnected)
        precondition(failingRow.detailText.contains("网关认证失败"))
        precondition(failingRow.detailText.contains("Version check failed: 无法读取版本"))
        precondition(failingRow.updateText == nil)

        let missing = agentEntry(
            id: "pi",
            name: "Pi",
            detected: false,
            cli: cliInfo(version: "", path: "", update: false)
        )
        let missingRow = agentRowPresentation(for: missing, isBusy: true)
        precondition(missingRow.cliVersionText == nil)
        precondition(missing.cli.isMissing)
        precondition(missingRow.statusText == "Not connected")
        precondition(missingRow.isBusy)
        precondition(missingRow.updateText == nil)
    }

    @MainActor
    private static func testCommandContract() {
        precondition(agentListArguments() == ["agent", "list", "--json"])
        precondition(agentVersionsArguments() == ["agent", "versions", "--json", "--refresh"])
        precondition(agentUpgradeArguments(id: "dsh") == ["agent", "upgrade", "dsh", "--json"])
        precondition(
            agentConnectArguments(id: "codex", mode: .officialAccount)
                == ["agent", "connect", "codex", "--mode", "official-account"]
        )
        precondition(
            agentConnectArguments(id: "codex", mode: .customOnly)
                == ["agent", "connect", "codex", "--mode", "custom-only"]
        )
        precondition(
            agentConnectArguments(id: "claude", mode: .officialAccount)
                == ["agent", "connect", "claude"]
        )
        precondition(
            agentConnectArguments(id: "claude", mode: nil) == ["agent", "connect", "claude"]
        )
        precondition(agentDisconnectArguments(id: "pi") == ["agent", "disconnect", "pi"])
        for id in AgentCatalog.knownOrder {
            let disconnect = agentDisconnectArguments(id: id)
            precondition(!disconnect.contains("service"))
            precondition(!disconnect.contains("stop"))
        }
    }

    @MainActor
    private static func testDisconnectConfirmationStatesRestoreScope() {
        let managed = agentEntry(
            id: "claude",
            name: "Claude Code",
            state: .managed,
            detail: "恢复 ~/.claude/settings.json 中的网关字段。",
            cli: cliInfo(version: "1.0.0")
        )
        let confirmation = agentDisconnectConfirmation(for: managed)
        precondition(confirmation.title == "Disconnect Claude Code?")
        precondition(confirmation.message.contains("Claude Code"))
        precondition(confirmation.message.contains("~/.claude/settings.json"))
        precondition(confirmation.message.contains("keeps running"))

        let withoutDetail = agentEntry(
            id: "pi",
            name: "Pi",
            state: .managed,
            cli: cliInfo(version: "1.0.0")
        )
        let fallback = agentDisconnectConfirmation(for: withoutDetail)
        precondition(
            fallback.message.contains(
                "Restores the configuration and sign-in state from before the connection."
            )
        )
        precondition(agentDisconnectScopeText(withoutDetail).contains("Restores the configuration"))
    }

    @MainActor
    private static func testWindowBuildsContentAndFoldStartsClosed() async {
        let model = makeModel(
            list: [
                upgradableEntry(id: "codex", name: "Codex"),
                upgradableEntry(id: "claude", name: "Claude Code"),
                agentEntry(
                    id: "pi",
                    name: "Pi",
                    detected: false,
                    cli: cliInfo(version: "", path: "", update: false)
                ),
            ]
        )
        let controller = AgentManagementWindowController(model: model)
        guard let window = controller.window, let contentView = window.contentView else {
            preconditionFailure("the agent window must build a content view")
        }
        precondition(window.title == "Agent Connections")
        precondition(window.minSize.width >= 720)
        // SwiftUI draws without AppKit subviews; check that it hosts content.
        precondition(window.contentViewController?.view === contentView)
        precondition(contentView.fittingSize.width > 0)
        await model.reload()
        precondition(model.detectedEntries.count == 2)
        precondition(model.undetectedEntries.count == 1)
        precondition(model.undetectedFoldTitle == "Agents not detected (1)")
        precondition(!model.showsUndetectedAgents)
        model.toggleUndetectedAgents()
        precondition(model.showsUndetectedAgents)
        precondition(model.showsUpdateAllButton)
        precondition(model.canUpgradeAll)
    }

    // MARK: - Refresh and upgrades

    @MainActor
    private static func testRefreshMergesVersions() async {
        let model = makeModel(
            list: [upgradableEntry(id: "codex", name: "Codex", version: "0.9.0", latest: "0.10.0")],
            versions: ["codex": cliInfo(version: "0.10.0", latest: "0.10.0", update: false)]
        )
        await model.refresh()
        precondition(model.entry(withID: "codex")?.cli.version == "0.10.0")
        precondition(model.entry(withID: "codex")?.cli.update == false)
        precondition(model.upgradableCount == 0)
        precondition(model.lastCheckedAt != nil)
        precondition(model.lastCheckedText?.isEmpty == false)
        precondition(model.loadError == nil)

        let failing = makeModel(
            list: [upgradableEntry(id: "codex", name: "Codex")],
            versionsError: FakeError(description: "CLI 未返回版本")
        )
        await failing.refresh()
        precondition(failing.entry(withID: "codex")?.cli.version == "1.0.0")
        precondition(failing.status?.isError == true)
        precondition(failing.status?.text.contains("CLI 未返回版本") == true)
    }

    @MainActor
    private static func testSingleUpgradeUpdatesRowAndBlocksReentry() async {
        let gate = OperationGate()
        var upgradeCalls = 0
        let model = AgentManagementModel(
            listHandler: {
                AgentListResponse(agents: [upgradableEntry(id: "codex", name: "Codex")])
            },
            versionsHandler: { AgentVersionsResponse(agents: []) },
            upgradeHandler: { entry in
                upgradeCalls += 1
                await gate.enter()
                return AgentUpgradeResponse(
                    id: entry.id,
                    cli: cliInfo(version: "1.1.0", latest: "1.1.0", update: false)
                )
            },
            connectHandler: { _, _ in },
            disconnectHandler: { _ in },
            confirmHandler: { _, _ in false }
        )
        await model.reload()
        let first = Task { await model.upgrade(agentID: "codex") }
        await gate.waitUntilEntered()
        precondition(model.isBusy(agentID: "codex"))
        await model.upgrade(agentID: "codex")
        precondition(upgradeCalls == 1, "a second upgrade must not start while one is running")
        gate.open()
        await first.value
        precondition(!model.isBusy(agentID: "codex"))
        precondition(model.entry(withID: "codex")?.cli.version == "1.1.0")
        precondition(model.entry(withID: "codex")?.cli.update == false)
        precondition(model.status?.text == "Codex updated to 1.1.0")
        precondition(model.status?.isError == false)
    }

    @MainActor
    private static func testUpgradeFailureReportsError() async {
        let model = AgentManagementModel(
            listHandler: {
                AgentListResponse(agents: [upgradableEntry(id: "codex", name: "Codex")])
            },
            versionsHandler: { AgentVersionsResponse(agents: []) },
            upgradeHandler: { _ in
                throw FakeError(description: "npm 权限不足")
            },
            connectHandler: { _, _ in },
            disconnectHandler: { _ in },
            confirmHandler: { _, _ in false }
        )
        await model.reload()
        await model.upgrade(agentID: "codex")
        precondition(model.status?.isError == true)
        precondition(model.status?.text == "Unable to update Codex: npm 权限不足")
        precondition(model.entry(withID: "codex")?.cli.update == true)
        precondition(!model.isBusy(agentID: "codex"))
    }

    @MainActor
    private static func testUpgradeAllRunsSequentiallyInOrder() async {
        let gate = OperationGate()
        var order: [String] = []
        let model = AgentManagementModel(
            listHandler: {
                AgentListResponse(agents: [
                    upgradableEntry(id: "opencode", name: "OpenCode"),
                    upgradableEntry(id: "codex", name: "Codex"),
                    upgradableEntry(id: "claude", name: "Claude Code"),
                    agentEntry(id: "pi", name: "Pi", cli: cliInfo(version: "1.0.0")),
                ])
            },
            versionsHandler: { AgentVersionsResponse(agents: []) },
            upgradeHandler: { entry in
                order.append(entry.id)
                if entry.id == "codex" {
                    await gate.enter()
                }
                return AgentUpgradeResponse(
                    id: entry.id,
                    cli: cliInfo(version: "1.1.0", latest: "1.1.0", update: false)
                )
            },
            connectHandler: { _, _ in },
            disconnectHandler: { _ in },
            confirmHandler: { _, _ in false }
        )
        await model.reload()
        precondition(model.upgradableCount == 3)
        precondition(model.canUpgradeAll)
        let batch = Task { await model.upgradeAll() }
        await gate.waitUntilEntered()
        precondition(order == ["codex"])
        precondition(model.batch?.total == 3)
        precondition(model.batch?.currentAgentName == "Codex")
        await model.upgradeAll()
        precondition(order == ["codex"], "a second batch must not start while one is running")
        gate.open()
        await batch.value
        precondition(order == ["codex", "claude", "opencode"])
        precondition(model.batch == nil)
        precondition(model.upgradableCount == 0)
        precondition(model.status?.isError == false)
        precondition(model.status?.text == "Updated 3 of 3: Codex 1.1.0, Claude Code 1.1.0, OpenCode 1.1.0")
    }

    @MainActor
    private static func testUpgradeAllContinuesAfterFailure() async {
        var attempted: [String] = []
        let model = AgentManagementModel(
            listHandler: {
                AgentListResponse(agents: [
                    upgradableEntry(id: "codex", name: "Codex"),
                    upgradableEntry(id: "claude", name: "Claude Code"),
                    upgradableEntry(id: "opencode", name: "OpenCode"),
                ])
            },
            versionsHandler: { AgentVersionsResponse(agents: []) },
            upgradeHandler: { entry in
                attempted.append(entry.id)
                if entry.id == "claude" {
                    throw FakeError(description: "npm 权限不足")
                }
                return AgentUpgradeResponse(
                    id: entry.id,
                    cli: cliInfo(version: "1.1.0", latest: "1.1.0", update: false)
                )
            },
            connectHandler: { _, _ in },
            disconnectHandler: { _ in },
            confirmHandler: { _, _ in false }
        )
        await model.reload()
        await model.upgradeAll()
        precondition(attempted == ["codex", "claude", "opencode"])
        precondition(model.entry(withID: "codex")?.cli.update == false)
        precondition(model.entry(withID: "claude")?.cli.update == true)
        precondition(model.entry(withID: "opencode")?.cli.update == false)
        precondition(model.status?.isError == true)
        precondition(model.status?.text.contains("Updated 2 of 3") == true)
        precondition(model.status?.text.contains("Claude Code") == true)
        precondition(model.status?.text.contains("npm 权限不足") == true)
        precondition(model.batch == nil)
    }

    // MARK: - Connect and disconnect

    @MainActor
    private static func testConnectRequiresModeForCodexOnly() async {
        let recorder = CommandRecorder()
        let model = AgentManagementModel(
            listHandler: {
                AgentListResponse(agents: [
                    agentEntry(id: "codex", name: "Codex", cli: cliInfo(version: "0.9.0")),
                    agentEntry(id: "claude", name: "Claude Code", cli: cliInfo(version: "1.0.0")),
                ])
            },
            versionsHandler: { AgentVersionsResponse(agents: []) },
            upgradeHandler: { entry in
                AgentUpgradeResponse(id: entry.id, cli: cliInfo(version: "1.0.0"))
            },
            connectHandler: { entry, mode in
                recorder.commands.append(
                    agentConnectArguments(id: entry.id, mode: mode)
                )
                recorder.connectModes.append(mode)
            },
            disconnectHandler: { entry in
                recorder.commands.append(agentDisconnectArguments(id: entry.id))
            },
            connectModePicker: { entry in
                recorder.pickerRequests.append(entry.id)
                return .customOnly
            },
            confirmHandler: { _, _ in true }
        )
        await model.reload()
        await model.connect(agentID: "codex")
        await model.connect(agentID: "claude")
        precondition(recorder.pickerRequests == ["codex"])
        precondition(recorder.connectModes.count == 2)
        precondition(recorder.connectModes[0] == .customOnly)
        precondition(recorder.connectModes[1] == nil)
        precondition(
            recorder.commands[0] == ["agent", "connect", "codex", "--mode", "custom-only"]
        )
        precondition(recorder.commands[1] == ["agent", "connect", "claude"])
        precondition(model.status?.text == "Claude Code is connected · restart Claude Code or start a new session")
        precondition(!model.isOperating)
    }

    @MainActor
    private static func testCancelledCodexModePanelLeavesAgentUntouched() async {
        var connectCalls = 0
        let model = AgentManagementModel(
            listHandler: {
                AgentListResponse(agents: [
                    agentEntry(id: "codex", name: "Codex", cli: cliInfo(version: "0.9.0")),
                ])
            },
            versionsHandler: { AgentVersionsResponse(agents: []) },
            upgradeHandler: { entry in
                AgentUpgradeResponse(id: entry.id, cli: cliInfo(version: "1.0.0"))
            },
            connectHandler: { _, _ in connectCalls += 1 },
            disconnectHandler: { _ in },
            connectModePicker: { _ in nil },
            confirmHandler: { _, _ in true }
        )
        await model.reload()
        var changeCount = 0
        let cancellable = model.objectWillChange.sink { changeCount += 1 }
        await model.connect(agentID: "codex")
        cancellable.cancel()
        precondition(connectCalls == 0)
        precondition(!model.isOperating)
        precondition(model.status == nil)
        precondition(
            changeCount > 0,
            "a cancelled mode panel must re-render the switch from the CLI state"
        )
    }

    @MainActor
    private static func testDisconnectRestoresOneAgentOnly() async {
        let recorder = CommandRecorder()
        var confirmations: [(String, String)] = []
        let managedClaude = agentEntry(
            id: "claude",
            name: "Claude Code",
            state: .managed,
            detail: "恢复 ~/.claude/settings.json 中的网关字段。",
            cli: cliInfo(version: "1.0.0")
        )
        let model = AgentManagementModel(
            listHandler: {
                recorder.commands.append(agentListArguments())
                return AgentListResponse(agents: [
                    managedClaude,
                    agentEntry(id: "codex", name: "Codex", state: .managed, cli: cliInfo(version: "0.9.0")),
                ])
            },
            versionsHandler: { AgentVersionsResponse(agents: []) },
            upgradeHandler: { entry in
                AgentUpgradeResponse(id: entry.id, cli: cliInfo(version: "1.0.0"))
            },
            connectHandler: { entry, mode in
                recorder.commands.append(agentConnectArguments(id: entry.id, mode: mode))
            },
            disconnectHandler: { entry in
                recorder.commands.append(agentDisconnectArguments(id: entry.id))
            },
            connectModePicker: { _ in nil },
            confirmHandler: { title, message in
                confirmations.append((title, message))
                return true
            }
        )
        await model.reload()
        await model.disconnect(agentID: "claude")
        precondition(confirmations.count == 1)
        precondition(confirmations[0].0 == "Disconnect Claude Code?")
        precondition(confirmations[0].1.contains("~/.claude/settings.json"))
        precondition(confirmations[0].1.contains("keeps running"))
        precondition(
            recorder.commands.contains(["agent", "disconnect", "claude"]),
            "disconnect must go through the agent command"
        )
        precondition(
            recorder.commands.contains(["agent", "list", "--json"]),
            "the window re-lists after disconnecting"
        )
        for command in recorder.commands {
            precondition(!command.contains("service"), "disconnect must not touch the gateway service")
            precondition(!command.contains("stop"))
            precondition(!command.contains("--restore-clients"))
        }
        precondition(model.status?.text == "Claude Code is disconnected · restart Claude Code or start a new session")
        precondition(!model.isOperating)

        var declinedCalls = 0
        let declining = AgentManagementModel(
            listHandler: { AgentListResponse(agents: [managedClaude]) },
            versionsHandler: { AgentVersionsResponse(agents: []) },
            upgradeHandler: { entry in
                AgentUpgradeResponse(id: entry.id, cli: cliInfo(version: "1.0.0"))
            },
            connectHandler: { _, _ in },
            disconnectHandler: { _ in declinedCalls += 1 },
            confirmHandler: { _, _ in false }
        )
        await declining.reload()
        await declining.disconnect(agentID: "claude")
        precondition(declinedCalls == 0, "a declined confirmation must not disconnect")
    }

    // MARK: - Helpers

    @MainActor
    private static func makeModel(
        list: [AgentEntry],
        versions: [String: AgentCLIInfo] = [:],
        versionsError: Error? = nil
    ) -> AgentManagementModel {
        AgentManagementModel(
            listHandler: { AgentListResponse(agents: list) },
            versionsHandler: {
                if let versionsError {
                    throw versionsError
                }
                return AgentVersionsResponse(
                    agents: versions.map { AgentVersionsEntry(id: $0.key, cli: $0.value) }
                )
            },
            upgradeHandler: { entry in
                AgentUpgradeResponse(id: entry.id, cli: cliInfo(version: "1.1.0"))
            },
            connectHandler: { _, _ in },
            disconnectHandler: { _ in },
            connectModePicker: { _ in nil },
            confirmHandler: { _, _ in false }
        )
    }
}
