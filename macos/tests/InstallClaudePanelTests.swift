import Foundation

@main
struct InstallClaudePanelTests {
    static func main() throws {
        let options = try decodeClaudeModelOptions(
            """
            {
              "providers": [{
                "id":"provider",
                "display_name":"Provider",
                "enabled":true,
                "protocol":"anthropic_messages",
                "selected_models":["opus-backend","sonnet-backend","haiku-backend","responses"],
                "cached_models":[
                  {"id":"opus-backend","display_name":"严谨处理复杂长任务、精准遵循指令、能主动自我验证输出（Thinking）"},
                  {"id":"sonnet-backend","display_name":"Sonnet Backend"},
                  {"id":"haiku-backend","display_name":"Haiku Backend"},
                  {"id":"responses","display_name":"Responses","protocol":"open_ai_responses"}
                ]
              }, {
                "id":"disabled",
                "display_name":"Disabled",
                "enabled":false,
                "selected_models":["hidden"],
                "cached_models":[{"id":"hidden"}]
              }, {
                "id":"partial",
                "display_name":"Partial",
                "enabled":true,
                "selected_models":["selected-only","shared"],
                "cached_models":[{"id":"cached-only"},{"id":"shared"}]
              }, {
                "id":"official",
                "kind":"official",
                "display_name":"OpenAI",
                "enabled":true,
                "selected_models":["gpt-5.6-sol"],
                "cached_models":[{"id":"gpt-5.6-sol"},{"id":"gpt-5.5"}]
              }]
            }
            """
        )
        precondition(options.map(\.id) == [
            "gpt-5.6-sol",
            "haiku-backend-provider",
            "opus-backend-provider",
            "responses-provider",
            "shared-partial",
            "sonnet-backend-provider",
        ])
        precondition(!options.contains { $0.id == "hidden-disabled" })
        precondition(!options.contains { $0.id == "selected-only-partial" })
        precondition(!options.contains { $0.id == "cached-only-partial" })
        precondition(!options.contains { $0.id == "gpt-5.5" })
        precondition(
            options.first { $0.id == "opus-backend-provider" }?.displayName
                == "opus-backend · Provider"
        )

        let mapping = try suggestedClaudeModelMapping(options: options)
        precondition(mapping.opus == "opus-backend-provider")
        precondition(mapping.sonnet == "sonnet-backend-provider")
        precondition(mapping.haiku == "haiku-backend-provider")
        let overridden = ClaudeModelMapping(
            opus: mapping.opus,
            sonnet: mapping.sonnet,
            haiku: mapping.haiku,
            opusOverride: "arn:aws:bedrock:us-east-1:123:application-inference-profile/opus",
            sonnetOverride: "",
            haikuOverride: ""
        )
        precondition(overridden.commandArguments == [
            "install-claude",
            "--opus-model", "opus-backend-provider",
            "--sonnet-model", "sonnet-backend-provider",
            "--haiku-model", "haiku-backend-provider",
            "--opus-model-override", "arn:aws:bedrock:us-east-1:123:application-inference-profile/opus",
        ])

        do {
            _ = try suggestedClaudeModelMapping(options: [])
            preconditionFailure("empty model options must fail")
        } catch ClaudeInstallPanelError.noModels {
        }

        print("Install Claude panel: passed")
    }
}
