import Foundation

enum GatewayError: Error {
    case command(String)
}

@main
struct CodexGatewaySwitchStateTests {
    static func main() throws {
        let managed = try decodeCodexIntegrationStatus(
            """
            {
              "integration": "managed",
              "mode": "codex_oauth_proxy",
              "gateway_required": true,
              "restore_mode": null
            }
            """
        )
        precondition(managed.integration == .managed)
        precondition(managed.mode == .codexOAuthProxy)
        precondition(managed.gatewayRequired)
        precondition(managed.restoreMode == nil)
        precondition(!managed.isOfficialMode)

        let official = try decodeCodexIntegrationStatus(
            """
            {
              "integration": "unmanaged",
              "mode": null,
              "gateway_required": false,
              "restore_mode": "custom_only"
            }
            """
        )
        precondition(official.integration == .unmanaged)
        precondition(official.mode == nil)
        precondition(!official.gatewayRequired)
        precondition(official.restoreMode == .customOnly)
        precondition(official.isOfficialMode)
        precondition(stoppedGatewayStatusTitle(codexStatus: official) == "官方 Codex 模式")
        precondition(
            providerDisplayNameForGatewayState(
                "DeepSeek",
                isGatewayRunning: false,
                codexStatus: official
            ) == "DeepSeek（Mixin 配置已停用）"
        )
        precondition(
            providerDisplayNameForGatewayState(
                "DeepSeek",
                isGatewayRunning: true,
                codexStatus: official
            ) == "DeepSeek"
        )
        precondition(stoppedGatewayStatusTitle(codexStatus: nil) == "本地服务已停止")
        print("Codex gateway switch state tests passed")
    }
}
