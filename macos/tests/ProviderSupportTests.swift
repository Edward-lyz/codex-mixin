import Foundation

enum GatewayError: Error {
    case command(String)
}

@main
struct ProviderSupportTests {
    static func main() throws {
        let response = try decodeProviderList(
            """
            {
              "config_version": 1,
              "gateway_auth_configured": false,
              "providers": [
                {
                  "id": "baidu-oneapi",
                  "display_name": "Baidu OneAPI",
                  "enabled": true,
                  "preset_id": "baidu-oneapi",
                  "protocol": "anthropic_messages",
                  "base_url": "https://example.com",
                  "api_path": "/v1/messages",
                  "model_source": {"kind": "baidu_oneapi", "path": "/v1/models"},
                  "api_key_configured": true,
                  "quota_parser": "baidu_one_api",
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
        precondition(baidu.modelItems[0].ratio == "1.4x")
        precondition(baidu.modelItems[0].priceType == "昂贵模型")
        precondition(shouldShowModelRatioColumn(for: baidu))
        precondition(!shouldShowModelRatioColumn(for: response.providers[1]))
        precondition(!shouldShowModelRatioColumn(for: nil))
        print("Provider model ratio support: passed")
    }
}
