import Foundation

enum GatewayError: Error {
    case command(String)
}

@main
struct ProviderSupportTests {
    static func main() throws {
        precondition(!baiduBridgeNeedsSetup(current: .ducxLoopback, selected: .ducxLoopback))
        precondition(!baiduBridgeNeedsSetup(current: .ducxLoopback, selected: .disabled))
        precondition(baiduBridgeNeedsSetup(current: nil, selected: .ducxLoopback))
        precondition(baiduBridgeNeedsSetup(current: .disabled, selected: .ducxLoopback))

        precondition(isManagedVersion("10.145.0.4", newerThan: "10.145.0.3"))
        precondition(!isManagedVersion("10.145.0.3", newerThan: "10.145.0.3"))
        precondition(!isManagedVersion("10.144.9.9", newerThan: "10.145.0.3"))
        precondition(
            modelCapabilityProbeCounts(
                "Probing capabilities for baidu-oneapi: 3/12 complete (2 routed, 1 indeterminate)"
            )?.completed == 3
        )
        precondition(
            modelCapabilityProbeCounts(
                "Probing capabilities for baidu-oneapi: 3/12 complete (2 routed, 1 indeterminate)"
            )?.total == 12
        )
        precondition(
            modelCapabilityProbeCounts(
                "Probing capabilities for baidu-oneapi: 13/12 complete (2 routed, 1 indeterminate)"
            ) == nil
        )
        precondition(modelCapabilityProbeCounts("Discovered 12 models for provider baidu-oneapi") == nil)

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
                  "baidu_auth_bridge": "ducx_loopback",
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
                },
                {
                  "id": "opencode-go",
                  "display_name": "OpenCode Go",
                  "enabled": true,
                  "auxiliary_model_upstream": false,
                  "preset_id": "opencode-go",
                  "protocol": "open_ai_responses",
                  "base_url": "https://opencode.ai/zen/go",
                  "api_path": "/v1/responses",
                  "model_source": {"kind": "open_ai_compatible", "path": "/v1/models"},
                  "api_key_configured": true,
                  "quota_workspace_id": "wrk_abc",
                  "quota_auth_cookie_configured": true,
                  "quota_parser": "opencode_go",
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

        let officialResponse = try decodeProviderList(
            """
            {
              "config_version": 2,
              "gateway_auth_configured": true,
              "codex_install_mode": "codex_oauth_proxy",
              "providers": [{
                "id": "official",
                "kind": "official",
                "display_name": "OpenAI",
                "enabled": true,
                "auxiliary_model_upstream": false,
                "protocol": "open_ai_responses",
                "base_url": "https://chatgpt.com/backend-api/codex",
                "website_url": "https://chatgpt.com",
                "api_path": "/responses",
                "model_source": {"kind": "static"},
                "api_key_configured": true,
                "quota_auth_cookie_configured": false,
                "quota_parser": "generic",
                "selected_models": ["gpt-5.6-sol"],
                "new_models": [],
                "unavailable_selected_models": [],
                "cached_models": [{"id":"gpt-5.6-sol"},{"id":"gpt-5.5"}],
                "readiness": "healthy",
                "readiness_issues": [],
                "routable_model_count": 0
              }]
            }
            """
        )

        let baidu = response.providers[0]
        let autoReviewOnly = response.providers[1]
        let unsupported = response.providers[2]
        let openCodeGo = response.providers[3]
        let official = officialResponse.providers[0]
        precondition(response.codexInstallMode == .customOnly)
        precondition(official.kind == .official)
        precondition(official.supportsModelRefresh)
        precondition(baidu.supportsModelRefresh)
        precondition(!official.needsInitialModelDiscovery)
        precondition(unsupported.needsInitialModelDiscovery)
        precondition(baidu.baiduAuthBridge == .ducxLoopback)
        precondition(baidu.effectiveBaiduAuthBridge == .ducxLoopback)
        precondition(baidu.apiPath == "/v1/messages")
        precondition(baidu.modelsPath == "/v1/models")
        precondition(autoReviewOnly.apiPath == "/v1/chat/completions")
        precondition(autoReviewOnly.modelsPath == "/v1/models")
        precondition(autoReviewOnly.effectiveBaiduAuthBridge == nil)
        precondition(baidu.auxiliaryModelUpstream)
        precondition(!autoReviewOnly.auxiliaryModelUpstream)
        precondition(baidu.auxiliaryModelSupport == .autoReviewAndVoice)
        precondition(autoReviewOnly.auxiliaryModelSupport == .autoReviewOnly)
        precondition(unsupported.auxiliaryModelSupport == .none)
        precondition(openCodeGo.quotaWorkspaceID == "wrk_abc")
        precondition(openCodeGo.quotaAuthCookieConfigured == true)
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
        let providerOptions = modelSelectionProviderOptions(response.providers)
        precondition(providerOptions.count == response.providers.count)
        precondition(Set(providerOptions.map(\.id)) == Set(response.providers.map(\.id)))
        precondition(modelSelectionProviderOptions(response.providers + [official]).count == 5)
        let officialSelectedKeys = selectedProviderModelKeys([official])
        precondition(
            officialSelectedKeys.contains(
                providerModelSelectionKey(providerID: "official", modelID: "gpt-5.6-sol")
            )
        )
        precondition(
            providerModelSelections([official], selectedKeys: officialSelectedKeys)["official"]
                == ["gpt-5.6-sol"]
        )
        let benchmarkColumns = modelBenchmarkColumnDefinitions()
        precondition(
            benchmarkColumns.map(\.title)
                == [
                    "加入 Codex", "上游模型", "TTFT", "吞吐", "上下文", "倍率", "协议",
                    "图片", "Tool Search", "Web Search", "Function Tools", "Thinking",
                ]
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
