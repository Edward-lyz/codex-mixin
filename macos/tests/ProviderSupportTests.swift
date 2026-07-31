import Foundation

enum GatewayError: Error {
    case command(String)
}

@main
struct ProviderSupportTests {
    static func main() throws {
        let fileManager = FileManager.default
        let testHome = fileManager.temporaryDirectory
            .appendingPathComponent("codex-mixin-managed-ducx-\(UUID().uuidString)")
        try fileManager.createDirectory(at: testHome, withIntermediateDirectories: true)
        defer { try? fileManager.removeItem(at: testHome) }
        let systemDucx = testHome.appendingPathComponent(".baidu-cx/baidu-cx/bin/ducx")
        try fileManager.createDirectory(
            at: systemDucx.deletingLastPathComponent(),
            withIntermediateDirectories: true
        )
        precondition(fileManager.createFile(atPath: systemDucx.path, contents: Data()))
        try fileManager.setAttributes(
            [.posixPermissions: 0o700],
            ofItemAtPath: systemDucx.path
        )
        precondition(
            managedDucxExecutableURL(
                homeDirectory: testHome,
                fileManager: fileManager
            ) == nil,
            "A system DUCX installation must not satisfy the managed DUCX requirement"
        )
        let managedDucx = managedDucxRoot(homeDirectory: testHome)
            .appendingPathComponent("current/bin/ducx")
        try fileManager.createDirectory(
            at: managedDucx.deletingLastPathComponent(),
            withIntermediateDirectories: true
        )
        precondition(fileManager.createFile(atPath: managedDucx.path, contents: Data()))
        try fileManager.setAttributes(
            [.posixPermissions: 0o700],
            ofItemAtPath: managedDucx.path
        )
        try "10.145.0.3\n".write(
            to: managedDucx
                .deletingLastPathComponent()
                .deletingLastPathComponent()
                .appendingPathComponent("version"),
            atomically: true,
            encoding: .utf8
        )
        precondition(
            managedDucxExecutableURL(
                homeDirectory: testHome,
                fileManager: fileManager
            ) == managedDucx
        )
        precondition(
            managedDucxInstalledVersion(
                homeDirectory: testHome,
                fileManager: fileManager
            ) == "10.145.0.3",
            "Managed DUCX version must come from the active package"
        )
        let systemDucc = testHome.appendingPathComponent(".baidu-cc/baidu-cc/bin/ducc")
        try fileManager.createDirectory(
            at: systemDucc.deletingLastPathComponent(),
            withIntermediateDirectories: true
        )
        precondition(fileManager.createFile(atPath: systemDucc.path, contents: Data()))
        try fileManager.setAttributes(
            [.posixPermissions: 0o700],
            ofItemAtPath: systemDucc.path
        )
        precondition(
            managedDuccExecutableURL(
                homeDirectory: testHome,
                fileManager: fileManager
            ) == nil,
            "A system DUCC installation must not satisfy the managed DUCC requirement"
        )
        let managedDucc = managedDuccInstallRoot(homeDirectory: testHome)
            .appendingPathComponent("baidu-cc/bin/ducc")
        try fileManager.createDirectory(
            at: managedDucc.deletingLastPathComponent(),
            withIntermediateDirectories: true
        )
        precondition(fileManager.createFile(atPath: managedDucc.path, contents: Data()))
        try fileManager.setAttributes(
            [.posixPermissions: 0o700],
            ofItemAtPath: managedDucc.path
        )
        try "2.1.218.3\n".write(
            to: managedDucc
                .deletingLastPathComponent()
                .deletingLastPathComponent()
                .appendingPathComponent("version"),
            atomically: true,
            encoding: .utf8
        )
        precondition(
            managedDuccExecutableURL(
                homeDirectory: testHome,
                fileManager: fileManager
            ) == managedDucc
        )
        precondition(
            managedDuccInstalledVersion(
                homeDirectory: testHome,
                fileManager: fileManager
            ) == "2.1.218.3",
            "Managed DUCC version must come from the active package"
        )
        precondition(
            managedDuccHome(homeDirectory: testHome).path
                .hasSuffix(".codex-mixin/ducc/home"),
            "Managed DUCC must use a dedicated HOME"
        )
        precondition(isManagedVersion("10.145.0.4", newerThan: "10.145.0.3"))
        precondition(!isManagedVersion("10.145.0.3", newerThan: "10.145.0.3"))
        precondition(!isManagedVersion("10.144.9.9", newerThan: "10.145.0.3"))

        let response = try decodeProviderList(
            """
            {
              "config_version": 1,
              "gateway_auth_configured": false,
              "codex_install_mode": "custom_only",
              "providers": [
                {
                  "id": "baidu-oneapi",
                  "display_name": "Baidu OneAPI",
                  "enabled": true,
                  "auxiliary_model_upstream": true,
                  "preset_id": "baidu-oneapi",
                  "protocol": "anthropic_messages",
                  "base_url": "https://example.com",
                  "api_path": "/v1/messages",
                  "model_source": {"kind": "baidu_oneapi", "path": "/v1/models"},
                  "api_key_configured": true,
                  "quota_parser": "baidu_one_api",
                  "ducx_app_server": true,
                  "selected_models": ["Claude Opus 4.6"],
                  "new_models": [],
                  "unavailable_selected_models": [],
                  "cached_models": [
                    {
                      "id": "Claude Opus 4.6",
                      "description": "自主规划更周密",
                      "ratio": "1.4x",
                      "price_type": "昂贵模型",
                      "context_window": 1000000
                    },
                    {
                      "id": "codex-auto-review"
                    },
                    {
                      "id": "gpt-realtime-1.5"
                    }
                  ],
                  "readiness": "healthy",
                  "readiness_issues": [],
                  "routable_model_count": 1
                },
                {
                  "id": "custom",
                  "display_name": "Custom",
                  "enabled": true,
                  "auxiliary_model_upstream": false,
                  "preset_id": "custom",
                  "protocol": "open_ai_chat",
                  "base_url": "https://example.com",
                  "api_path": "/v1/chat/completions",
                  "model_source": {"kind": "open_ai_compatible", "path": "/v1/models"},
                  "api_key_configured": true,
                  "quota_parser": "generic",
                  "selected_models": [],
                  "new_models": [],
                  "unavailable_selected_models": [],
                  "cached_models": [
                    {
                      "id": "codex-auto-review"
                    }
                  ],
                  "readiness": "healthy",
                  "readiness_issues": [],
                  "routable_model_count": 0
                },
                {
                  "id": "unsupported",
                  "display_name": "Unsupported",
                  "enabled": true,
                  "auxiliary_model_upstream": false,
                  "preset_id": "custom",
                  "protocol": "open_ai_chat",
                  "base_url": "https://example.com",
                  "api_path": "/v1/chat/completions",
                  "model_source": {"kind": "open_ai_compatible", "path": "/v1/models"},
                  "api_key_configured": true,
                  "quota_parser": "generic",
                  "selected_models": [],
                  "new_models": [],
                  "unavailable_selected_models": [],
                  "cached_models": [],
                  "readiness": "healthy",
                  "readiness_issues": [],
                  "routable_model_count": 0
                }
              ]
            }
            """
        )

        let baidu = response.providers[0]
        let autoReviewOnly = response.providers[1]
        let unsupported = response.providers[2]
        precondition(response.codexInstallMode == .customOnly)
        precondition(baidu.ducxAppServer == true)
        precondition(baidu.baiduAuthBridge == nil)
        precondition(baidu.effectiveBaiduAuthBridge == .ducxAppServer)
        precondition(autoReviewOnly.ducxAppServer == nil)
        precondition(autoReviewOnly.effectiveBaiduAuthBridge == nil)
        precondition(baidu.auxiliaryModelUpstream)
        precondition(!autoReviewOnly.auxiliaryModelUpstream)
        precondition(baidu.auxiliaryModelSupport == .autoReviewAndVoice)
        precondition(autoReviewOnly.auxiliaryModelSupport == .autoReviewOnly)
        precondition(unsupported.auxiliaryModelSupport == .none)
        precondition(
            isAuxiliaryModelUpstreamSelectable(
                for: baidu,
                codexInstallMode: response.codexInstallMode
            )
        )
        precondition(
            isAuxiliaryModelUpstreamSelectable(
                for: autoReviewOnly,
                codexInstallMode: response.codexInstallMode
            )
        )
        precondition(
            !isAuxiliaryModelUpstreamSelectable(
                for: unsupported,
                codexInstallMode: response.codexInstallMode
            )
        )
        precondition(
            isAuxiliaryModelUpstreamSelectable(
                for: unsupported,
                codexInstallMode: .codexOAuthProxy
            )
        )
        precondition(baidu.modelItems[0].ratio == "1.4x")
        precondition(baidu.modelItems[0].priceType == "昂贵模型")
        precondition(shouldShowModelRatioColumn(for: baidu))
        precondition(!shouldShowModelRatioColumn(for: response.providers[1]))
        precondition(!shouldShowModelRatioColumn(for: nil))
        let selectedKeys = selectedProviderModelKeys(response.providers)
        precondition(
            selectedKeys.contains(
                providerModelSelectionKey(providerID: baidu.id, modelID: baidu.selectedModels[0])
            )
        )
        let selections = providerModelSelections(response.providers, selectedKeys: selectedKeys)
        precondition(selections[baidu.id] == baidu.selectedModels)
        let providerOptions = configuredProviderOptions(response.providers)
        precondition(providerOptions.count == response.providers.count)
        precondition(Set(providerOptions.map(\.id)) == Set(response.providers.map(\.id)))
        let benchmarkColumns = modelBenchmarkColumnDefinitions()
        precondition(
            benchmarkColumns.map(\.title)
                == ["加入 Codex", "上游模型", "TTFT", "吞吐", "上下文", "倍率"]
        )
        precondition(Set(benchmarkColumns.map(\.id)).count == benchmarkColumns.count)
        precondition(benchmarkRatioValue("0.5x") == 0.5)
        precondition(benchmarkRatioValue(nil) == nil)
        precondition(
            providerIssueDetails(
                fromGatewayStatus: """
                gateway: running
                provider-readiness: degraded
                provider-issue: Baidu OneAPI：模型 unreachable-model 当前不可达
                provider-issue: AIHub：模型列表刷新失败：upstream returned 503
                """
            ) == [
                "Baidu OneAPI：模型 unreachable-model 当前不可达",
                "AIHub：模型列表刷新失败：upstream returned 503",
            ]
        )
        print("Provider model ratio support: passed")
    }
}
